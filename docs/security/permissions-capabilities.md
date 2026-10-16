# Permissions and capabilities

## Default policy

No ambient privileged I/O.

```text
read filesystem       DENY
write filesystem      DENY
connect network       DENY
listen network        DENY
read environment      DENY
spawn process         DENY
load native library   DENY
inspector             DENY
```

## Capability types

```text
FsRead(path pattern)
FsWrite(path pattern)
NetConnect(host, port/range)
NetListen(address, port/range)
EnvRead(name/pattern)
EnvWrite(name/pattern)
ProcessSpawn(executable/pattern)
ImportRemote(host)
NativeLoad(path)
InspectorBind(address)
```

## Rules

- explicit deny overrides allow;
- paths must be normalized before comparison;
- symlink handling must prevent path-policy bypass;
- DNS rebinding must be considered for host-based network policy;
- resolved socket destinations may require policy revalidation;
- IPv4/IPv6 equivalence must be normalized;
- wildcard rules must be conservative.

## Workers

Child effective authority:

```text
child = parent ∩ requested
```

Never union.

## Embedder API

Permission policy should be supplied as data/traits so platforms can define policy without reproducing CLI parsing.
