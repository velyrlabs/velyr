#!/usr/bin/env python3
"""Large-app footprint driver (VELYR-183): the fixture of benches/large-app/gen.py on velyr (V8 and
native engines) and node, with 1 and 4 workers, loaded by `oha`. Reuses the helpers of
benches/http/run.py (port, load, machine, write), like benches/multicore/run.py.

    python3 -I benches/large-app/gen.py                       # once: writes the fixture
    python3 benches/large-app/run.py --smoke                  # boot, 1 request, stop (velyr V8)
    python3 benches/large-app/run.py --rounds 3 [--duration 6] [--workers 1 4]

Runtimes (names are the metric prefixes):
  velyr_v8        `velyr serve --workers N` over HTTP.         VELYR_BIN, default target/release/velyr
  node            node, cluster with N workers (1 = one process).
  velyr_v8_inproc `velyr run` calls the handlers in a loop, no HTTP; 1 worker.
  velyr_native_inproc   the same with `--engine native` (`velyr serve` has no engine switch); needs
                  a velyr built with the engine-native feature at VELYR_NATIVE_BIN, else skipped.
  node_inproc     node, same loop.
Memory is the RSS of the whole process tree (cluster workers included; velyr workers are threads):
idle after load, mean under load, peak. Performance: startup to the first response, graph load (what
the app measures around its dynamic import), req/s, p99, CPU per request, req/s per peak MB.
Writes <out-dir>/footprint-large-<ts>.json (every round) and footprint-large-latest.json (medians as
flat metrics, the shape velyr-bench reads in server mode). oha shares the machine: indicative numbers.
"""
import argparse
import importlib.util
import json
import os
import statistics
import subprocess
import sys
import threading
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "benches/fixtures/large-app"
ROUTES = ["json", "transform", "template"]
ENV_KNOBS = "LARGE_APP_CACHE_OBJECTS,LARGE_APP_JSON_ITEMS,LARGE_APP_TEMPLATE_ROWS,LARGE_APP_TRANSFORM_STEPS"

spec = importlib.util.spec_from_file_location("http_run", ROOT / "benches/http/run.py")
http = importlib.util.module_from_spec(spec)
spec.loader.exec_module(http)


def parse_time(text):
    total = 0.0
    for part in text.strip().replace("-", ":").split(":"):
        total = total * 60 + float(part)
    return total


def process_table():
    rows = {}
    for line in (http.out(["ps", "-axo", "pid=,ppid=,rss=,time="]) or "").splitlines():
        pid, ppid, rss, cpu = line.split()
        rows[int(pid)] = (int(ppid), int(rss) / 1024, parse_time(cpu))
    return rows


def tree(root_pid):
    """(rss MB, cpu seconds) summed over the process and all its descendants."""
    rows = process_table()
    members, grew = {root_pid}, True
    while grew:
        grew = False
        for pid, (ppid, _, _) in rows.items():
            if ppid in members and pid not in members:
                members.add(pid)
                grew = True
    live = [rows[p] for p in members if p in rows]
    return sum(r[1] for r in live), sum(r[2] for r in live)


def get(port, path, timeout=5):
    with urllib.request.urlopen(f"http://{http.HOST}:{port}{path}", timeout=timeout) as response:
        return response.status, response.read()


def wait_first_response(port, proc, started):
    while time.time() - started < 90:
        if proc.poll() is not None:
            raise RuntimeError(f"server exited with {proc.returncode} before answering")
        try:
            if get(port, "/health", 2)[0] == 200:
                return (time.time() - started) * 1000
        except OSError:
            time.sleep(0.02)
    raise RuntimeError("no response from /health in 90s")


def server_argv(runtime, workers, port, velyr):
    if runtime == "velyr_v8":
        return [velyr, "serve", f"--allow-listen={http.HOST}:{port}", f"--allow-env={ENV_KNOBS}",
                "--hostname", http.HOST, "--port", str(port), "--workers", str(workers), "server.js"], {}
    return ["node", "node-server.mjs"], {"PORT": str(port), "WORKERS": str(workers)}


