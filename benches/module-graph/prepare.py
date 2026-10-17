#!/usr/bin/env python3
"""Generates the two module-graph bench fixtures under target/bench-fixtures (not committed).

    python3 -I benches/module-graph/prepare.py

ESM: 40 groups x 50 modules, all `.js` (2041 `.js` under src/). CJS: 30 x 50, every module `.cjs`
(1500 `.cjs` + 31 `.js`). Idempotent: skips a fixture whose on-disk hash matches `gen.py --check`.
"""
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GEN = ROOT / "benches/large-app/gen.py"
FIXTURES = ROOT / "target/bench-fixtures"
# name -> (gen.py args, glob, expected file count)
SPECS = {
    "module-graph-esm": (["--groups", "40", "--per-group", "50", "--cjs-every", "0"], "src/**/*.js", 2041),
    "module-graph-cjs": (["--groups", "30", "--per-group", "50", "--cjs-every", "1"], "src/**/*.cjs", 1500),
}


def gen(out, args, *flags):
    cmd = [sys.executable, "-I", str(GEN), "--out", str(out), *args, *flags]
    return subprocess.run(cmd, check=True, capture_output=True, text=True).stdout


def prepare(name, args, pattern, expected):
    out = FIXTURES / name
    if "on disk: matches" in gen(out, args, "--check"):
        print(f"{name}: up to date")
    else:
        print(f"{name}: {gen(out, args).strip()}")
    found = len(list(out.glob(pattern)))
    if found != expected:
        sys.exit(f"error: {name} has {found} files matching {pattern}, expected {expected}")


if __name__ == "__main__":
    for name, spec in SPECS.items():
        prepare(name, *spec)
