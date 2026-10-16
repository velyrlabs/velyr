# Primary references

These references form the initial standards and implementation study set. Pin exact versions/commits in CI where test suites are imported.

## ECMAScript
- ECMA-262: https://tc39.es/ecma262/
- Test262: https://github.com/tc39/test262

## Web interoperability
- Minimum Common Web API / WinterTC TC55: https://min-common-api.proposal.wintertc.org/
- Web Platform Tests: https://web-platform-tests.org/
- WHATWG Fetch: https://fetch.spec.whatwg.org/
- WHATWG Streams: https://streams.spec.whatwg.org/
- WHATWG URL: https://url.spec.whatwg.org/

## Node compatibility
- Node packages/modules: https://nodejs.org/api/packages.html
- Node-API: https://nodejs.org/api/n-api.html
- Node compatibility test ecosystem / CITGM: https://github.com/nodejs/citgm

## Rust/runtime
- Cargo features: https://doc.rust-lang.org/cargo/reference/features.html
- Tokio: https://tokio.rs/
- Hyper: https://hyper.rs/
- rustls: https://rustls.dev/
- RustSec Advisory DB: https://github.com/RustSec/advisory-db

## Security reference
- Deno permissions: https://docs.deno.com/runtime/reference/permissions/
- Deno security model: https://docs.deno.com/runtime/fundamentals/security/

## Engine references
- Boa: https://boajs.dev/
- V8: https://v8.dev/
- SpiderMonkey: https://spidermonkey.dev/
- JavaScriptCore: https://docs.webkit.org/Deep%20Dive/JSC/JavaScriptCore.html
- Hermes: https://hermesengine.dev/

## WebAssembly plugins
- Wasmtime: https://docs.wasmtime.dev/
- WebAssembly Component Model: https://component-model.bytecodealliance.org/

## Tooling/frontend
- Oxc: https://oxc.rs/
- Vite: https://vite.dev/
- Vue tooling/compiler-sfc: https://vuejs.org/guide/scaling-up/tooling
- React: https://react.dev/

## Study rules

1. Standards define compatibility contracts.
2. Existing runtimes are implementation references, not normative specifications.
3. Never copy an implementation assumption into Velyr without verifying licensing, semantics and architectural fit.
4. Imported compatibility/conformance suites must be version-pinned and reproducible.
