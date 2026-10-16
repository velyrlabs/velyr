# Compatibility matrix template

Machine-generated status should replace placeholders.

| Area | Target | Status | Evidence |
|---|---|---|---|
| ECMAScript | ECMA-262 / Test262 | Planned | Test262 runner |
| Web APIs | Minimum Common Web API | Planned | WPT manifest |
| ESM | Velyr native | Planned | module suite |
| CommonJS | Node compatibility | Planned | compat suite |
| package.json exports | Node semantics | Planned | resolver suite |
| npm registry | package consumption | Planned | install corpus |
| Node core | per module | Planned | module suites |
| Node-API | versioned subset | Planned | addon corpus |
| React | current supported target | Planned | acceptance app |
| Vue | current supported target | Planned | acceptance app |
| Vite | current supported target | Planned | acceptance app |

## Rule

Never edit a `Supported` status by hand. CI/release automation owns supported evidence.
