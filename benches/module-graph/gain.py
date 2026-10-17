#!/usr/bin/env python3
"""Gain gate for the module-graph suites: gain = 1 - new/base on one metric.

    python3 -I benches/module-graph/gain.py --base BASE.json --result-glob 'benches/results/module-graph-esm-*.json' \
        --metric instructions --min-gain 0.05

Exit 0 when gain >= min-gain, 1 when below, 2 when a file or metric is missing.
"""
import argparse
import glob
import json
import os
import sys
import tempfile
from pathlib import Path


def load_metric(path, metric):
    with open(path) as handle:
        return float(json.load(handle)["metrics"][metric])


def run(base, result_glob, metric, min_gain):
    matches = glob.glob(result_glob)
    if not matches or not os.path.isfile(base):
        print("error: base file or result glob matched nothing", file=sys.stderr)
        return 2
    newest = max(matches, key=os.path.getmtime)
    try:
        base_value, new_value = load_metric(base, metric), load_metric(newest, metric)
    except (KeyError, TypeError, ValueError, OSError) as error:
        print(f"error: cannot read metric {metric}: {error!r}", file=sys.stderr)
        return 2
    gain = 1 - new_value / base_value
    print(f"{metric}: base={base_value:g} new={new_value:g} ({newest}) gain={gain:.4f} min={min_gain}")
    return 0 if gain >= min_gain else 1


def selftest():
    with tempfile.TemporaryDirectory() as tmp:
        for name, value in (("base", 100), ("new", 70)):
            Path(tmp, f"{name}.json").write_text(json.dumps({"metrics": {"instructions": value}}))
        base, glob_ = f"{tmp}/base.json", f"{tmp}/new.json"
        cases = [(0.25, 0), (0.35, 1)]
        for min_gain, want in cases:
            got = run(base, glob_, "instructions", min_gain)
            assert got == want, (min_gain, got, want)
        assert run(base, f"{tmp}/none*.json", "instructions", 0.0) == 2
        assert run(base, glob_, "missing_metric", 0.0) == 2
    print("selftest ok")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--base")
    ap.add_argument("--result-glob")
    ap.add_argument("--metric", default="instructions")
    ap.add_argument("--min-gain", type=float, default=0.0)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        selftest()
    elif not (args.base and args.result_glob):
        ap.error("--base and --result-glob are required")
    else:
        sys.exit(run(args.base, args.result_glob, args.metric, args.min_gain))
