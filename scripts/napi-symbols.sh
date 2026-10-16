#!/usr/bin/env bash
# VELYR-95: list the napi_* / node_api_* symbols that native Node addons import.
#
# Usage: scripts/napi-symbols.sh <workdir> [headers-ref]
#   workdir      directory for tarballs, extracted .node files and results (created if missing)
#   headers-ref  nodejs/node git ref used to map symbol -> minimum N-API version (default: v24.x)
#
# Safety: downloads come from `npm pack --ignore-scripts` (no lifecycle scripts). Files are
# only extracted and read with nm. Nothing downloaded is executed, loaded or installed.
# Platform: darwin-arm64 prebuilds. On Linux use `nm -D -u <file>.node`; on Windows use
# `dumpbin /imports <file>.node`; both can feed the same napi_/node_api_ filter.
set -euo pipefail

WORKDIR=${1:?usage: napi-symbols.sh <workdir> [headers-ref]}
HEADERS_REF=${2:-v24.x}
NAPI_FILTER='^(napi_|node_api_)'

# label|npm package|regex of the tar member to analyze
PACKAGES=(
  'rolldown|@rolldown/binding-darwin-arm64|\.node$'
  'swc|@swc/core-darwin-arm64|\.node$'
  'lightningcss|lightningcss-darwin-arm64|\.node$'
  'sharp|@img/sharp-darwin-arm64|\.node$'
  'better-sqlite3|better-sqlite3|prebuilds/darwin-arm64\.node$'
  'bcrypt|bcrypt|prebuilds/darwin-arm64/[^/]*\.node$'
  'argon2|argon2|prebuilds/darwin-arm64/[^/]*\.node$'
  'bufferutil|bufferutil|prebuilds/darwin-arm64/[^/]*\.node$'
  'utf-8-validate|utf-8-validate|prebuilds/darwin-arm64/[^/]*\.node$'
  'esbuild|@esbuild/darwin-arm64|bin/esbuild$'
)

for tool in npm tar nm otool zstd perl curl shasum; do
  command -v "$tool" > /dev/null || { echo "missing required tool: $tool" >&2; exit 1; }
done

mkdir -p "$WORKDIR"/{tarballs,extracted,undefined,exports,host-other}
WORKDIR=$(cd "$WORKDIR" && pwd)

# Mach-O prefixes C symbols with "_"; ELF may append "@VERSION".
normalize() { sed -E 's/^_//; s/@.*$//' | { grep -E "$NAPI_FILTER" || true; } | sort -u; }

# swc ships a "carrier" .node whose real addon is a zstd blob in the __swc_native Mach-O section.
# Decompressing it is pure data processing; the result is only read with nm, never loaded.
# Return 0 = unwrapped, 2 = file has no carrier section, 1 = carrier found but unwrap failed.
unwrap_swc_carrier() {
  local carrier=$1 out=$2 info offset size frame
  info=$(otool -l "$carrier" | awk '/sectname __swc_native/ {f=1} f && $1=="size" {s=$2} f && $1=="offset" {print $2, s; exit}')
  [ -n "$info" ] || return 2
  read -r offset size <<< "$info"
  frame=$(tail -c +$((offset + 1)) "$carrier" | head -c $((size)) | perl -0777 -ne 'my $i = index($_, "\x28\xb5\x2f\xfd"); print $i if $i >= 0')
  [ -n "$frame" ] || return 1
  # zstd exits non-zero on the trailing bytes after the frame; the output is still complete.
  tail -c +$((offset + frame + 1)) "$carrier" | zstd -dc > "$out" 2>/dev/null || true
  [ -s "$out" ]
}

# Declared module API version of the original file (before carrier unwrap): the constant returned by node_api_module_get_api_version_v1 (arm64 only).
declared_version() {
  local hex
  hex=$(otool -tV -p _node_api_module_get_api_version_v1 "$1" 2>/dev/null | awk '/mov[ \t]+w0, #/ { sub(/#/, "", $NF); print $NF; exit }' || true)
  [ -n "$hex" ] && echo $((hex))
  return 0
}

