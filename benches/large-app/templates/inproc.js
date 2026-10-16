// No HTTP: loads the graph and the state, prints READY, then calls the handlers in a loop.
// Works on `velyr run` (V8 and native) and on node, so the engines can be compared where only
// one of them can serve. Output lines: `READY {json}` after the load, `RESULT {json}` at the end.
// INPROC_MS is the time spent per route (default 1500).
import { handle, graphLoadMs, stateMs, registry, state } from "./app.js";

const budgetMs = (typeof process !== "undefined" && process.env && Number(process.env.INPROC_MS)) || 1500;
const now = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

console.log(
  "READY " +
    JSON.stringify({ graphLoadMs: graphLoadMs, stateMs: stateMs, modules: registry.modules, cacheObjects: state.items.length }),
);

// Gives the driver time to read the idle RSS before the loops start.
if (typeof setTimeout === "function") {
  await new Promise((resolve) => setTimeout(resolve, 1500));
}

const routes = {};
for (const path of ["/json", "/transform", "/template"]) {
  let count = 0;
  let bytes = 0;
  const start = now();
  while (now() - start < budgetMs) {
    bytes += handle("GET", path + "?seed=" + (count % 97)).body.length;
    count++;
  }
  const seconds = (now() - start) / 1000;
  routes[path] = { perSec: count / seconds, bytesPerCall: bytes / count };
}
console.log("RESULT " + JSON.stringify(routes));
