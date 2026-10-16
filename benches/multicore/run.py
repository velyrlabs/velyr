#!/usr/bin/env python3
"""Multicore HTTP benchmark (VELYR-61): `velyr serve --workers N` for N in 1, 2, 4, 8 (and 16 when
the machine has the cores), with and without --experimental-shared-code-cache, loaded by `oha`.
Reuses the helpers of benches/http/run.py.

    VELYR_BIN=/abs/path/velyr python3 benches/multicore/run.py [--duration 6] [--scenarios plaintext stream]

Per configuration: req/s, p99, idle RSS (listening, before load), peak RSS under load, CPU
(process cpu seconds / wall seconds, 100 = one core). RSS is of the whole process: the workers are
threads. Per-worker RSS is the slope against the 1-worker run (idle and loaded) and total / N.
Connection distribution is the platform's: handoff (acceptor) on macOS, SO_REUSEPORT on Linux; the
file says which one ran. Writes benches/baselines/m5-multicore-<scenario>-<platform>.json.
oha runs on the same machine and competes for its cores; numbers are indicative, not a gate.
"""
import argparse
import importlib.util
import json
import os
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("http_run", ROOT / "benches/http/run.py")
http = importlib.util.module_from_spec(spec)
spec.loader.exec_module(http)


def cpu_seconds(pid):
    text = http.out(["ps", "-o", "time=", "-p", str(pid)]) or "0:00.00"
    total = 0.0
    for part in text.strip().replace("-", ":").split(":"):
        total = total * 60 + float(part)
    return total


def one(velyr, workers, cache, scenario, seconds):
    port = http.free_port()
    argv = [velyr] + (["--experimental-shared-code-cache"] if cache else []) + [
        "serve", f"--allow-listen={http.HOST}:{port}", "--hostname", http.HOST, "--port", str(port),
        "--workers", str(workers), "server.js"]
    proc = subprocess.Popen(argv, cwd=http.FIXTURE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    peak, stop = [0.0], threading.Event()

    def sample():
        while not stop.is_set():
            peak[0] = max(peak[0], http.rss_mb(proc.pid))
            time.sleep(0.25)

    try:
        http.wait_listening(port, proc)
        time.sleep(1)
        idle = http.rss_mb(proc.pid)
        url = f"http://{http.HOST}:{port}/{scenario}"
        http.load(url, 2)
        sampler = threading.Thread(target=sample)
        sampler.start()
        cpu0, wall0 = cpu_seconds(proc.pid), time.time()
        report = http.load(url, seconds)
        cpu = (cpu_seconds(proc.pid) - cpu0) / (time.time() - wall0) * 100
    finally:
        stop.set()
        proc.terminate()
        try:
            proc.wait(15)
        except subprocess.TimeoutExpired:
            proc.kill()
    total = sum(report["statusCodeDistribution"].values())
    if total == 0 or report["statusCodeDistribution"].get("200", 0) != total:
        raise RuntimeError(f"not every response was 200: {report['statusCodeDistribution']}")
    return {"workers": workers, "shared_code_cache": cache,
            "requests_per_sec": report["summary"]["requestsPerSec"],
            "p99_ms": report["latencyPercentiles"]["p99"] * 1000,
            "idle_rss_mb": idle, "peak_rss_mb": peak[0], "cpu_percent": cpu}


def slopes(rows):
    """MB per extra worker against the 1-worker row, idle and loaded."""
    base = rows[0]
    for row in rows[1:]:
        extra = row["workers"] - 1
        row["idle_rss_mb_per_extra_worker"] = (row["idle_rss_mb"] - base["idle_rss_mb"]) / extra
        row["peak_rss_mb_per_extra_worker"] = (row["peak_rss_mb"] - base["peak_rss_mb"]) / extra
    for row in rows:
        row["peak_rss_mb_per_worker_total"] = row["peak_rss_mb"] / row["workers"]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--duration", type=int, default=6)
    ap.add_argument("--scenarios", nargs="+", default=["plaintext", "stream"])
    ap.add_argument("--workers", type=int, nargs="+")
    ap.add_argument("--out-dir", type=Path, default=ROOT / "benches/baselines")
    args = ap.parse_args()
    velyr = os.environ.get("VELYR_BIN", str(ROOT / "target/release/velyr"))
    cores = os.cpu_count() or 1
    counts = args.workers or [n for n in (1, 2, 4, 8, 16) if n <= cores]
    distribution = "reuseport" if sys.platform.startswith(("linux", "freebsd")) else "handoff (acceptor)"
    for scenario in args.scenarios:
        result = {"scenario": scenario, "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                  "machine": http.machine(), "cores": cores, "distribution": distribution,
                  "reuseport": "pending: Linux only, covered by the reuseport CI job (VELYR-156)",
                  "load_generator": "oha", "connections": http.CONNECTIONS, "duration_s": args.duration,
                  "velyr": http.out([velyr, "--version"]), "load_average": os.getloadavg(),
                  "note": "indicative numbers from a machine that runs other work; oha shares the cores",
                  "runs": {}}
        for cache in (False, True):
            rows = [one(velyr, n, cache, scenario, args.duration) for n in counts]
            slopes(rows)
            result["runs"]["shared_code_cache" if cache else "baseline"] = rows
        result["load_average_after"] = os.getloadavg()
        http.write(args.out_dir / f"m5-multicore-{scenario}-{result['machine']['platform']}.json", result)


if __name__ == "__main__":
    main()
