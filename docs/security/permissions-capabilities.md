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

## Running programs (`--allow-run`)

`ProcessSpawn` is trust in the granted program, not a sandbox around it. The child runs with the
user's full authority and velyr's permissions do not apply to it, so granting a program that
executes code or commands is the same as granting everything. This covers shells (`sh`, `bash`,
`zsh`, `cmd`, `powershell`), interpreters (`node`, `velyr`, `python`, `perl`, `ruby`) and tools
that run or read arbitrary things (`git`, `find`, `tar`, `ssh`, `env`, `xargs`, `cat`). A bare
`--allow-run` grants every program.

The CLI prints one warning on stderr when the effective `allow-run` covers one of these or has no
list. Grant only programs you would run yourself with your own account.

Loader variables (`LD_*`, `DYLD_*`) are refused in a child's environment. The `quotas.processes`
limit counts live children for the whole process, workers included.

The `node` shim (`velyr run --node-shim`, put on `PATH` by `velyr x` and lifecycle scripts) never
grants authority: it runs with the flags of the process that spawned it. A worker's children get
no flags and run with the default (deny).

## Workers

Child effective authority:

```text
child = parent ∩ requested
```

Never union.

## Embedder API

Permission policy should be supplied as data/traits so platforms can define policy without reproducing CLI parsing.
