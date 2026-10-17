// Vue SSR streaming over `velyr serve`: the shell is flushed first, the async component
// resolves after ASYNC_DELAY_MS and renderToWebStream streams its markup as a later chunk.
import { createSSRApp, defineAsyncComponent, h } from "vue";
import { renderToWebStream } from "vue/server-renderer";

const ASYNC_DELAY_MS = 150;

const Slow = defineAsyncComponent(
  () => new Promise((resolve) => setTimeout(() => resolve({ render: () => h("p", { id: "slow" }, "async-content") }), ASYNC_DELAY_MS)),
);

const App = { render: () => h("main", null, [h("h1", null, "shell-marker"), h(Slow)]) };

export default {
  async fetch(request) {
    if (new URL(request.url).pathname !== "/") return new Response("not found", { status: 404 });
    return new Response(renderToWebStream(createSSRApp(App)), { headers: { "content-type": "text/html; charset=utf-8" } });
  },
};
