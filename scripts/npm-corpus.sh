#!/usr/bin/env bash
# M6 acceptance: install a curated npm corpus with real network access and check that
# node_modules is reproducible. Steps, all inside one mktemp -d:
#   1. `velyr add` each package of compat/npm/corpus.txt, hash node_modules (paths + content);
#   2. smoke-import each package with `velyr run`;
#   3. delete node_modules and the cache, `velyr install --frozen`, hash again (cold cache);
#   4. delete node_modules only, `velyr install --frozen`, hash again (warm cache).
# Output: compat/results/npm-corpus-<ts>.json and compat/npm/passing.txt.
# Exit code: 1 when a whole-tree hash differs between runs, 2 on setup errors, else 0.
# Packages that fail because of runtime bugs are recorded with their error, never a blocker.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORPUS="$ROOT/compat/npm/corpus.txt"
PASSING="$ROOT/compat/npm/passing.txt"
RESULTS_DIR="$ROOT/compat/results"
TS="$(date +%s)"
SMOKE_TIMEOUT="${SMOKE_TIMEOUT:-60}"

VELYR="${VELYR:-}"
if [ -z "$VELYR" ]; then
  for candidate in "$ROOT/target/release/velyr" "$ROOT/target/debug/velyr"; do
    [ -x "$candidate" ] && VELYR="$candidate" && break
  done
fi
[ -x "$VELYR" ] || { echo "error: no velyr binary (cargo build --release -p velyr-cli, or set VELYR)" >&2; exit 2; }
command -v python3 >/dev/null || { echo "error: python3 is required" >&2; exit 2; }
[ -f "$CORPUS" ] || { echo "error: missing $CORPUS" >&2; exit 2; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
APP="$WORK/app"
LOGS="$WORK/logs"
mkdir -p "$APP" "$LOGS" "$WORK/home" "$RESULTS_DIR"
# Nothing of the user's HOME, npmrc or cache is touched.
export HOME="$WORK/home"
export VELYR_NPM_CACHE_DIR="$WORK/cache"
printf '{"name":"npm-corpus","version":"0.0.0","private":true}\n' > "$APP/package.json"

# Hashes node_modules: relative path + kind + content (symlinks by target). Prints
# {"tree": <sha256>, "packages": {<name>: <sha256>}}. Velyr bookkeeping files are skipped.
HASHER="$WORK/hash_tree.py"
cat > "$HASHER" <<'PY'
import hashlib, json, os, sys

root = sys.argv[1]
names = sys.argv[2:]
SKIP = {".velyr-state.json", ".velyr-scripts.json"}

def walk(base):
    for dirpath, dirnames, filenames in os.walk(base, followlinks=False):
        dirnames.sort()
        rel_dir = os.path.relpath(dirpath, root)
        for name in sorted(filenames + [d for d in dirnames if os.path.islink(os.path.join(dirpath, d))]):
            if rel_dir == "." and name in SKIP:
                continue
            yield os.path.join(dirpath, name)

def digest(paths):
    h = hashlib.sha256()
    for path in paths:
        rel = os.path.relpath(path, root).replace(os.sep, "/")
        if os.path.islink(path):
            h.update(f"L {rel} -> {os.readlink(path)}\n".encode())
        elif os.path.isfile(path):
            h.update(f"F {rel} {os.stat(path).st_mode & 0o111 != 0}\n".encode())
            with open(path, "rb") as f:
                for chunk in iter(lambda: f.read(1 << 20), b""):
                    h.update(chunk)
        else:
            h.update(f"? {rel}\n".encode())
    return h.hexdigest()

if not os.path.isdir(root):
    print(json.dumps({"tree": "missing", "packages": {n: "missing" for n in names}}))
    sys.exit(0)
everything = sorted(set(walk(root)))
result = {"tree": digest(everything), "packages": {}}
for n in names:
    base = os.path.join(root, n)
    result["packages"][n] = digest(sorted(set(walk(base)))) if os.path.isdir(base) else "missing"
print(json.dumps(result))
PY

NAMES=()
SPECS=()
IMPORTS=()
while read -r name spec _; do
  case "$name" in ''|'#'*) continue ;; esac
  NAMES+=("$name")
  IMPORTS+=("${spec:-$name}")
done < "$CORPUS"
TOTAL="${#NAMES[@]}"
echo "corpus: $TOTAL packages, binary: $VELYR"

hash_tree() { python3 -I "$HASHER" "$APP/node_modules" "${NAMES[@]}" > "$1"; }

# 1. add one by one: a failing package must not hide the others.
ADD_STATUS="$WORK/add.tsv"
: > "$ADD_STATUS"
(cd "$APP" && for name in "${NAMES[@]}"; do
  log="$LOGS/add-${name//\//__}.log"
  "$VELYR" add "$name" > "$log" 2>&1
  printf '%s\t%s\n' "$name" "$?" >> "$ADD_STATUS"
done)
hash_tree "$WORK/hash-initial.json"

