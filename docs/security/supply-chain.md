# Supply-chain security

## Package installation principles

- verify integrity when registry metadata provides it;
- deterministic lockfile;
- content-addressed package cache;
- lifecycle scripts disabled by default;
- no implicit network execution during normal runtime when lock/cache policy forbids it.

## Lifecycle scripts

Default:
```text
preinstall  DENY
install     DENY
postinstall DENY
prepare     DENY
```

Opt-in should be package-scoped:

```bash
velyr install --allow-scripts=esbuild,sharp
```

A broad `--allow-all-scripts` option, if provided, should emit an explicit warning.

## Rust dependencies

CI:
- `cargo audit`;
- `cargo deny`;
- pinned/controlled toolchain;
- lockfile in repository for binaries;
- license policy;
- yanked crate policy;
- dependency diff review for releases.

## Release artifacts

Future release pipeline should target:
- provenance/SBOM;
- checksums;
- signed releases;
- reproducible-build investigation.