analyze() {
  local label=$1 pkg=$2 member_re=$3 tgz member dest file mode=nm init=legacy declared sha rc
  tgz=$(cd "$WORKDIR/tarballs" && npm pack "$pkg" --silent --ignore-scripts 2>/dev/null | tail -1) || true
  if [ -z "$tgz" ] || [ ! -f "$WORKDIR/tarballs/$tgz" ]; then
    echo "FAIL $label: npm pack $pkg" >&2
    return 1
  fi
  member=$(tar tzf "$WORKDIR/tarballs/$tgz" | { grep -E "$member_re" || true; } | head -1)
  if [ -z "$member" ]; then
    echo "FAIL $label: no tar member matches $member_re in $tgz" >&2
    return 1
  fi
  dest="$WORKDIR/extracted/$label"
  mkdir -p "$dest"
  tar xzf "$WORKDIR/tarballs/$tgz" -C "$dest" "$member"
  file="$dest/$member"
  declared=$(declared_version "$file")
  sha=$(shasum -a 256 "$file" | cut -c1-64)
  rc=0
  unwrap_swc_carrier "$file" "$dest/payload.bin" || rc=$?
  case $rc in
    0) file="$dest/payload.bin"; mode=nm+carrier ;;
    1) echo "FAIL $label: swc carrier found but payload unwrap failed" >&2; return 1 ;;
  esac
  nm -u "$file" 2>/dev/null | awk '{print $NF}' | normalize > "$WORKDIR/undefined/$label.txt"
  nm -gU "$file" 2>/dev/null | awk '{print $NF}' | normalize > "$WORKDIR/exports/$label.txt"
  # Non-Node-API host imports (libuv, V8, node internals) break dlopen just the same.
  nm -u "$file" 2>/dev/null | awk '{print $NF}' | { grep -E '^_{1,2}(uv_|ZN2v8|ZN4node)' || true; } | sort -u > "$WORKDIR/host-other/$label.txt"
  # No imported napi symbol: the addon may resolve them with dlsym, so fall back to the name table in .rodata.
  if [ ! -s "$WORKDIR/undefined/$label.txt" ] && [ "$label" != esbuild ]; then
    strings -a "$file" | normalize > "$WORKDIR/undefined/$label.txt"
    mode=strings
  fi
  if grep -qx napi_register_module_v1 "$WORKDIR/exports/$label.txt"; then init=v1
  elif [ ! -s "$WORKDIR/undefined/$label.txt" ]; then init=none
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$label" "$tgz" "$(basename "$member")" "$mode" "$init" "${declared:--}" \
    "$sha" "$(wc -l < "$WORKDIR/undefined/$label.txt" | tr -d ' ')" >> "$WORKDIR/packages.tsv"
}

# symbol -> minimum N-API version, from the "#if NAPI_VERSION >= N" guards of the Node headers.
fetch_symbol_versions() {
  local out=$WORKDIR/symbol-versions.tsv h
  : > "$out"
  for h in js_native_api.h node_api.h; do
    curl -fsSL "https://raw.githubusercontent.com/nodejs/node/$HEADERS_REF/src/$h" |
      awk '
        /^#ifdef NAPI_EXPERIMENTAL/ || /^#if +defined\(NAPI_EXPERIMENTAL\)/ { v = "exp" }
        /^#if +NAPI_VERSION *>= *[0-9]+/ { v = $NF }
        /^#endif/ { v = "" }
        match($0, /(napi|node_api)_[a-z0-9_]+\(/) { print substr($0, RSTART, RLENGTH - 1) "\t" (v == "" ? "1" : v) }
      ' >> "$out"
  done
  sort -u "$out" -o "$out"
}

: > "$WORKDIR/packages.tsv"
ok=0
for entry in "${PACKAGES[@]}"; do
  IFS='|' read -r label pkg member_re <<< "$entry"
  if analyze "$label" "$pkg" "$member_re"; then ok=$((ok + 1)); fi
done
fetch_symbol_versions

# symbol <TAB> count <TAB> packages <TAB> min N-API version ("?" if the header has no such symbol)
cd "$WORKDIR/undefined"
for f in *.txt; do sed "s/\$/\t${f%.txt}/" "$f"; done | sort |
  awk -F'\t' '{ c[$1]++; p[$1] = p[$1] (p[$1] ? "," : "") $2 } END { for (s in c) print s "\t" c[s] "\t" p[s] }' |
  sort -t$'\t' -k2,2nr -k1,1 |
  awk -F'\t' 'BEGIN { while ((getline l < ARGV[1]) > 0) { split(l, a, "\t"); v[a[1]] = a[2] } ARGV[1] = "" }
              { print $1 "\t" $2 "\t" $3 "\t" (($1 in v) ? v[$1] : "?") }' "$WORKDIR/symbol-versions.tsv" - > "$WORKDIR/summary.tsv"

echo "packages analyzed: $ok/${#PACKAGES[@]}"
echo "unique napi symbols: $(wc -l < "$WORKDIR/summary.tsv" | tr -d ' ')"
echo "non-napi host imports (uv_/v8/node): $(cat "$WORKDIR"/host-other/*.txt | wc -l | tr -d ' ')"
echo "results: $WORKDIR/summary.tsv (symbol, count, packages, min N-API)"
[ "$ok" -gt 0 ]