# 2. smoke import of each package that was added.
SMOKE_STATUS="$WORK/smoke.tsv"
: > "$SMOKE_STATUS"
(cd "$APP" && for i in "${!NAMES[@]}"; do
  name="${NAMES[$i]}"
  log="$LOGS/smoke-${name//\//__}.log"
  printf 'import * as m from "%s";\nif (Object.keys(m).length === 0) throw new Error("no exports");\nconsole.log("ok");\n' "${IMPORTS[$i]}" > smoke.mjs
  timeout "$SMOKE_TIMEOUT" "$VELYR" run -A smoke.mjs > "$log" 2>&1
  printf '%s\t%s\n' "$name" "$?" >> "$SMOKE_STATUS"
done)

# 3. cold cache: node_modules and cache gone, velyr.lock kept.
rm -rf "$APP/node_modules" "$VELYR_NPM_CACHE_DIR"
(cd "$APP" && "$VELYR" install --frozen > "$LOGS/install-cold.log" 2>&1); COLD_STATUS=$?
hash_tree "$WORK/hash-cold.json"

# 4. warm cache: only node_modules gone.
rm -rf "$APP/node_modules"
(cd "$APP" && "$VELYR" install --frozen > "$LOGS/install-warm.log" 2>&1); WARM_STATUS=$?
hash_tree "$WORK/hash-warm.json"

OUT="$RESULTS_DIR/npm-corpus-$TS.json"
python3 -I - "$WORK" "$OUT" "$PASSING" "$VELYR" "$TS" "$COLD_STATUS" "$WARM_STATUS" <<'PY'
import json, os, sys

work, out, passing, velyr, ts, cold_status, warm_status = sys.argv[1:8]

def table(name):
    rows = {}
    with open(os.path.join(work, name)) as f:
        for line in f:
            key, code = line.rstrip("\n").split("\t")
            rows[key] = int(code)
    return rows

def load(name):
    with open(os.path.join(work, name)) as f:
        return json.load(f)

def tail(path, limit=6):
    try:
        with open(path, errors="replace") as f:
            lines = [l.rstrip() for l in f if l.strip()]
    except OSError:
        return ""
    return "\n".join(lines[-limit:])

adds, smokes = table("add.tsv"), table("smoke.tsv")
initial, cold, warm = load("hash-initial.json"), load("hash-cold.json"), load("hash-warm.json")
results, passed = {}, []
for name in adds:
    safe = name.replace("/", "__")
    reasons = []
    if adds[name] != 0:
        reasons.append({"stage": "add", "exit": adds[name], "error": tail(f"{work}/logs/add-{safe}.log")})
    elif smokes.get(name, 1) != 0:
        reasons.append({"stage": "smoke", "exit": smokes.get(name), "error": tail(f"{work}/logs/smoke-{safe}.log")})
    if adds[name] == 0:
        h0 = initial["packages"][name]
        if cold["packages"][name] != h0:
            reasons.append({"stage": "cold-hash", "error": f"{cold['packages'][name][:12]} != {h0[:12]}"})
        if warm["packages"][name] != h0:
            reasons.append({"stage": "warm-hash", "error": f"{warm['packages'][name][:12]} != {h0[:12]}"})
    results[name] = {"pass": not reasons, "failures": reasons}
    if not reasons:
        passed.append(name)

tree_equal_cold = cold["tree"] == initial["tree"]
tree_equal_warm = warm["tree"] == initial["tree"]
report = {
    "timestamp": int(ts),
    "binary": os.path.relpath(velyr, os.getcwd()) if velyr.startswith(os.getcwd()) else velyr,
    "total": len(adds),
    "passed": len(passed),
    "tree_hash": initial["tree"],
    "cold": {"install_exit": int(cold_status), "tree_hash": cold["tree"], "equal": tree_equal_cold,
             "error": tail(f"{work}/logs/install-cold.log") if int(cold_status) else ""},
    "warm": {"install_exit": int(warm_status), "tree_hash": warm["tree"], "equal": tree_equal_warm,
             "error": tail(f"{work}/logs/install-warm.log") if int(warm_status) else ""},
    "packages": results,
}
with open(out, "w") as f:
    json.dump(report, f, indent=2)
    f.write("\n")
with open(passing, "w") as f:
    f.write("".join(f"{n}\n" for n in sorted(passed)))
print(f"pass {len(passed)}/{len(adds)}")
print(f"tree hash initial={initial['tree'][:16]} cold={cold['tree'][:16]} warm={warm['tree'][:16]}")
print(f"cold cache equal: {tree_equal_cold}; warm cache equal: {tree_equal_warm}")
for name, r in results.items():
    for fail in r["failures"]:
        first = (fail.get("error") or "").splitlines()
        print(f"FAIL {name} [{fail['stage']}] {first[-1] if first else ''}")
print(f"results: {out}")
sys.exit(0 if tree_equal_cold and tree_equal_warm else 1)
PY
exit $?
