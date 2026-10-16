#!/usr/bin/env bash
# Fails if velyr-runtime-core (normal deps) pulls in web/compat/tooling/io crates (ADR-011).
# Usage: check-deps.sh [repo-root]   (or VELYR_ROOT env; defaults to the repo containing this script)
set -euo pipefail

root="${1:-${VELYR_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}}"

tree="$(cargo tree --manifest-path "$root/Cargo.toml" -p velyr-runtime-core -e normal --prefix none)"

if violations="$(grep -E 'velyr-(web|compat|tooling|io)-' <<<"$tree")"; then
  echo "error: velyr-runtime-core must not depend on web/compat/tooling/io crates:" >&2
  sort -u <<<"$violations" >&2
  exit 1
fi
echo "check-deps: ok"
