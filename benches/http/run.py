#!/usr/bin/env python3
"""HTTP benchmark driver: one scenario (plaintext, json or stream) against Velyr and, when
installed, node, deno and bun, loaded by `oha`.

    VELYR_BIN=target/release/velyr python3 benches/http/run.py json [--duration 10]

Writes, under benches/baselines (override with --out-dir):
  m4-<scenario>-<platform>.json             Velyr only, in benches/schema/result.schema.json shape
  m4-<scenario>-comparison-<platform>.json  every runtime, with versions and what was skipped
  m4-summary-<platform>.json                all comparison files found, merged
Velyr runs with scoped grants (--allow-listen on the benchmark port only); nothing is relaxed.
Numbers from a shared machine are indicative, not a gate.
"""
import argparse
import json
import os
import platform
import shutil
import socket
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "tests/fixtures/m4-acceptance"
SERVERS = ROOT / "benches/http/servers"
CONNECTIONS = 64
HOST = "127.0.0.1"


def free_port():
    with socket.socket() as s:
        s.bind((HOST, 0))
        return s.getsockname()[1]


def out(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def runtimes(velyr, port):
    """name -> (version, argv, cwd); only the ones that are installed."""
    found = {"velyr": (out([velyr, "--version"]), [velyr, "serve", f"--allow-listen={HOST}:{port}",
                                                  "--hostname", HOST, "--port", str(port), "server.js"], FIXTURE)}
    if shutil.which("node"):
        found["node"] = (out(["node", "--version"]), ["node", str(SERVERS / "node.mjs"), str(port)], ROOT)
    if shutil.which("deno"):
        found["deno"] = (out(["deno", "--version"]).splitlines()[0],
                         ["deno", "run", f"--allow-net={HOST}:{port}", "--allow-read", str(SERVERS / "deno.js"), str(port)], ROOT)
    if shutil.which("bun"):
        found["bun"] = (out(["bun", "--version"]), ["bun", str(SERVERS / "bun.js"), str(port)], ROOT)
    return found


def wait_listening(port, proc):
    deadline = time.time() + 20
    while time.time() < deadline:
        if proc.poll() is not None:
            raise RuntimeError("server exited before listening")
        with socket.socket() as s:
            if s.connect_ex((HOST, port)) == 0:
                return
        time.sleep(0.1)
    raise RuntimeError("server never listened")


def rss_mb(pid):
    kb = out(["ps", "-o", "rss=", "-p", str(pid)])
    return int(kb) / 1024 if kb else 0.0


def load(url, seconds):
    cmd = ["oha", "-z", f"{seconds}s", "-c", str(CONNECTIONS), "--no-tui", "--output-format", "json", url]
    return json.loads(subprocess.run(cmd, capture_output=True, text=True, check=True).stdout)


def measure(argv, cwd, port, scenario, seconds):
    proc = subprocess.Popen(argv, cwd=cwd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    peak = [0.0]
    stop = threading.Event()

    def sample():
        while not stop.is_set():
            peak[0] = max(peak[0], rss_mb(proc.pid))
            time.sleep(0.25)

    try:
        wait_listening(port, proc)
        time.sleep(1)  # let startup allocations settle before sampling the idle RSS
        idle_rss_mb = rss_mb(proc.pid)
        url = f"http://{HOST}:{port}/{scenario}"
        load(url, 2)  # warm-up
        sampler = threading.Thread(target=sample)
        sampler.start()
        report = load(url, seconds)
    finally:
        stop.set()
        proc.terminate()
        try:
            proc.wait(10)
        except subprocess.TimeoutExpired:
            proc.kill()
    ok = report["statusCodeDistribution"].get("200", 0)
    total = sum(report["statusCodeDistribution"].values())
    if total == 0 or ok != total:
        raise RuntimeError(f"not every response was 200: {report['statusCodeDistribution']}")
    return {
        "requests_per_sec": report["summary"]["requestsPerSec"],
        "p99_ms": report["latencyPercentiles"]["p99"] * 1000,
        "rss_mb": peak[0],
        "idle_rss_mb": idle_rss_mb,
    }


def machine():
    cpu = out(["sysctl", "-n", "machdep.cpu.brand_string"]) or platform.processor() or "unknown"
    return {
        "platform": f"{platform.system().lower()}-{platform.machine().replace('arm64', 'aarch64')}",
        "cpu": cpu,
        "os": platform.platform(),
    }


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, indent=2) + "\n")
    print(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scenario", choices=["plaintext", "json", "stream"])
    ap.add_argument("--duration", type=int, default=10)
    ap.add_argument("--out-dir", type=Path, default=ROOT / "benches/baselines")
    ap.add_argument("--profile", default=os.environ.get("VELYR_PROFILE", "web"))
    args = ap.parse_args()
    velyr = os.environ.get("VELYR_BIN", str(ROOT / "target/release/velyr"))
    if not shutil.which("oha"):
        sys.exit("oha is missing: cargo install oha")
    if not Path(velyr).exists():
        sys.exit(f"{velyr} is missing: cargo build --release -p velyr-cli --no-default-features --features profile-{args.profile}")
    port = free_port()
    results, skipped = {}, []
    available = runtimes(velyr, port)
    for name in ["velyr", "node", "deno", "bun"]:
        if name not in available:
            skipped.append(name)
            print(f"warning: {name} is not installed, skipped", file=sys.stderr)
            continue
        version, argv, cwd = available[name]
        results[name] = {"version": version, **measure(argv, cwd, port, args.scenario, args.duration)}
    mach = machine()
    stamp = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    suffix = mach["platform"]
    velyr_metrics = {k: v for k, v in results["velyr"].items() if k != "version"}
    memory = out(["sysctl", "-n", "hw.memsize"])
    write(args.out_dir / f"m4-{args.scenario}-{suffix}.json", {
        "benchmark": f"http-{args.scenario}", "commit": out(["git", "rev-parse", "HEAD"]) or "unknown",
        # Same spelling as velyr-bench results (std::env::consts::OS), unlike the file suffix.
        "profile": args.profile, "platform": suffix.replace("darwin", "macos"), "cpu": mach["cpu"],
        "memory": f"{int(memory) // 2**20} MiB" if memory else "unknown",
        "rustc": out(["rustc", "--version"]) or "unknown", "os": mach["os"], "timestamp": stamp,
        "load_average": list(os.getloadavg()), "metrics": velyr_metrics,
    })
    write(args.out_dir / f"m4-{args.scenario}-comparison-{suffix}.json", {
        "scenario": args.scenario, "timestamp": stamp, "machine": mach, "load_generator": out(["oha", "--version"]),
        "connections": CONNECTIONS, "duration_s": args.duration, "velyr_profile": args.profile,
        "note": "indicative numbers from a machine that runs other work; not a gate",
        "runtimes": results, "skipped": skipped,
    })
    merged = {}
    for file in sorted(args.out_dir.glob(f"m4-*-comparison-{suffix}.json")):
        data = json.loads(file.read_text())
        merged[data["scenario"]] = data
    write(args.out_dir / f"m4-summary-{suffix}.json", {"machine": mach, "scenarios": merged})
    for name, row in results.items():
        print(f"{args.scenario:9} {name:6} {row['requests_per_sec']:10.0f} req/s  p99 {row['p99_ms']:7.2f} ms  rss {row['rss_mb']:6.1f} MB")


if __name__ == "__main__":
    main()
