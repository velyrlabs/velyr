# Frontend ecosystem compatibility

## Principle

Velyr does not implement React or Vue compatibility as framework-specific shims.

Compatibility emerges from:
- ECMAScript;
- ESM;
- package resolution;
- Web APIs;
- Node compatibility where tooling requires it.

## React

Acceptance scenarios:
- React package import;
- JSX transform;
- React DOM client build;
- server rendering;
- Web Streams SSR path;
- Vite React development/build.

## Vue

Acceptance scenarios:
- Vue package import;
- Composition API;
- Vue SFC compilation through `vue/compiler-sfc`;
- Vite Vue plugin flow;
- SSR/Web Streams where applicable.

## Vite

Vite is a strategic ecosystem benchmark because it exercises:
- filesystem;
- path;
- package resolution;
- ESM;
- watchers;
- child/native tooling;
- WebSockets/HMR;
- transforms;
- source maps.

Vite currently documents npm/Yarn/pnpm/Bun/Deno usage, demonstrating a compatibility surface broader than a single runtime.

## TypeScript

TypeScript syntax is transformed before engine execution. Type checking may be a separate tool/process/library and is not part of ECMAScript VM semantics.

## Later targets
- Next.js
- Nuxt
- Vitest
- popular ORMs/database clients
- NestJS
