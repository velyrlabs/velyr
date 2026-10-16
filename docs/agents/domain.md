# Domain docs

How skills consume this repo's domain documentation. Conventions live in the `domain-modeling` skill.

## Layout: single-context

Single-context (default):

```
/
  GLOSSARY.md
  docs/adr/ADR-NNN-slug.md
```

Multi-context (a `GLOSSARY-MAP.md` at the root points to one `GLOSSARY.md` per context; context-specific ADRs sit in `<context>/docs/adr/`, system-wide ones in the root `docs/adr/`).

## Before exploring

- Read `GLOSSARY.md` (or `GLOSSARY-MAP.md` and the glossaries relevant to the topic).
- Read the ADRs that touch the area you will change.
- If a file does not exist, proceed silently. `domain-modeling` creates them lazily.

## Rules

- Use the glossary's canonical terms in tickets, specs, tests and code; avoid the synonyms listed under `_Avoid_`.
- If the output contradicts an ADR, say so explicitly instead of overriding it.
