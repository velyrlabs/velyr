#!/usr/bin/env bash
# Acceptance runner for compat/acceptance/<name> fixtures.
# Usage: scripts/acceptance.sh react-vite | react-ssr | vue-vite | vue-ssr
# react-vite: `velyr install --frozen`, `velyr x vite build` (checks dist/), then the dev server
# with an HTTP request to the index and to one transformed module, then an HMR check over WebSocket. Runs on a copy of the fixture.
# vue-vite: same steps as react-vite, with @vitejs/plugin-vue (SFC compiled by @vue/compiler-sfc inside Vite).
# vue-ssr: same as react-ssr, with createSSRApp + renderToWebStream.
# react-ssr: `velyr install --frozen`, then driver.py checks the chunked response (shell before the
# suspended content) against `velyr serve`. VELYR=<profile-web binary> covers "no node profile"; that binary
# has no installer, so VELYR_INSTALLER (default: $VELYR) names the binary that runs `install`.
# Exit code: 0 green, 1 a step failed (blocker named on stderr), 2 setup error.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAME="${1:-}"
FIXTURE="$ROOT/compat/acceptance/$NAME"
[ -n "$NAME" ] && [ -d "$FIXTURE" ] || { echo "usage: $0 <fixture> (one of: $(ls "$ROOT/compat/acceptance" | tr '\n' ' '))" >&2; exit 2; }
case "$NAME" in react-vite | react-ssr | vue-vite | vue-ssr) ;; *) { echo "error: no runner for fixture '$NAME'" >&2; exit 2; } ;; esac

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
FLAGS=(--allow-read --allow-write --allow-env)

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
"${VELYR_INSTALLER:-$VELYR}" install --frozen >"$WORK/install.log" 2>&1 || fail install "velyr install --frozen" "$WORK/install.log"

if [ "$NAME" = "react-ssr" ] || [ "$NAME" = "vue-ssr" ]; then
  command -v python3 >/dev/null || { echo "error: python3 is required" >&2; exit 2; }
  echo "== serve (chunked SSR, shell before suspended content)"
  python3 -I "$APP/driver.py" check --velyr "$VELYR" --dir "$APP" >"$WORK/check.log" 2>&1 || fail serve "driver.py check" "$WORK/check.log"
  cat "$WORK/check.log"
  echo "PASS $NAME"
  exit 0
fi

# Per-fixture expectations for the vite cases.
if [ "$NAME" = "vue-vite" ]; then
  ENTRY=/src/main.ts ROOT_ID=app MAIN_MARK=vue HMR_FILE=src/App.vue HMR_SUFFIX=/App.vue
  HMR_PROBE=$'\n<script lang="ts">export const hmrProbe = 1</script>\n'
else
  ENTRY=/src/main.tsx ROOT_ID=root MAIN_MARK=react HMR_FILE=src/App.tsx HMR_SUFFIX=/App.tsx
  HMR_PROBE=$'\n// hmr-probe\n'
fi

# vite spawns only the esbuild binary: scope --allow-run to it instead of any program.
ESBUILD_BIN="$(ls "$APP"/node_modules/@esbuild/*/bin/esbuild 2>/dev/null | head -1)"
# Physical path: macOS $TMPDIR is a symlink and the permission check sees the resolved one.
[ -n "$ESBUILD_BIN" ] && ESBUILD_BIN="$(cd "$(dirname "$ESBUILD_BIN")" && pwd -P)/esbuild"
[ -x "$ESBUILD_BIN" ] || fail install "esbuild binary not found under node_modules/@esbuild" "$WORK/install.log"
FLAGS+=("--allow-run=$ESBUILD_BIN")