def stop(proc):
    proc.terminate()
    try:
        proc.wait(15)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def serve_once(runtime, workers, duration, velyr):
    port = http.free_port()
    argv, extra = server_argv(runtime, workers, port, velyr)
    started = time.time()
    proc = subprocess.Popen(argv, cwd=FIXTURE, env={**os.environ, **extra},
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    samples, stop_flag = [], threading.Event()

    def sample():
        while not stop_flag.is_set():
            samples.append((time.time(), tree(proc.pid)[0]))
            time.sleep(0.25)

    try:
        startup_ms = wait_first_response(port, proc, started)
        stats = json.loads(get(port, "/stats")[1])
        for route in ROUTES:  # touch every route before the idle reading
            get(port, f"/{route}")
        time.sleep(1.5)
        idle_rss, cpu_before = tree(proc.pid)
        sampler = threading.Thread(target=sample)
        sampler.start()
        load_from, per_route, requests = time.time(), {}, 0
        for route in ROUTES:
            http.load(f"http://{http.HOST}:{port}/{route}", 2)  # warm-up
            report = http.load(f"http://{http.HOST}:{port}/{route}", duration)
            codes = report["statusCodeDistribution"]
            if codes.get("200", 0) != sum(codes.values()) or not codes:
                raise RuntimeError(f"/{route}: not every response was 200: {codes}")
            per_route[route] = {"requests_per_sec": report["summary"]["requestsPerSec"],
                                "p99_ms": report["latencyPercentiles"]["p99"] * 1000}
            requests += sum(codes.values())
        _, cpu_after = tree(proc.pid)
    finally:
        stop_flag.set()
        stop(proc)
    loaded = [rss for t, rss in samples if t >= load_from] or [idle_rss]
    rps = statistics.mean(r["requests_per_sec"] for r in per_route.values())
    peak = max(loaded)
    return {
        "startup_first_response_ms": startup_ms, "graph_load_ms": stats["graphLoadMs"],
        "state_ms": stats["stateMs"], "modules": stats["modules"], "cache_objects": stats["cacheObjects"],
        "idle_rss_mb": idle_rss, "load_rss_mb": statistics.mean(loaded), "peak_rss_mb": peak,
        "requests_per_sec": rps, "p99_ms": max(r["p99_ms"] for r in per_route.values()),
        "cpu_ms_per_request": (cpu_after - cpu_before) * 1000 / max(requests, 1),
        "requests_per_sec_per_mb": rps / peak, "routes": per_route,
    }


def inproc_argv(runtime, velyr, native):
    if runtime == "velyr_v8_inproc":
        return [velyr, "run", "-A", "inproc.js"]
    if runtime == "velyr_native_inproc":
        return [native, "run", "-A", "--engine", "native", "inproc.js"]
    return ["node", "inproc.js"]


def inproc_once(runtime, duration, velyr, native):
    argv = inproc_argv(runtime, velyr, native)
    env = {**os.environ, "INPROC_MS": str(int(duration * 1000))}
    started = time.time()
    # The native engine cannot load .cjs (no CommonJS host); VELYR_NATIVE_FIXTURE is a copy made with
    # `gen.py --cjs-every 0`: same modules, ESM only.
    cwd = Path(os.environ["VELYR_NATIVE_FIXTURE"]) if runtime == "velyr_native_inproc" and "VELYR_NATIVE_FIXTURE" in os.environ else FIXTURE
    proc = subprocess.Popen(argv, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    state = {"ready": None, "result": None, "idle": 0.0, "startup_ms": 0.0, "peak": 0.0}

    def read():
        for line in proc.stdout:
            if line.startswith("READY "):
                state["startup_ms"] = (time.time() - started) * 1000
                state["idle"] = tree(proc.pid)[0]
                state["ready"] = json.loads(line[6:])
            elif line.startswith("RESULT "):
                state["result"] = json.loads(line[7:])

    reader = threading.Thread(target=read)
    reader.start()
    while proc.poll() is None:
        state["peak"] = max(state["peak"], tree(proc.pid)[0])
        time.sleep(0.2)
    reader.join()
    if proc.returncode != 0 or not state["ready"] or not state["result"]:
        raise RuntimeError(f"{' '.join(argv[:2])} failed (exit {proc.returncode}); run it by hand in {FIXTURE}")
    result, ready = state["result"], state["ready"]
    return {
        "startup_first_response_ms": state["startup_ms"], "graph_load_ms": ready["graphLoadMs"],
        "state_ms": ready["stateMs"], "modules": ready["modules"], "cache_objects": ready["cacheObjects"],
        "idle_rss_mb": state["idle"], "peak_rss_mb": max(state["peak"], state["idle"]),
        "requests_per_sec": statistics.mean(r["perSec"] for r in result.values()),
        "requests_per_sec_per_mb": statistics.mean(r["perSec"] for r in result.values()) / max(state["peak"], 1),
        "routes": {k.lstrip("/"): {"requests_per_sec": v["perSec"]} for k, v in result.items()},
    }


def smoke(velyr, runtime):
    port = http.free_port()
    argv, extra = server_argv(runtime, 1, port, velyr)
    started = time.time()
    proc = subprocess.Popen(argv, cwd=FIXTURE, env={**os.environ, **extra},
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        first = wait_first_response(port, proc, started)
        status, body = get(port, "/json")
        data = json.loads(body)
        rss = tree(proc.pid)[0]
    finally:
        stop(proc)
    assert status == 200 and data["count"] > 0, (status, data.get("count"))
    print(f"smoke {runtime}: first response {first:.0f} ms, /json {len(body)} bytes, rss {rss:.0f} MB")


def median_row(rows):
    keys = [k for k, v in rows[0].items() if isinstance(v, (int, float))]
    return {k: statistics.median(r[k] for r in rows) for k in keys}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--smoke", action="store_true")
    ap.add_argument("--runtime", default="velyr_v8", choices=["velyr_v8", "node"], help="smoke only")
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--duration", type=int, default=6)
    ap.add_argument("--workers", type=int, nargs="+", default=[1, 4])
    ap.add_argument("--out-dir", type=Path, default=ROOT / "benches/results")
    args = ap.parse_args()
    velyr = os.environ.get("VELYR_BIN", str(ROOT / "target/release/velyr"))
    native = os.environ.get("VELYR_NATIVE_BIN", "")
    if not (FIXTURE / "app.js").exists():
        sys.exit("fixture missing: python3 -I benches/large-app/gen.py")
    if args.smoke:
        return smoke(velyr, args.runtime)
    if not __import__("shutil").which("oha"):
        sys.exit("oha is missing: cargo install oha")

    plan = [(r, w) for r in ("velyr_v8", "node") for w in args.workers]
    plan += [(r, 1) for r in ("velyr_v8_inproc", "velyr_native_inproc", "node_inproc")]
    runs, skipped = {}, {}
    for runtime, workers in plan:
        key = f"{runtime}_w{workers}"
        if runtime == "velyr_native_inproc" and not (native and Path(native).exists()):
            skipped[key] = "VELYR_NATIVE_BIN is not set to a velyr built with the engine-native feature"
            continue
        runs[key] = []
        for n in range(args.rounds):
            try:
                row = (inproc_once(runtime, args.duration, velyr, native) if runtime.endswith("_inproc")
                       else serve_once(runtime, workers, args.duration, velyr))
            except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
                skipped[key] = f"round {n + 1}: {error}"
                break
            row["load_average"] = os.getloadavg()[0]
            runs[key].append(row)
            print(f"{key} round {n + 1}: idle {row['idle_rss_mb']:.0f} MB, peak {row['peak_rss_mb']:.0f} MB, "
                  f"{row['requests_per_sec']:.0f} req/s", flush=True)
    runs = {k: v for k, v in runs.items() if v}

    stamp = datetime.now(timezone.utc)
    mach = http.machine()
    doc = {"benchmark": "footprint-large", "timestamp": stamp.strftime("%Y-%m-%dT%H:%M:%SZ"), "machine": mach,
           "cores": os.cpu_count(), "rounds": args.rounds, "duration_s": args.duration, "connections": http.CONNECTIONS,
           "velyr": http.out([velyr, "--version"]), "node": http.out(["node", "--version"]),
           "load_generator": http.out(["oha", "--version"]), "skipped": skipped, "runs": runs,
           "note": "indicative numbers from a machine that runs other work; oha shares the cores"}
    http.write(args.out_dir / f"footprint-large-{int(stamp.timestamp())}.json", doc)
    metrics = {f"{key}_{name}": value for key, rows in runs.items() for name, value in median_row(rows).items()
               if name != "load_average"}
    memory = http.out(["sysctl", "-n", "hw.memsize"])
    http.write(args.out_dir / "footprint-large-latest.json", {
        "benchmark": "footprint-large", "commit": http.out(["git", "rev-parse", "HEAD"]) or "unknown",
        "profile": "full", "platform": mach["platform"].replace("darwin", "macos"), "cpu": mach["cpu"],
        "memory": f"{int(memory) // 2**20} MiB" if memory else "unknown",
        "rustc": http.out(["rustc", "--version"]) or "unknown", "os": mach["os"],
        "timestamp": doc["timestamp"], "metrics": metrics})
    for key, why in skipped.items():
        print(f"skipped {key}: {why}", file=sys.stderr)


if __name__ == "__main__":
    main()
