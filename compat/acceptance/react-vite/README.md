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

## Status: green

`bash scripts/acceptance.sh react-vite` passes: `install --frozen`, `velyr x vite build` (checks
`dist/index.html`, the JS bundle and `#root`), then the dev server with a GET on `/` and on
`/src/main.tsx` (must be transformed JS importing React). Then HMR: a stdlib `python3 -I` WebSocket client
connects to the dev server (`vite-hmr` subprotocol), appends a comment to `src/App.tsx` in the
temp copy and requires an `update` (or `full-reload`) for `/src/App.tsx` within 5 s.

The script runs on a temp copy of the fixture and kills the dev server on exit. It prefers
`target/release/velyr`, then `target/debug/velyr`; override with `VELYR=<path>`. The binary needs the
VELYR-214 (`node:readline`, `node:vm`), VELYR-218 (async WebAssembly) and VELYR-219 (`unref`) fixes.

Permissions: `--allow-run` is scoped to the installed esbuild binary
(`node_modules/@esbuild/<platform>/bin/esbuild`, resolved by the script after install).
