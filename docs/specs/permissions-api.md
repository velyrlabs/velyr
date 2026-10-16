# SPEC — Permissions API

## Runtime-side representation

Permissions are normalized into capability rules independent from CLI syntax.

## Decision

```text
ALLOW
DENY
```

No implicit "prompt" exists in headless/server mode. Interactive prompt behavior, if implemented, is a CLI concern.

## Precedence

```text
explicit deny
    >
explicit scoped allow
    >
default deny
```

## Path decisions

MUST consider:
- canonicalization;
- relative path base;
- symlink policy;
- case sensitivity by platform;
- Windows drive/UNC semantics.

## Network decisions

MUST consider:
- host names;
- IP literals;
- ports;
- IPv4/IPv6 normalization;
- DNS resolution and rebinding model.

## API namespace

Runtime-specific permission APIs should live under a Velyr namespace, for example:

```js
await Velyr.permissions.query({ name: "read", path: "./data" })
```

Exact public shape remains versioned separately from internal capabilities.
