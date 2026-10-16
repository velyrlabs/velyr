# Fuzz targets

`fuzz/` is a separate `cargo-fuzz` workspace (nightly). It is not a member of the main workspace.

Setup (once):

```sh
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked
```

Run from the repository root (cargo-fuzz finds `fuzz/` from there):

```sh
cargo +nightly fuzz build
cargo +nightly fuzz run error_code_roundtrip -- -max_total_time=30
```

Targets:

- `error_code_roundtrip`: arbitrary bytes (UTF-8 only) fed to `ErrorCode::from_str`. Must never panic; a parsed code must round-trip through `as_str()`.
- `permissions_path`: arbitrary path strings checked against a fixed grant/deny tree (fs-read, fs-write, import-local, native-load). Must never panic; decisions are deterministic and idempotent under lexical cleaning; Allow only when the independently canonicalized path is under the grant and outside the deny.
- `module_resolve`: arbitrary specifiers resolved by `FsResolver` (with an import map and an escaping symlink) and loaded through `ModuleHost`. Must never panic; a loaded module's canonical path is always under the `ImportLocal` grant.
