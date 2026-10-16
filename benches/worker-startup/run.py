#!/usr/bin/env python3
"""Worker startup and RSS per worker, with and without --experimental-shared-code-cache (VELYR-60).

    VELYR_BIN=target/release/velyr python3 benches/worker-startup/run.py [--modules 100]
        [--workers 8] [--repeats 5]

Generates a graph of --modules ES modules in a temp dir; the entry spawns --workers module workers
one after the other (all stay alive), each importing the whole graph, and prints the startup of
each (spawn until its first message). RSS per worker is the slope of the process peak RSS between
1 and --workers workers (`/usr/bin/time -l`, macOS; `-v` on Linux). Writes
benches/results/worker-startup-detail-<ts>.json. Numbers from a shared machine are indicative.
"""
import argparse
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

MAIN = """\
const count = Number(Deno_count());
const workers = [];
const startups = [];
for (let i = 0; i < count; i++) {
  const spawned = performance.now();
  const worker = new Worker(new URL("./worker.mjs", import.meta.url), { type: "module" });
  await new Promise((resolve) => (worker.onmessage = (event) => resolve(event.data)));
  startups.push(performance.now() - spawned);
  workers.push(worker);
}
console.log(JSON.stringify(startups));
for (const worker of workers) worker.terminate();
""".replace("Deno_count()", "{count}")

WORKER = """\
import { f0_0 } from "./m0.mjs";
// A timer: the isolate publishes its code cache before the first loop turn, not after it.
setTimeout(() => postMessage(f0_0(1)), 0);
"""


def module_source(i, total):
    lines = [f'import {{ f{c}_0 }} from "./m{c}.mjs";' for c in (2 * i + 1, 2 * i + 2) if c < total]
    for k in range(24):
        callee = f"f{2 * i + 1}_0(x)" if k == 0 and 2 * i + 1 < total else "x"
        lines.append(
            f"export function f{i}_{k}(x) {{\n"
            f"  const items = [x, x + {k}, x * {i + 1}];\n"
            f"  let total = {callee};\n"
            f"  for (const item of items) {{ if (item % 2 === 0) total += item; else total -= {k}; }}\n"
            f"  return total > 0 ? total : -total;\n}}"
        )
    return "\n".join(lines) + "\n"


def generate(directory, modules, count):
    for i in range(modules):
        (directory / f"m{i}.mjs").write_text(module_source(i, modules))
    (directory / "worker.mjs").write_text(WORKER)
    (directory / "main.mjs").write_text(MAIN.replace("{count}", str(count)))


def peak_rss_kb(argv, cwd):
    flag = "-l" if sys.platform == "darwin" else "-v"
    done = subprocess.run(["/usr/bin/time", flag, *argv], cwd=cwd, capture_output=True, text=True, timeout=300)
    if done.returncode != 0:
        sys.exit(f"{argv} failed:\n{done.stderr}")
    if sys.platform == "darwin":
        rss = int(re.search(r"(\d+)\s+maximum resident set size", done.stderr).group(1)) // 1024
    else:
        rss = int(re.search(r"Maximum resident set size \(kbytes\): (\d+)", done.stderr).group(1))
    return json.loads(done.stdout.strip().splitlines()[-1]), rss


def measure(velyr, directory, cache, count):
    flags = ["--experimental-shared-code-cache"] if cache else []
    return peak_rss_kb([velyr, *flags, "run", str(directory / "main.mjs")], directory)


def summarize(values):
    return {"median": statistics.median(values), "min": min(values), "max": max(values)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--modules", type=int, default=100)
    parser.add_argument("--workers", type=int, default=8)
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--out-dir", default=str(ROOT / "benches/results"))
    args = parser.parse_args()
    velyr = os.environ.get("VELYR_BIN", str(ROOT / "target/release/velyr"))
    version = subprocess.run([velyr, "--version"], capture_output=True, text=True).stdout.strip()
    result = {"modules": args.modules, "workers": args.workers, "repeats": args.repeats,
              "velyr": version, "platform": f"{platform.system()}-{platform.machine()}",
              "unix_time": int(time.time()), "runs": {}}
    with tempfile.TemporaryDirectory() as tmp:
        one, many = Path(tmp) / "one", Path(tmp) / "many"
        for directory, count in ((one, 1), (many, args.workers)):
            directory.mkdir()
            generate(directory, args.modules, count)
        for cache in (False, True):
            first, later, rss_one, rss_many = [], [], [], []
            for _ in range(args.repeats):
                startups, rss = measure(velyr, one, cache, 1)
                rss_one.append(rss)
                startups, rss = measure(velyr, many, cache, args.workers)
                rss_many.append(rss)
                first.append(startups[0])
                later.extend(startups[1:])
            slopes = [(m - o) / (args.workers - 1) for o, m in zip(rss_one, rss_many)]
            result["runs"]["cache" if cache else "baseline"] = {
                "first_worker_startup_ms": summarize(first),
                "later_worker_startup_ms": summarize(later),
                "peak_rss_kb_1_worker": summarize(rss_one),
                "peak_rss_kb_n_workers": summarize(rss_many),
                "rss_per_extra_worker_kb": summarize(slopes),
            }
    out = Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    path = out / f"worker-startup-detail-{result['unix_time']}.json"
    path.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result["runs"], indent=2))
    print(path)


if __name__ == "__main__":
    main()
