# Velyr.serve with a ReadableStream body (VELYR-164)

Scenario: `GET /stream`, 16 chunks of 64 B from a `ReadableStream` with a synchronous `pull`
(`tests/fixtures/m4-acceptance/server.js`), 64 connections, `oha`, Apple M4 Pro. Baseline in
`benches/baselines/m4-stream-comparison-darwin-aarch64.json`: 9922 req/s, p99 9.09 ms.

## Profile before the change

`samply record` of `velyr serve` (release, profile-full) under load, 8030 samples on the isolate
thread (`velyr-isolate`). The tokio worker threads had 5 samples: the isolate thread is the
bottleneck, not I/O.

| self time, isolate thread | before | after |
|---|---|---|
| `__sendto` (socket writes, via hyper `poll_write_priv`) | 38.9% | 11.4% |
| `__recvfrom` | 3.8% | 7.8% |
| everything else | under 4% each | under 4% each |

Cause: `serveResponse` (`crates/web-fetch/src/js/60_serve.js`) did `await reader.read()` then
`await op_serve_write(chunk)` per chunk, then `op_serve_end`. Each write is a round trip through
the event loop, so hyper received one frame per loop turn, found the body channel empty, and
flushed: one `sendto` per chunk plus one for the terminating chunk, 17 syscalls and 17 loop turns
per request. The stream had all 16 chunks ready within a single microtask burst; nothing used that.
It was not a wake problem in the event loop and not the io-http channel: both only saw one chunk
at a time because the JS side handed them one at a time.

## Change

- `serveStreamBody`: after a chunk, keep reading while the next read settles in the same
  microtask burst (`Promise.race` against an already resolved promise), up to 64 KiB, and send the
  batch as one `op_serve_write`. A read that is not ready is kept as the pending read, so a chunk
  that needs a later turn (timer, I/O) is sent alone and never held back. Order is unchanged; bytes
  read before an error or a non-`Uint8Array` chunk are written before the error is raised.
- `op_serve_write(id, data, end)`: when the batch reached the end of the stream, the last write also
  closes the body, which saves the `op_serve_end` round trip and lets hyper emit the final chunk in
  the same flush.

Quotas, permissions and backpressure are untouched: a write still waits on the bounded body channel
and the 64 KiB cap bounds what one write buffers.

## Result

Same release binary profile (full), same driver (`benches/http/run.py stream --duration 8`), a
machine with a load average of 8 to 10 (other work running), so read these as indicative:

| | req/s | p99 |
|---|---|---|
| before (this machine, same day) | 10039 | 12.5 ms |
| after, four runs | 18730, 28019, 26423, 25513 (under samply) | 4.4 to 20 ms |

Median about 26k req/s, 2.6x the baseline; the slowest run (18.7k) coincided with another job on the
machine. Node, deno and bun dropped about 10% in the same runs. plaintext 39.0k (baseline 40.1k) and
json 34.5k to 35.4k (baseline 40.1k) did not change code paths; node fell from 80.7k to 72.7k on json
in the same run, so the ratio to node is the same (0.50 and 0.49).

## What is left

Velyr is still about half of node on this scenario (the isolate thread is still saturated; each
request costs `accept`, `take`, `respond`, `write`, `disconnected` and `close` ops). Folding more
of those ops is the next step and lives in the same files plus runtime-ops.

Side effect: the client no longer sees 16 chunks (HTTP chunk boundaries are not a contract); the
acceptance test checks the 1024 bytes and 1 to 16 chunks instead.
