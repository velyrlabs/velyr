# Issue tracker

Tracker: forgeboard MCP. Project key: `VELYR`. Project name: Velyr Core.

Work items live in forgeboard (server `forgeboard`), not in files. Skills that plan or triage call the `fb_*` tools below.

## Project selection

The project is chosen per request by the `X-Forgeboard-Project` header (the key or slug), pinned in this repo's `.mcp.json`. Without the header, pass `project` as a tool argument or call `fb_project_resolve` with `repoRemote`. Every other `fb_*` tool also accepts `project`.

The actor comes from the `X-Forgeboard-Actor` header, or the `actor` argument. A project-scope entry overrides the user-scope `forgeboard` entry of the same name, so keep both headers here:

```json
{
  "mcpServers": {
    "forgeboard": {
      "type": "http",
      "url": "http://127.0.0.1:9090/forgeboard/mcp",
      "headers": {
        "X-Forgeboard-Project": "VELYR",
        "X-Forgeboard-Actor": "claude-code"
      }
    }
  }
}
```

## Operation map

| Operation | Tool | Notes |
|-----------|------|-------|
| Confirm the project | `fb_project_resolve` | `create=true` with `key` and `name` only after the user agrees |
| Create tickets in batch | `fb_ticket_create` | `tickets[]` with `ref`, edges by `ref` (`from` blocks `to`); warns about duplicates but still creates |
| List ready tickets | `fb_frontier` | filter by `effort`, `type`, `mode`; `responseFormat: detailed` for bodies |
| Claim | `fb_ticket_claim` | no `ticket` takes the top of the frontier; returns a `fence` for every later write |
| Heartbeat | `fb_ticket_claim` | same `ticket` plus `fence`, every 1-2 minutes (lease defaults to 10 min) |
| Release | `fb_ticket_release` | needs `fence`; ticket returns to the frontier |
| Resolve | `fb_ticket_resolve` | `resolution` (first line becomes the decision gist), `outcome` done or wontfix |
| Read one ticket | `fb_ticket_get` | by key (`VELYR-12`) or exact title |
| Edit or reopen | `fb_ticket_update` | `fence` while claimed; `reopen=true` for closed tickets |
| Link | `fb_link` | `action` add or remove; `kind` blocks, parent or relates; works across projects; cycles are rejected |
| Effort map | `fb_effort` | `action` get, create, update, add_fog, graduate_fog |
| Search and dedup | `fb_search` | `allProjects=true` to search every project |
| Attach files | `fb_artifact` | `kind` adr, spec, questionnaire, research or pr; repo-relative `path` or `url` |

Ticket `type`: research, prototype, grilling, task, slice, bug, enhancement. `mode`: hitl, afk. `triageState`: needs-triage, needs-info, ready-for-agent, ready-for-human, blocked.

## Rules

- Claim before the first write; send the `fence` on every later write.
- Search (`fb_search`) before creating tickets for a topic that may already exist.
- Reference tickets by key or name, never by internal id.

## Velyr conventions for agents

- Repository root: `/Users/lucasbrito/dev/wdt/velyr-core`. Every path in a ticket is relative to it.
- Rust toolchain: `source ~/.cargo/env` before `cargo`. Run only the tests of the touched crate (`cargo test -p velyr-<crate>`); never the whole workspace.
- Decisions live in `docs/adr/` (ADR-009 to ADR-015 cover the engine, threading, conventions, Oxc, config/CLI, server/multicore and conformance). Read the ADRs a ticket references before coding.
- Efforts map 1:1 to ROADMAP milestones (`m0-foundation` ... `m11-advanced`). Track C lives in `m10-native-engine`.
