# vue-vite acceptance fixture (VELYR-93)

Minimal app in the style of the `create-vite` vue-ts template (`App.vue` + one child component with
`<script setup lang="ts">` and a scoped style). No `node_modules` and no `dist/` are committed;
`velyr.lock` is.

Run: `bash scripts/acceptance.sh vue-vite` (install --frozen, `velyr x vite build`, dev server plus
HTTP requests to `/`, `/src/main.ts` and `/src/App.vue`, then an HMR check).

SFCs are compiled by `@vue/compiler-sfc` inside `@vitejs/plugin-vue`; the runtime has no native
`.vue` support and does not need one.

## Bundler path

Same as react-vite: `vite ~4.5.14` (esbuild as a spawned binary, pure-JS Rollup 3), no N-API.
`@vitejs/plugin-vue ^4.6` is the last line that supports Vite 4. `vue` is loaded through its CJS
entry, which re-exports with `Object.keys(...).forEach` (named exports detected by VELYR-225).

## Status: green

`install --frozen`, `velyr x vite build` (checks `dist/index.html`, the JS bundle and `#app`), then
the dev server: `/src/main.ts` must be transformed JS importing Vue, and `/src/App.vue` must come
back compiled (`_sfc_main`, no raw `<template`). HMR: a `python3 -I` WebSocket client connects
(`vite-hmr` subprotocol), appends a second `<script>` block to `src/App.vue` in the temp copy and
requires an `update` (or `full-reload`) for `/src/App.vue` within 5 s.

`--allow-run` is scoped to the installed esbuild binary. The script runs on a temp copy of the
fixture, kills the dev server on exit, and prefers `target/release/velyr`, then `target/debug/velyr`;
override with `VELYR=<path>`.
