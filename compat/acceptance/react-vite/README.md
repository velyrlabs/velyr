# react-vite acceptance fixture (VELYR-91)

Minimal app from the `create-vite` react-ts template. No `node_modules` and no `dist/` are
committed; `velyr.lock` is.

Run: `bash scripts/acceptance.sh react-vite` (install --frozen, `velyr x vite build`, dev server plus
HTTP requests to `/` and `/src/main.tsx`).

## Bundler path

The template's Vite 8 uses Rolldown (N-API, needs `--allow-native`, VELYR-96): not usable yet.
Vite 6/7 use Rollup 4, whose native `.node` addon is also N-API. The WASM fallback needs `overrides`
or `npm:` aliases, which `velyr install` does not support (`VELYR_UNSUPPORTED: unsupported
dependency spec npm:@rollup/wasm-node@...`).

So the fixture pins `vite ~4.5.14` (esbuild 0.18 as a spawned binary, run with `--allow-run`;
pure-JS Rollup 3) and `@vitejs/plugin-react ^4.7`. No N-API is involved.

## Current status: blocked

`velyr x vite build` fails before running Vite:

    Uncaught (in promise) VELYR_UNSUPPORTED: node: modules need Node compatibility, which is not
    linked: node:readline

Vite imports `node:readline` (`vite/dist/node/cli.js`, `index.js`, `chunks/dep-*.js`) and Rollup 3
imports `node:vm`. Neither builtin exists in the `crates/compat-node` builtin table (the message is
misleading: Node compat is linked, the module is just missing). Blockers beyond those two are
unknown until they are implemented. Once they are, re-run the script and, if green, drop this
section.
