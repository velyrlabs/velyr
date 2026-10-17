# Velyr Security Program

## Security objective

A normal Velyr application should receive only the host authority it explicitly needs.

## Required repository controls

- `SECURITY.md` at repository root before public beta;
- private disclosure mechanism;
- security advisories process;
- dependency advisory automation;
- fuzzing of parser, bytecode, resolver and protocol boundaries;
- explicit unsafe-code inventory;
- release threat review;
- no secrets in CI logs;
- signed/provenance release plan before stable.

## Severity examples

### Critical
- arbitrary native code execution from ordinary JS without granted native/process authority;
- runtime permission bypass exposing arbitrary filesystem/environment/network;
- cross-isolate memory corruption.

### High
- scoped permission escape;
- crafted package/source causing reliable memory corruption;
- inspector remotely exposed without authorization.

### Medium/Low
Assessed by exploitability, authority gained, confidentiality/integrity impact and deployment prerequisites.

## Process spawn deny is best-effort with `allow-run=*`

`--deny-run` matches the requested name, the basename of the resolved path (case-insensitive on
macOS) and the canonical path (symlinks followed), so `deny-run=sudo` blocks `/usr/bin/sudo`,
`SUDO` on macOS and a symlink to it. With `--allow-run=*` this is still best-effort: copying the
binary to another name and path defeats it. For a real boundary use an explicit `--allow-run`
list. The CLI prints a warning when `allow-run=*` is combined with any `deny-run`.

## Filesystem: known limitations (VELYR-179, VELYR-181)

- **Residual TOCTOU in `node:fs`.** Asynchronous operations re-resolve every authorized path right
  before the I/O, which closes the deterministic swap (a parent directory replaced by a symlink
  between the call and the job). A window of microseconds remains between that re-check and the
  syscall, and the recursive walks of `cp` and `rm` resolve each child by path. A script that
  races symlink swaps on the blocking pool can occasionally win. Closing it needs fd-relative
  resolution (`openat`/`openat2`/`cap-std`); this was evaluated and accepted as a known
  limitation, so it is not planned. For a hard boundary use OS-level isolation (container, separate user).
- **Nested deny in tree operations.** Recursive `cp`, `rename` of a directory and recursive `rm`
  authorize their root, then ask the permission set whether a deny of the same kind exists below it
  (`CapabilitySet::has_deny_beneath`, cost proportional to the number of denies) and fail with
  `VELYR_PERMISSION_DENIED` if so. Recursive `cp` checks both read and write denies. Entries
  created or linked into the tree after the check are not re-checked (see the TOCTOU item).
- **Deny on a symlink applies to its target.** Patterns are canonicalized when the set is built, so
  `deny` on `DIR/link` denies the link target, as in Deno. Unlinking or renaming the link itself is
  still allowed by the grant on its parent directory.
