#!/usr/bin/env bash
# Acceptance runner for compat/acceptance/<name> fixtures.
# Usage: scripts/acceptance.sh react-vite
# react-vite: `velyr install --frozen`, `velyr x vite build` (checks dist/), then the dev server
# with an HTTP request to the index and to one transformed module. Runs on a copy of the fixture.
# Exit code: 0 green, 1 a step failed (blocker named on stderr), 2 setup error.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAME="${1:-}"
FIXTURE="$ROOT/compat/acceptance/$NAME"
[ -n "$NAME" ] && [ -d "$FIXTURE" ] || { echo "usage: $0 <fixture> (one of: $(ls "$ROOT/compat/acceptance" | tr '\n' ' '))" >&2; exit 2; }
[ "$NAME" = "react-vite" ] || { echo "error: no runner for fixture '$NAME'" >&2; exit 2; }

VELYR="${VELYR:-}"
if [ -z "$VELYR" ]; then
  for candidate in "$ROOT/target/release/velyr" "$ROOT/target/debug/velyr"; do
    [ -x "$candidate" ] && VELYR="$candidate" && break
  done
fi
[ -x "$VELYR" ] || { echo "error: no velyr binary (cargo build --release -p velyr-cli, or set VELYR)" >&2; exit 2; }
command -v curl >/dev/null || { echo "error: curl is required" >&2; exit 2; }

WORK="$(mktemp -d)"
DEV_PID=""
cleanup() {
  if [ -n "$DEV_PID" ]; then
    kill "$DEV_PID" 2>/dev/null
    wait "$DEV_PID" 2>/dev/null
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT
APP="$WORK/app"
cp -R "$FIXTURE" "$APP"
cd "$APP" || exit 2
export HOME="$WORK/home"
mkdir -p "$HOME"
export VELYR_NPM_CACHE_DIR="$WORK/cache"
PORT="${ACCEPTANCE_PORT:-$((20000 + RANDOM % 20000))}"
FLAGS=(--allow-read --allow-write --allow-env --allow-run)

fail() {
  echo "FAIL [$1]: $2" >&2
  echo "--- last lines of $3 ---" >&2
  tail -n 15 "$3" >&2
  # Name the known blockers so a red run says why.
  if grep -qiE 'napi|rolldown|\.node\b|--allow-native' "$3"; then
    echo "BLOCKER: native N-API addon required (VELYR-96, crates/compat-napi, M9 --allow-native)" >&2
  fi
  if grep -qE 'node:[a-z_/]+$' "$3" && grep -qE 'not linked|not supported yet' "$3"; then
    echo "BLOCKER: missing Node builtin in crates/compat-node: $(grep -oE 'node:[a-z_/]+$' "$3" | head -1)" >&2
  fi
  echo "See compat/acceptance/$NAME/README.md" >&2
  exit 1
}

echo "== install --frozen"
"$VELYR" install --frozen >"$WORK/install.log" 2>&1 || fail install "velyr install --frozen" "$WORK/install.log"

echo "== x vite build"
"$VELYR" x "${FLAGS[@]}" vite build >"$WORK/build.log" 2>&1 || fail build "velyr x vite build" "$WORK/build.log"
[ -f dist/index.html ] || fail build "dist/index.html missing" "$WORK/build.log"
ls dist/assets/*.js >/dev/null 2>&1 || fail build "no JS bundle in dist/assets" "$WORK/build.log"
grep -q '<div id="root">' dist/index.html || fail build "dist/index.html lacks #root" "$WORK/build.log"

echo "== x vite (dev server on :$PORT)"
"$VELYR" x "${FLAGS[@]}" --allow-listen="127.0.0.1:$PORT" --allow-net vite --host 127.0.0.1 --port "$PORT" --strictPort \
  >"$WORK/dev.log" 2>&1 &
DEV_PID=$!
for _ in $(seq 1 60); do
  curl -fs "http://127.0.0.1:$PORT/" >"$WORK/index.html" 2>/dev/null && break
  kill -0 "$DEV_PID" 2>/dev/null || fail dev "dev server exited early" "$WORK/dev.log"
  sleep 0.5
done
[ -s "$WORK/index.html" ] || fail dev "no response from the index" "$WORK/dev.log"
grep -q '/src/main.tsx' "$WORK/index.html" || fail dev "index lacks the entry script" "$WORK/dev.log"
curl -fs "http://127.0.0.1:$PORT/src/main.tsx" >"$WORK/main.js" 2>/dev/null || fail dev "GET /src/main.tsx" "$WORK/dev.log"
# A transformed module has no TypeScript/JSX left and imports the pre-bundled React.
if ! grep -q 'react' "$WORK/main.js" || grep -q '<App' "$WORK/main.js"; then
  fail dev "/src/main.tsx not transformed" "$WORK/dev.log"
fi

echo "PASS react-vite (esbuild + rollup 3 JS)"
