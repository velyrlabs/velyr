# SPEC — Plugin API

## Preferred extension format

WebAssembly Component Model + WIT.

## Requirements
- explicit imported interfaces;
- no ambient filesystem/network;
- capability injection;
- configurable memory limits;
- configurable execution/fuel/epoch limits where supported;
- versioned WIT worlds;
- deterministic serialization contracts.

## Plugin classes

### Pure
No host capabilities. Parsing/transformation/computation.

### Capability plugin
Receives explicit narrow services, e.g. read one virtual store.

### Privileged
Requires deployment opt-in.

## Native plugins

Native dynamic libraries are compatibility/advanced features, not the recommended general Velyr plugin API.
