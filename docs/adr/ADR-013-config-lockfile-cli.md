# ADR-013 — Configuration file, lockfile, permission flags and CLI verbs

**Status:** Accepted (2026-10-15)
**Relates to:** PRD §7, SPEC §3, §8, `security/supply-chain.md`

## Decision

### `velyr.json`
- JSON, with comments allowed (JSONC), at the project root. It is discovered by walking up from the current directory.
- Top-level keys: `permissions`, `quotas`, `workers`, `imports` (import map), `compilerOptions` (`jsx`, `jsxImportSource`).
- `permissions` uses the same grammar as the CLI flags. If any `--allow-*`, `--deny-*` or `-A` flag is given, the CLI flags **replace** the config's `permissions` block instead of merging with it.
- Only the root project's `velyr.json` can grant authority. A dependency's config is ignored.

### Permission flags
- `--allow-read[=paths]`, `--allow-write[=paths]`, `--allow-net[=host[:port]]` (outbound connect), `--allow-listen[=addr[:port]]`, `--allow-env[=names]`, `--allow-run[=programs]`, `--allow-native[=paths]`, `--allow-import[=hosts]` (remote modules), `--inspect[=addr]`.
- `--deny-<kind>` exists for each kind and always wins.
- `-A` / `--allow-all` grants everything except native loading and inspector, and prints a warning to stderr.
- No interactive prompts.

### Local module authority
- The CLI grants an implicit `ImportLocal(<project root>/**)`, where the project root is the directory of `velyr.json`, or the entry file's directory if there is no config.
- Importing a local module outside the root requires `--allow-read` covering it.
- `ImportLocal` never grants `FsRead`: being able to import a module does not let code read arbitrary files.

### Lockfile `velyr.lock`
- JSON with a `"lockfileVersion": 1` field and keys sorted at every level.
- `packages` maps `name@version` to `{ resolved, integrity, dependencies, optionalDependencies, os, cpu, hasInstallScript }`. `integrity` is an SRI `sha512` hash.
- Only `velyr add`, `velyr install` and `velyr remove` write it. `velyr run` never writes the lockfile.

### Package commands
- `velyr add <pkg>[@range]...`, `velyr remove <pkg>...` and `velyr install`. `velyr install --frozen` fails on any drift from the lockfile; CI uses `--frozen`.
- Lifecycle scripts are denied by default. `--allow-scripts=<pkg,...>` scopes the exception. `--allow-all-scripts` prints a warning.
- `node_modules` uses a hoisted, npm-compatible layout, populated by hardlinking from the content-addressed global cache.
- The global cache lives in the platform cache directory (`directories` crate): `velyr/npm/<sha512-hex>/`. Writes are atomic (temp file plus rename).
- Registry configuration and `_authToken` are read from `.npmrc` by the installer only. Auth tokens are never exposed to user JS.
- Default export conditions: ESM resolves `["velyr", "import", "default"]`. `"node"` is added only when Node compatibility is linked. CommonJS resolves with `"require"` in place of `"import"`.
