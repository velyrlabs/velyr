#!/usr/bin/env bash
# Node compatibility harness (M7 acceptance, PLANS section 9, ADR-015).
#
#   bash scripts/node-compat.sh behavior [module...]   behavior tests of compat/node/behavior/
#   bash scripts/node-compat.sh packages [package...]  install + smoke of compat/node/packages.txt
#
# behavior: every compat/node/behavior/<module>/*.js runs under `velyr run` with minimal
#   permissions; a test passes when it exits 0. One JSON per module:
#   compat/results/node-behavior-<module>-<ts>.json
# packages: each package is installed (lifecycle scripts stay denied, nothing native) into its own
#   directory under $NODE_COMPAT_WORK, one at a time, then compat/node/smoke/<pkg>.mjs imports it
#   and exercises the main API. The package test suites are NOT run. One JSON per run:
#   compat/results/node-packages-<ts>.json. Registry failures are recorded, never fatal.
#
# Environment:
#   VELYR              velyr binary (default: the newest of target/release/velyr, target/debug/velyr)
#   NODE_COMPAT_WORK   work directory outside the repo (default: ${TMPDIR:-/tmp}/velyr-node-compat)
#   RSS_GUARD          path of a script `rss-guard.sh <cap-mb> <command...>` that kills the process
#                      group over the RSS cap; unset = no cap
#   GUARD_MB           RSS cap handed to the guard (default 1500)
#   TEST_TIMEOUT       seconds per test or smoke (default 60)
#   INSTALL_TIMEOUT    seconds per `velyr add` (default 180)
#
# Exit code: 2 on setup errors, else 0 (failing tests are results, not errors).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BEHAVIOR_DIR="$ROOT/compat/node/behavior"
SMOKE_DIR="$ROOT/compat/node/smoke"
PACKAGES="$ROOT/compat/node/packages.txt"
RESULTS_DIR="$ROOT/compat/results"
WORK="${NODE_COMPAT_WORK:-${TMPDIR:-/tmp}/velyr-node-compat}"
GUARD_MB="${GUARD_MB:-1500}"
TEST_TIMEOUT="${TEST_TIMEOUT:-60}"
INSTALL_TIMEOUT="${INSTALL_TIMEOUT:-180}"
TS="$(date +%s)"

die() { echo "error: $*" >&2; exit 2; }

VELYR="${VELYR:-}"
if [ -z "$VELYR" ]; then
  for candidate in "$ROOT/target/release/velyr" "$ROOT/target/debug/velyr"; do
    [ -x "$candidate" ] || continue
    if [ -z "$VELYR" ] || [ "$candidate" -nt "$VELYR" ]; then VELYR="$candidate"; fi
  done
