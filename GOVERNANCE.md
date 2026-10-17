# Governance

## Current state

Velyr is in alpha and has one maintainer, @lucasbrito-wdt, who is the whole core team
(`@velyrlabs/core`). Decisions are made by that maintainer. This is a BDFL-style arrangement and it
is explicitly temporary: the goal is to grow area maintainers and move to the model below as
contributors appear. The trade-off is speed and coherence now against a single point of failure.

## Roles

| Role | What they do | How they get it |
|------|--------------|-----------------|
| Contributor | Opens issues, Discussions and pull requests | Anyone |
| Triager | Applies `area/*` and `kind/*` labels, reproduces bugs, closes duplicates | Sustained, useful triage; invited by the core team |
| Area maintainer | Reviews and approves changes in one area; owns its CODEOWNERS entry | Sustained quality contributions in that area; nominated by the core team |
| Core team | Cross-area decisions, releases, security response, role grants | Nomination by the core team; today only @lucasbrito-wdt |

Roles are granted by the core team after a public nomination (in a Discussion) with no
unresolved objection for 7 days. Inactive roles lapse after 6 months without activity and can be
restored on request.

## Areas

All areas are owned by `@velyrlabs/core` for now. They are listed so ownership can be split later.

| Area | Crates and paths |
|------|------------------|
| engine | `engine-api`, `engine-api-conformance`, `engine-v8`, `engine-native`, `engine-vm`, `engine-gc` |
| runtime | `runtime-core`, `runtime-event-loop`, `runtime-modules`, `runtime-workers`, `runtime-ops`, `runtime-inspector`, `runtime-telemetry`, `runtime-vfs` |
| security | `runtime-permissions`, `runtime-resources`, `runtime-errors`, `docs/security`, `SECURITY.md` |
| web | `web-core`, `web-crypto`, `web-fetch`, `web-streams`, `web-websocket` |
| io | `io-fs`, `io-http`, `io-net`, `io-process`, `io-tls` |
| compat | `compat-commonjs`, `compat-napi`, `compat-node`, `compat-npm`, `qa-test262`, `qa-wpt` |
| cli | `cli`, `embed`, `tooling-*`, `bench-harness`, `benches/` |

## Decision making

- Lazy consensus: a proposal in an issue or pull request is accepted if nobody objects within a
  reasonable review window. Silence is agreement, objections must be specific.
- Architecture decisions are recorded as ADRs in `docs/adr`. A change that reverses or adds to one
  needs a new ADR in the same pull request.
- Changes to the public surface need an RFC. Until there is an RFC repository, open it as a
  Discussion in the "Ideas" category labelled `needs-rfc`.
- Security-sensitive decisions are made privately by the core team and published in the advisory.
- Disagreements the maintainers cannot settle are decided by the core team.

## Code of Conduct

Enforced by the core team. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Changing this document

Open a pull request. Governance changes need approval from the core team and stay open for at
least 7 days.
