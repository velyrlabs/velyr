# SPEC — Workers

## Isolation

Each Worker has:
- engine isolate/realm;
- JS heap;
- microtask queue;
- permission context;
- quota context;
- resource table or resource namespace.

## Permission derivation

```text
effective = parent ∩ requested ∩ deployment_policy
```

## Messaging

Support:
- structured clone;
- transferable buffers where semantics permit;
- MessagePort-like channels for Web compatibility.

## Termination

Worker termination:
1. prevent new host ops;
2. cancel cancellable ops;
3. reject/ignore late completions safely;
4. close resources;
5. destroy engine state.

## Server workers

Internal server workers need not expose every mechanism as public JS Worker APIs. Public semantics and internal scheduling remain separate layers.
