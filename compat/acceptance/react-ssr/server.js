// React SSR streaming over `velyr serve`: the shell is flushed first, the Suspense boundary
// resolves after SUSPENSE_DELAY_MS and React streams its content as a later chunk.
import { createElement as h, Suspense } from "react";
import { renderToReadableStream } from "react-dom/server";

const SUSPENSE_DELAY_MS = 150;
const pending = new Map();
let counter = 0;

// Suspense data source: throw the promise until it settles, then return the value once and drop it.
function read(key, load) {
  let entry = pending.get(key);
  if (!entry) {
    entry = { done: false };
    entry.promise = load().then((value) => {
      entry.done = true;
      entry.value = value;
    });
    pending.set(key, entry);
  }
  if (!entry.done) throw entry.promise;
  pending.delete(key);
  return entry.value;
}

function Slow({ requestId }) {
  const value = read(requestId, () => new Promise((resolve) => setTimeout(() => resolve("suspended-content"), SUSPENSE_DELAY_MS)));
  return h("p", { id: "slow" }, value);
}

function App({ requestId }) {
  return h(
    "html",
    { lang: "en" },
    h("head", null, h("title", null, "react-ssr")),
    h(
      "body",
      null,
      h("h1", null, "shell-marker"),
      h(Suspense, { fallback: h("p", { id: "fallback" }, "fallback-marker") }, h(Slow, { requestId })),
    ),
  );
}

export default {
  async fetch(request) {
    if (new URL(request.url).pathname !== "/") return new Response("not found", { status: 404 });
    const stream = await renderToReadableStream(h(App, { requestId: counter++ }));
    return new Response(stream, { headers: { "content-type": "text/html; charset=utf-8" } });
  },
};
