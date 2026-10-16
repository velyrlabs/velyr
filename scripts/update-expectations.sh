#!/usr/bin/env bash
# Rewrites compat/test262/expectations/<engine>.txt from a fresh Test262 run (ADR-015).
#
#   scripts/update-expectations.sh native                      # whole suite (about 3 minutes)
#   scripts/update-expectations.sh native test/built-ins/Array # only these paths
#
# Entries outside the given paths are kept as they are, so a partial run is safe. A passing
# test leaves the manifest, a new failure is recorded with its reason. Commit the result in
# the same PR as the change that moved the numbers (it is how an unexpected pass is accepted).
set -euo pipefail

usage() { echo "usage: $0 <v8|native> [test262 path ...]" >&2; exit 2; }

[ $# -ge 1 ] || usage
ENGINE="$1"
shift
case "$ENGINE" in v8 | native) ;; *) usage ;; esac

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
if [ ! -d compat/vendor/test262/harness ]; then
  echo "error: compat/vendor/test262 is missing, run scripts/fetch-test262.sh" >&2
  exit 2
fi
[ $# -ge 1 ] || set -- test

cargo run --release -p velyr-qa-test262 -- --engine "$ENGINE" --update-expectations "$@"
echo "updated compat/test262/expectations/$ENGINE.txt"
