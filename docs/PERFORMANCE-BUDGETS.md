# Performance budgets

Budgets start as provisional and are calibrated on dedicated runners.

## Merge budget proposal

| Metric | Regression threshold |
|---|---:|
| HTTP throughput | >3% slower |
| P99 HTTP latency | >5% worse |
| idle RSS | >3% worse |
| loaded RSS | >3% worse |
| cold startup | >5% worse |
| worker startup | >5% worse |
| module graph load | >5% worse |

A threshold breach requires:
- rerun;
- noise analysis;
- issue/PR note;
- explicit approval if intentional.

## Product targets

Do not freeze marketing targets before baseline data. Once Phase 4 exists, set targets from measured Node/Deno/Bun and Velyr results on dedicated machines.
