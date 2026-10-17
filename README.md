<p align="center">
  <a href="https://velyr.dev">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset=".github/assets/brand/banner-dark.png">
      <img src=".github/assets/brand/banner-light.png" alt="Velyr" width="100%">
    </picture>
  </a>
</p>

<h3 align="center">The security-first JavaScript and TypeScript runtime, written in Rust.</h3>

<p align="center">
  <a href="https://velyr.dev">Website</a>
  &nbsp;&middot;&nbsp;
  <a href="https://velyr.dev/guide/features">Docs</a>
  &nbsp;&middot;&nbsp;
  <a href="https://github.com/velyrlabs/velyr/releases">Releases</a>
  &nbsp;&middot;&nbsp;
  <a href="https://velyr.dev/guide/roadmap">Roadmap</a>
  &nbsp;&middot;&nbsp;
  <a href="https://github.com/velyrlabs/velyr/discussions">Discussions</a>
</p>

<p align="center">
  <a href="https://github.com/velyrlabs/velyr/actions/workflows/ci.yml"><img src="https://github.com/velyrlabs/velyr/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/velyrlabs/velyr/releases"><img src="https://img.shields.io/github/v/release/velyrlabs/velyr?include_prereleases&label=release" alt="Latest release"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License: MIT OR Apache-2.0"></a>
</p>

> [!WARNING]
> Velyr is **alpha** software (`v0.1.0-alpha.1`). APIs and flags will change, and it
> is not ready for production. See [what works today](#status).

## Why Velyr

- **Nothing is allowed until you allow it.** Files, network, environment and
  subprocesses are denied by default and granted per path, host or name.
- **Web standards first, TypeScript included.** `fetch`, streams, Web Crypto and
  WebSocket out of the box; `.ts` and `.tsx` run directly by type stripping.
- **Releases you can verify.** Every binary ships with SHA-256 checksums, a
  CycloneDX SBOM and a signed build provenance attestation.

## A quick look

```ts
// main.ts
const res = await fetch("https://example.com");
console.log(res.status);
```

```console
$ velyr run main.ts
Uncaught VELYR_PERMISSION_DENIED: net-connect access denied

$ velyr run --allow-net=example.com main.ts
200
```

Serve HTTP with a single export:

```ts
// server.ts
export default {
  fetch(req: Request): Response {
    return new Response("hello from velyr");
  },
};
```

```console
$ velyr serve --hostname 127.0.0.1 --port 8000 --allow-listen=127.0.0.1:8000 server.ts
$ curl http://127.0.0.1:8000
hello from velyr
```

## Install

Download a prebuilt binary from
[GitHub Releases](https://github.com/velyrlabs/velyr/releases):

| Platform | Archive |
|----------|---------|
| macOS (Apple silicon / Intel) | `velyr-aarch64-apple-darwin.tar.gz` / `velyr-x86_64-apple-darwin.tar.gz` |
| Linux x64 (glibc / musl) | `velyr-x86_64-unknown-linux-gnu.tar.gz` / `velyr-x86_64-unknown-linux-musl.tar.gz` |
| Linux arm64 (glibc / musl) | `velyr-aarch64-unknown-linux-gnu.tar.gz` / `velyr-aarch64-unknown-linux-musl.tar.gz` |
| Windows x64 | `velyr-x86_64-pc-windows-msvc.zip` |

```sh
TARGET=aarch64-apple-darwin   # pick your platform from the table
curl -fLO "https://github.com/velyrlabs/velyr/releases/download/v0.1.0-alpha.1/velyr-$TARGET.tar.gz"
tar -xzf "velyr-$TARGET.tar.gz" && ./velyr --version
```

<details>
<summary>Verify the download (checksum and provenance)</summary>

```sh
BASE=https://github.com/velyrlabs/velyr/releases/download/v0.1.0-alpha.1
curl -fLO "$BASE/SHA256SUMS"
grep "velyr-$TARGET.tar.gz" SHA256SUMS | shasum -a 256 -c -   # sha256sum -c on Linux
gh attestation verify "velyr-$TARGET.tar.gz" --repo velyrlabs/velyr
```

</details>

<details>
<summary>Build from source</summary>

Requires the Rust toolchain pinned in `rust-toolchain.toml`.

```sh
git clone https://github.com/velyrlabs/velyr && cd velyr
cargo build --release -p velyr-cli
./target/release/velyr --version
```

</details>

## Usage

| Command | What it does |
|---------|--------------|
| `velyr run <file>` | Run JavaScript or TypeScript |
| `velyr serve <file>` | Serve a module that exports `fetch`, optionally on several workers (`--workers`) |
| `velyr check` | Type check with the project's own `tsc` |
| `velyr test` / `velyr bench` | Run tests and benchmarks |
| `velyr add` / `remove` / `install` | Manage dependencies with a `velyr.lock` lockfile |
| `velyr x <bin>` | Run a binary from `node_modules/.bin` |
| `velyr compile <file>` | Bundle into a single executable |
| `velyr compat` | Report which Node APIs and packages a project uses |

Run `velyr --help` for every flag.

## Permissions

| Flag | Grants |
|------|--------|
| `--allow-read[=PATHS]` / `--allow-write[=PATHS]` | Filesystem access |
| `--allow-net[=HOST[:PORT]]` | Outbound network |
| `--allow-listen[=ADDR[:PORT]]` | Inbound connections |
| `--allow-env[=NAMES]` | Environment variables |
| `--allow-sys[=NAMES]` | System information |
| `-A`, `--allow-all` | All of the above; reaches loopback but not private networks |

Every `--allow-*` has a matching `--deny-*`, and deny always wins. Permissions are
an application boundary, not a sandbox for hostile code. Read the
[permissions guide](https://velyr.dev/guide/permissions) and the
[security model](docs/SECURITY.md).

## Status

| Works today | Not there yet |
|-------------|---------------|
| ES modules and scripts on V8, with TypeScript and JSX | Node.js compatibility is partial; many npm packages will not run ([matrix](docs/COMPATIBILITY-MATRIX.md)) |
| `fetch`, streams, Web Crypto, WebSocket | HTTP throughput is behind Node.js, Deno and Bun; no benchmark claims yet |
| `serve` with multiple worker isolates | The Rust-native engine (`--engine native`) is experimental and not in prebuilt binaries |
| Package management, `test`, `bench`, `compile` | No stability guarantees, no completed security audit, no 1.0 API |

Track progress in the [roadmap](docs/ROADMAP.md).

## Built on

Velyr stands on [V8](https://v8.dev), [Tokio](https://tokio.rs),
[Oxc](https://oxc.rs) and [Hyper](https://hyper.rs), and measures itself against
[test262](https://github.com/tc39/test262) and
[Web Platform Tests](https://web-platform-tests.org).

## Contributing

Issues and pull requests are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md);
the project is run as described in [GOVERNANCE.md](GOVERNANCE.md) and follows the
[Code of Conduct](CODE_OF_CONDUCT.md). The codebase is a Cargo workspace under
[`crates/`](crates/), and design documents live in [`docs/`](docs/README.md).

Found a vulnerability? Please report it privately through
[GitHub security advisories](https://github.com/velyrlabs/velyr/security/advisories/new),
not in a public issue. See [SECURITY.md](SECURITY.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
