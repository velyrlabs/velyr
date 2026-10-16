#!/usr/bin/env bash
# Checks scripts/install.sh against a local fake release (no network, no build).
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

target=x86_64-unknown-linux-gnu
file="velyr-$target.tar.gz"
rel="$work/release"
mkdir -p "$rel" "$work/pkg"

printf '#!/bin/sh\necho fake-velyr\n' > "$work/pkg/velyr"
chmod 755 "$work/pkg/velyr"
tar -czf "$rel/$file" -C "$work/pkg" velyr

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$rel" && sha256sum "$file" > SHA256SUMS)
else
  (cd "$rel" && shasum -a 256 "$file" > SHA256SUMS)
fi

run_install() {
  VELYR_DOWNLOAD_URL="file://$rel" VELYR_TARGET="$target" VELYR_INSTALL="$1" \
    sh "$root/scripts/install.sh"
}

fail() { echo "FAIL: $*" >&2; exit 1; }

# Good checksum installs a working binary.
run_install "$work/ok" >/dev/null || fail "valid checksum was rejected"
[ "$("$work/ok/velyr")" = "fake-velyr" ] || fail "installed binary does not run"

# Wrong checksum fails and installs nothing.
printf '%064d  %s\n' 0 "$file" > "$rel/SHA256SUMS"
if run_install "$work/bad" >/dev/null 2>"$work/err"; then fail "wrong checksum was accepted"; fi
grep -q 'checksum mismatch' "$work/err" || fail "missing mismatch message"
[ ! -e "$work/bad/velyr" ] || fail "binary installed despite mismatch"

# Missing checksum entry fails.
: > "$rel/SHA256SUMS"
if run_install "$work/missing" >/dev/null 2>&1; then fail "missing entry was accepted"; fi

echo "install.sh checks passed"
