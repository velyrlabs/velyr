# Security policy

## Supported versions

Velyr is in alpha. Only the latest `v0.1.0-alpha.N` pre-release receives security fixes. There is no
backport policy yet.

## Reporting a vulnerability

Do not open a public issue. Report privately through GitHub:

https://github.com/velyrlabs/velyr/security/advisories/new

Please include:

- the Velyr version (`velyr --version`), OS and target;
- the command line, including every permission flag used;
- a minimal script or package that reproduces the problem;
- what you expected and what happened, and the authority gained (for example file read outside the
  granted paths, process spawn, network access);
- whether you want to be credited.

## What to expect

This project has a single maintainer, so these are best-effort targets, not guarantees:

- acknowledgement within 7 days;
- an assessment and a plan after that, with updates in the advisory;
- coordinated disclosure: we agree on a date with you before publishing.

## Scope and known limitations

The threat model, severity examples and documented limitations (for example the residual TOCTOU
window in `node:fs` and the best-effort `--deny-run` with `--allow-run=*`) are in
[docs/SECURITY.md](docs/SECURITY.md) and [docs/security/threat-model.md](docs/security/threat-model.md).
Reports about a limitation already documented there are welcome as hardening ideas, but are not
treated as new vulnerabilities.