fi
[ -x "$VELYR" ] || die "no velyr binary (cargo build -p velyr-cli, or set VELYR)"
command -v jq >/dev/null || die "jq is required"
command -v timeout >/dev/null || die "timeout (coreutils) is required"
case "$WORK" in "$ROOT"|"$ROOT"/*) die "NODE_COMPAT_WORK must be outside the repo" ;; esac
mkdir -p "$WORK" "$RESULTS_DIR"

# Runs a command under the RSS guard when one is configured; the guard kills only its own child.
guarded() {
  if [ -n "${RSS_GUARD:-}" ]; then bash "$RSS_GUARD" "$GUARD_MB" "$@"; else "$@"; fi
}

# First bytes of a log (the error message comes before the stack) as valid UTF-8 without
# control characters, for the JSON `error` field.
tail_log() { head -c 400 "$1" | iconv -c -f utf-8 -t utf-8 | LC_ALL=C tr -d '\000-\010\013-\037'; }

behavior() {
  local modules=("$@") module file name log code pass tmp ndjson
  if [ "${#modules[@]}" -eq 0 ]; then
    while IFS= read -r module; do modules+=("$module"); done < <(ls "$BEHAVIOR_DIR")
  fi
  tmp="$WORK/behavior-tmp"
  mkdir -p "$tmp" "$WORK/behavior-logs" "$WORK/home"
  for module in "${modules[@]}"; do
    [ -d "$BEHAVIOR_DIR/$module" ] || { echo "skip: no behavior tests for $module" >&2; continue; }
    ndjson="$WORK/behavior-logs/$module.ndjson"
    : > "$ndjson"
    for file in "$BEHAVIOR_DIR/$module"/*.js; do
      name="$(basename "$file" .js)"
      log="$WORK/behavior-logs/$module-$name.log"
      guarded timeout "$TEST_TIMEOUT" env -i HOME="$WORK/home" PATH="$PATH" COMPAT_TMP="$tmp" "$VELYR" run \
        "--allow-read=$tmp" "--allow-write=$tmp" --allow-env \
        --allow-net=127.0.0.1 --allow-listen=127.0.0.1 "$file" > "$log" 2>&1
      code=$?
      pass=false
      [ "$code" -eq 0 ] && pass=true
      jq -nc --arg name "$name" --argjson pass "$pass" --argjson exit "$code" \
        --arg error "$([ "$pass" = true ] || tail_log "$log")" \
        '{name: $name, pass: $pass, exit: $exit, error: $error}' >> "$ndjson"
      echo "$module/$name: $([ "$pass" = true ] && echo ok || echo "FAIL (exit $code)")"
    done
    jq -s --arg module "$module" --argjson ts "$TS" --arg velyr "$VELYR" \
      '{timestamp: $ts, module: $module, velyr: $velyr, total: length,
        passed: (map(select(.pass)) | length), tests: .}' \
      "$ndjson" > "$RESULTS_DIR/node-behavior-$module-$TS.json"
  done
}

# `@nestjs/core` -> `nestjs-core.mjs`, the name the matrix generator links to.
smoke_name() { local n="${1#@}"; echo "${n//\//-}.mjs"; }

install_and_smoke() {
  local name="$1" flag="$2" dir log version installed=false smoke=skip partial=false error=""
  dir="$WORK/pkgs/${name//\//__}"
  mkdir -p "$dir"
  printf '{"name":"smoke","version":"0.0.0","private":true,"type":"module"}\n' > "$dir/package.json"
  log="$dir/add.log"
  # Isolated HOME and cache: nothing of the user's npmrc or cache is touched.
  (cd "$dir" && HOME="$WORK/home" VELYR_NPM_CACHE_DIR="$WORK/cache" \
    guarded timeout "$INSTALL_TIMEOUT" "$VELYR" add "$name" > "$log" 2>&1)
  if [ -f "$dir/node_modules/$name/package.json" ]; then
    installed=true
    version="$(jq -r '.version // "-"' "$dir/node_modules/$name/package.json")"
  else
    version="-"
    error="install failed: $(tail_log "$log")"
  fi
  if [ "$installed" = true ]; then
    if [ ! -f "$SMOKE_DIR/$(smoke_name "$name")" ]; then
      error="no smoke script compat/node/smoke/$(smoke_name "$name")"
    else
      cp "$SMOKE_DIR/$(smoke_name "$name")" "$dir/smoke.mjs"
      log="$dir/smoke.log"
      # Packages read env at import time (NODE_ENV, DEBUG, PG*): the env is allowed but scrubbed.
      (cd "$dir" && guarded timeout "$TEST_TIMEOUT" env -i HOME="$WORK/home" PATH="$PATH" "$VELYR" run \
        "--allow-read=$dir" --allow-env --allow-net=127.0.0.1 --allow-listen=127.0.0.1 \
        smoke.mjs > "$log" 2>&1)
      if [ $? -eq 0 ] && grep -qx ok "$log"; then
        smoke=pass
        [ "$flag" = no-server ] && partial=true
      else
        smoke=fail
        error="smoke failed: $(tail_log "$log")"
      fi
    fi
  fi
  echo "$name@$version: install=$installed smoke=$smoke partial=$partial"
  jq -nc --arg name "$name" --arg version "$version" --argjson installed "$installed" \
    --arg smoke "$smoke" --argjson partial "$partial" --arg error "$error" \
    '{name: $name, version: $version, installed: $installed, smoke: $smoke,
      partial: $partial, error: $error}' >> "$WORK/packages.ndjson"
}

packages() {
  local only=("$@") name flag
  [ -f "$PACKAGES" ] || die "missing $PACKAGES"
  mkdir -p "$WORK/home"
  : > "$WORK/packages.ndjson"
  # One package at a time: a failing install or smoke must not hide the others.
  while read -r name flag _; do
    case "$name" in ''|'#'*) continue ;; esac
    if [ "${#only[@]}" -gt 0 ]; then
      case " ${only[*]} " in *" $name "*) ;; *) continue ;; esac
    fi
    install_and_smoke "$name" "${flag:-}"
  done < "$PACKAGES"
  jq -s --argjson ts "$TS" --arg velyr "$VELYR" \
    '{timestamp: $ts, velyr: $velyr,
      packages: (map({key: .name, value: del(.name)}) | from_entries)}' \
    "$WORK/packages.ndjson" > "$RESULTS_DIR/node-packages-$TS.json"
}

mode="${1:-}"
[ "$#" -gt 0 ] && shift
case "$mode" in
  behavior) behavior "$@" ;;
  packages) packages "$@" ;;
  *) die "usage: node-compat.sh behavior [module...] | packages [package...]" ;;
esac
