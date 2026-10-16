# SPEC — Host Operations

## Operation request

Conceptual:

```rust
struct OpContext {
    isolate_id: IsolateId,
    permissions: CapabilitySet,
    quotas: QuotaContext,
    cancellation: CancellationToken,
    trace: TraceContext,
}
```

An op MAY be sync only if:
- it is computationally bounded;
- it does not block on OS I/O;
- worst-case latency is acceptable on isolate thread.

Blocking work must use an appropriate native worker mechanism.

## Async completion

```text
JS call
→ decode
→ authorize
→ reserve budget
→ start Future
→ Promise returned
...
Future completes
→ completion queue
→ isolate wake
→ decode result
→ resolve/reject
→ microtask semantics
```

## Error classes

Host errors should include stable categories:
- permission denied;
- quota exceeded;
- not found;
- already exists;
- invalid data;
- network;
- TLS;
- timeout;
- cancelled;
- unsupported;
- internal.

Compatibility adapters may map these to ecosystem-specific codes.
