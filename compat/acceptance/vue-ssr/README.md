# vue-ssr acceptance fixture (VELYR-93)

`createSSRApp` + `renderToWebStream` (from `vue/server-renderer`) over `export default { fetch }`,
served by `velyr serve`. No `node_modules` is committed; `velyr.lock` is.

Run: `bash scripts/acceptance.sh vue-ssr` (install --frozen, then `driver.py check`).

`driver.py check` starts the server, does a raw-socket `GET /` and asserts a chunked response where
`shell-marker` arrives at least 50 ms before the async component's `async-content` (150 ms delay).

Unlike React, Vue SSR has no Suspense fallback streaming: the shell is flushed up to the async
component and the rest follows once it resolves, so there is no fallback marker to check.
