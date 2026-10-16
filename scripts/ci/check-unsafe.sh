#!/usr/bin/env bash
# Fails if a crate outside unsafe-allowlist.txt lacks `[lints] workspace = true` (ADR-011).
# Usage: check-unsafe.sh [repo-root]   (or VELYR_ROOT env; defaults to the repo containing this script)
set -euo pipefail

root="${1:-${VELYR_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}}"
allowlist="$root/unsafe-allowlist.txt"
[ -f "$allowlist" ] || { echo "error: $allowlist not found" >&2; exit 2; }

status=0
for manifest in "$root"/crates/*/Cargo.toml; do
  crate="$(basename "$(dirname "$manifest")")"
  grep -qxF "$crate" <(sed -e 's/#.*//' -e 's/[[:space:]]*$//' "$allowlist") && continue
  if ! awk '
    /^[[:space:]]*\[/ { in_lints = ($0 ~ /^[[:space:]]*\[lints\][[:space:]]*(#.*)?$/); next }
    in_lints && /^[[:space:]]*workspace[[:space:]]*=[[:space:]]*true[[:space:]]*(#.*)?$/ { found = 1 }
    END { exit !found }
  ' "$manifest"; then
    echo "error: crates/$crate must set [lints] workspace = true (or be added to unsafe-allowlist.txt)" >&2
    status=1
  fi
done
[ $status -eq 0 ] && echo "check-unsafe: ok"
exit $status
