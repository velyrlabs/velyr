#!/usr/bin/env python3
"""Driver for the vue-ssr fixture (VELYR-93). Starts `velyr serve` itself and stops it on exit.

    driver.py check --velyr BIN --dir APP_DIR
        GET / over a raw socket; asserts chunked encoding and that the shell arrives before the
        async component's markup.
"""
import argparse
import socket
import subprocess
import sys
import time
from pathlib import Path

HOST = "127.0.0.1"
SHELL_MARK, ASYNC_MARK = b"shell-marker", b"async-content"


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
    head, _, _ = b"".join(c for _, c in chunks).partition(b"\r\n\r\n")
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
    first_async = next((at for at, d in body if ASYNC_MARK in d), None)
    assert first_shell is not None, "shell marker never arrived"
    assert first_async is not None, "async content never arrived"
    assert first_async - first_shell >= 0.05, (
        f"async content arrived with the shell ({first_shell * 1000:.0f} ms vs {first_async * 1000:.0f} ms)")
    print(f"shell at {first_shell * 1000:.0f} ms, async content at {first_async * 1000:.0f} ms, "
          f"{len(body)} body reads, chunked")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["check"])
    ap.add_argument("--velyr", required=True)
    ap.add_argument("--dir", type=Path, default=Path(__file__).resolve().parent)
    args = ap.parse_args()
    check(str(Path(args.velyr).resolve()), args.dir)  # the server runs with cwd = the fixture


if __name__ == "__main__":
    main()
