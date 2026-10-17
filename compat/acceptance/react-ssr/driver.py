#!/usr/bin/env python3
"""Driver for the react-ssr fixture (VELYR-92). Starts `velyr serve` itself and stops it on exit.

    driver.py check --velyr BIN --dir APP_DIR
        GET / over a raw socket; asserts chunked encoding and that the shell arrives before the
        Suspense content.
    driver.py bench --velyr BIN [--dir APP_DIR] [--duration 10] [--out PATH]
        TTFB (median of sequential requests) and throughput (oha); writes the velyr-bench result JSON.

Throughput is capped by the 150 ms Suspense delay: connections / 0.15 s.
"""
import argparse
import json
import os
import platform
import socket
import statistics
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).resolve().parent
HOST = "127.0.0.1"
CONNECTIONS = 64
SHELL_MARK, SUSPENDED_MARK = b"shell-marker", b"suspended-content"


def free_port():
    with socket.socket() as s:
        s.bind((HOST, 0))
        return s.getsockname()[1]


def start(velyr, app_dir, port):
    proc = subprocess.Popen(
        [velyr, "serve", f"--allow-listen={HOST}:{port}", "--allow-env=NODE_ENV", "--hostname", HOST, "--port", str(port), "server.js"],
        cwd=app_dir, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    deadline = time.time() + 30
    while time.time() < deadline:
        if proc.poll() is not None:
            sys.exit(f"server exited with {proc.returncode}: {proc.stderr.read().decode(errors='replace')[-600:]}")
        with socket.socket() as s:
            if s.connect_ex((HOST, port)) == 0:
                return proc
        time.sleep(0.1)
    proc.kill()
    sys.exit("server never listened")


def stop(proc):
    proc.terminate()
    try:
        proc.wait(10)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def fetch_timed(port):
    """One raw GET /: (headers, [(seconds since send, bytes)]) for every recv."""
    with socket.create_connection((HOST, port), timeout=10) as s:
        sent = time.perf_counter()
        s.sendall(f"GET / HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n".encode())
        chunks = []
        while data := s.recv(65536):
            chunks.append((time.perf_counter() - sent, data))
    raw = b"".join(c for _, c in chunks)
    head, _, _ = raw.partition(b"\r\n\r\n")
    return head.decode("latin-1"), chunks


def body_chunks(head, chunks):
    """(time, bytes) of the recvs that carry body bytes."""
    head_len = len(head) + 4
    seen, out = 0, []
    for at, data in chunks:
        if seen + len(data) > head_len:
            out.append((at, data[max(head_len - seen, 0):]))
        seen += len(data)
    return out


def check(velyr, app_dir):
    port = free_port()
    proc = start(velyr, app_dir, port)
    try:
        head, chunks = fetch_timed(port)
    finally:
        stop(proc)
    lowered = head.lower()
    assert head.startswith("HTTP/1.1 200"), f"status line: {head.splitlines()[0]}"
    assert "transfer-encoding: chunked" in lowered, f"response is not chunked:\n{head}"
    assert "content-length" not in lowered, "chunked response carries content-length"
    body = body_chunks(head, chunks)
    first_shell = next((at for at, d in body if SHELL_MARK in d), None)
    first_slow = next((at for at, d in body if SUSPENDED_MARK in d), None)
    assert first_shell is not None, "shell marker never arrived"
    assert first_slow is not None, "suspended content never arrived"
    assert first_slow - first_shell >= 0.05, (
        f"suspended content arrived with the shell ({first_shell * 1000:.0f} ms vs {first_slow * 1000:.0f} ms)")
    full = b"".join(d for _, d in body)
    assert b'id="fallback"' in full, "fallback not in the streamed HTML"
    print(f"shell at {first_shell * 1000:.0f} ms, suspended content at {first_slow * 1000:.0f} ms, "
          f"{len(body)} body reads, chunked")


def out(cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


def bench(velyr, app_dir, duration, out_path, profile):
    port = free_port()
    proc = start(velyr, app_dir, port)
    try:
        ttfb = []
        for _ in range(30):
            head, chunks = fetch_timed(port)
            ttfb.append(body_chunks(head, chunks)[0][0] * 1000)
        url = f"http://{HOST}:{port}/"
        cmd = ["oha", "-z", f"{duration}s", "-c", str(CONNECTIONS), "--no-tui", "--output-format", "json", url]
        report = json.loads(subprocess.run(cmd, capture_output=True, text=True, check=True).stdout)
    finally:
        stop(proc)
    dist = report["statusCodeDistribution"]
    if not dist or set(dist) != {"200"}:
        sys.exit(f"not every response was 200: {dist}")
    mach = platform.machine().replace("arm64", "aarch64")
    memory = out(["sysctl", "-n", "hw.memsize"])
    result = {
        "benchmark": "react-ssr", "commit": out(["git", "rev-parse", "HEAD"]) or "unknown", "profile": profile,
        "platform": f"{platform.system().lower().replace('darwin', 'macos')}-{mach}",
        "cpu": out(["sysctl", "-n", "machdep.cpu.brand_string"]) or platform.processor() or "unknown",
        "memory": f"{int(memory) // 2**20} MiB" if memory else "unknown",
        "rustc": out(["rustc", "--version"]) or "unknown", "os": platform.platform(),
        "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "metrics": {
            "ttfb_ms": statistics.median(ttfb),
            "ttfb_ms_p95": sorted(ttfb)[int(len(ttfb) * 0.95) - 1],
            "requests_per_sec": report["summary"]["requestsPerSec"],
            "p99_ms": report["latencyPercentiles"]["p99"] * 1000,
        },
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(result, indent=2) + "\n")
    print(out_path)
    print(json.dumps(result["metrics"]))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["check", "bench"])
    ap.add_argument("--velyr", default=os.environ.get("VELYR_BIN", "target/release/velyr"))
    ap.add_argument("--dir", type=Path, default=HERE)
    ap.add_argument("--duration", type=int, default=10)
    ap.add_argument("--profile", default="web")
    ap.add_argument("--out", type=Path, default=HERE.parents[2] / "benches/results/react-ssr/react-ssr.json")
    args = ap.parse_args()
    args.velyr = str(Path(args.velyr).resolve())  # the server runs with cwd = the fixture
    if args.mode == "check":
        check(args.velyr, args.dir)
    else:
        bench(args.velyr, args.dir, args.duration, args.out, args.profile)


if __name__ == "__main__":
    main()