echo "== x vite build"
"$VELYR" x "${FLAGS[@]}" vite build >"$WORK/build.log" 2>&1 || fail build "velyr x vite build" "$WORK/build.log"
[ -f dist/index.html ] || fail build "dist/index.html missing" "$WORK/build.log"
ls dist/assets/*.js >/dev/null 2>&1 || fail build "no JS bundle in dist/assets" "$WORK/build.log"
grep -q "<div id=\"$ROOT_ID\">" dist/index.html || fail build "dist/index.html lacks #$ROOT_ID" "$WORK/build.log"

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
grep -q "$ENTRY" "$WORK/index.html" || fail dev "index lacks the entry script" "$WORK/dev.log"
curl -fs "http://127.0.0.1:$PORT$ENTRY" >"$WORK/main.js" 2>/dev/null || fail dev "GET $ENTRY" "$WORK/dev.log"
# A transformed module has no TypeScript/JSX left and imports the pre-bundled React.
if ! grep -q "$MAIN_MARK" "$WORK/main.js" || grep -q '<App' "$WORK/main.js"; then
  fail dev "$ENTRY not transformed" "$WORK/dev.log"
fi
if [ "$NAME" = "vue-vite" ]; then
  # The SFC must come back compiled by @vue/compiler-sfc: a component object, no raw blocks.
  curl -fs "http://127.0.0.1:$PORT/src/App.vue" >"$WORK/app.js" 2>/dev/null || fail dev "GET /src/App.vue" "$WORK/dev.log"
  if ! grep -q '_sfc_main' "$WORK/app.js" || grep -q '<template' "$WORK/app.js"; then
    fail dev "/src/App.vue not compiled" "$WORK/dev.log"
  fi
fi

echo "== x vite HMR (edit $HMR_FILE, expect update within 5s)"
python3 -I - "$PORT" "$APP/$HMR_FILE" "$HMR_SUFFIX" "$HMR_PROBE" >"$WORK/hmr.log" 2>&1 <<'PY' || fail hmr "no HMR update for $HMR_FILE" "$WORK/dev.log"
import base64, json, os, socket, sys, time
port, path, suffix, probe = int(sys.argv[1]), sys.argv[2], sys.argv[3], sys.argv[4]
sock = socket.create_connection(("127.0.0.1", port), timeout=5)
key = base64.b64encode(os.urandom(16)).decode()
sock.sendall(("GET / HTTP/1.1\r\nHost: 127.0.0.1:%d\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
              "Sec-WebSocket-Key: %s\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: vite-hmr\r\n\r\n" % (port, key)).encode())
buf = b""
while b"\r\n\r\n" not in buf:
    chunk = sock.recv(4096)
    if not chunk: sys.exit("closed during handshake")
    buf += chunk
head, buf = buf.split(b"\r\n\r\n", 1)
if b" 101 " not in head.split(b"\r\n")[0]: sys.exit("handshake rejected: %r" % head)

def read(n):
    global buf
    while len(buf) < n:
        chunk = sock.recv(4096)
        if not chunk: sys.exit("socket closed")
        buf += chunk
    out, buf = buf[:n], buf[n:]
    return out

def message():
    # Server frames are unmasked; Vite sends single-frame text messages.
    b0, b1 = read(2)
    n = b1 & 0x7F
    if n == 126: n = int.from_bytes(read(2), "big")
    elif n == 127: n = int.from_bytes(read(8), "big")
    return b0 & 0x0F, read(n)

def next_json(deadline):
    while time.time() < deadline:
        op, data = message()
        if op == 1: return json.loads(data)
    sys.exit("timeout")

assert next_json(time.time() + 5)["type"] == "connected"
with open(path, "a") as f: f.write(probe)
start = time.time()
while True:
    msg = next_json(start + 5)
    names = [u.get("path", "") for u in msg.get("updates", [])]
    if msg["type"] == "full-reload" or (msg["type"] == "update" and any(n.endswith(suffix) for n in names)):
        print("HMR %s %s in %.0f ms" % (msg["type"], names, (time.time() - start) * 1000))
        break
PY
cat "$WORK/hmr.log"

echo "PASS $NAME (esbuild + rollup 3 JS)"
