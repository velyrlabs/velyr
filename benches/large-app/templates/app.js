// The application: loads the whole module graph, builds the live state, and answers requests.
// `handle(method, url)` is runtime neutral; server.js (velyr), node-server.mjs (node) and
// inproc.js (velyr run / node, no HTTP) wrap it. Plain ES2015 syntax on purpose, so the native
// engine can run it too.
import { config } from "./config.js";

const now = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

const loadStart = now();
const graph = await import("./src/index.js");
export const graphLoadMs = now() - loadStart;

export const registry = { modules: 0, transforms: [], formatters: [], scorers: [], edges: [] };
graph.registerAll(registry);

const COLORS = ["red", "green", "blue", "amber", "violet", "teal", "grey", "pink"];

function buildState(count) {
  const cache = new Map();
  const items = [];
  let x = 12345;
  for (let i = 0; i < count; i++) {
    x = (Math.imul(x, 1103515245) + 12345) >>> 0;
    const item = {
      id: "item-" + i,
      index: i,
      name: "Item " + x.toString(36),
      price: (x % 100000) / 100,
      tags: ["t" + (x % 17), "t" + ((x >> 3) % 31), "t" + ((x >> 7) % 53)],
      attrs: { color: COLORS[x % 8], size: x % 5, stock: x % 1000, updated: 1700000000 + (x % 10000000) },
      note: "n".repeat(20 + (x % 40)) + i,
    };
    cache.set(item.id, item);
    items.push(item);
  }
  return { cache, items };
}

const stateStart = now();
export const state = buildState(config.cacheObjects);
export const stateMs = now() - stateStart;
const startedAt = Date.now();

function pick(list, index) {
  return list[index % list.length];
}

function memory() {
  if (typeof process === "undefined" || typeof process.memoryUsage !== "function") return null;
  try {
    return process.memoryUsage();
  } catch (e) {
    return null;
  }
}

function escapeHtml(text) {
  return String(text)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function syntheticText(seed) {
  const words = ["alpha", "beta", "gamma", "delta", "omega", "sigma", "kappa", "theta", "lambda", "zeta"];
  let x = (seed * 2654435761) >>> 0;
  const out = [];
  for (let i = 0; i < 400; i++) {
    x = (Math.imul(x, 1664525) + 1013904223) >>> 0;
    out.push(words[x % words.length] + (x % 97));
  }
  return out.join(" ");
}

function json(seed) {
  const items = state.items;
  const start = (seed * 7919) % items.length;
  const out = [];
  for (let k = 0; k < config.jsonItems; k++) {
    const item = items[(start + k * 13) % items.length];
    out.push({
      id: item.id,
      name: item.name,
      price: item.price,
      tags: item.tags,
      attrs: item.attrs,
      note: item.note,
      label: pick(registry.formatters, item.index + seed)(item),
      score: pick(registry.scorers, item.index * 3 + seed)(item),
    });
  }
  return JSON.stringify({ seed: seed, count: out.length, items: out });
}

function transform(seed) {
  let text = syntheticText(seed);
  for (let step = 0; step < config.transformSteps; step++) {
    text = pick(registry.transforms, seed * 31 + step * 97).fn(text);
  }
  const freq = new Map();
  for (const word of text.split(" ")) {
    if (word) freq.set(word, (freq.get(word) || 0) + 1);
  }
  const top = Array.from(freq.entries())
    .sort((a, b) => b[1] - a[1] || (a[0] < b[0] ? -1 : 1))
    .slice(0, 20);
  return JSON.stringify({ seed: seed, length: text.length, distinct: freq.size, top: top });
}

function template(seed) {
  const items = state.items;
  const start = (seed * 104729) % items.length;
  const rows = [];
  for (let k = 0; k < config.templateRows; k++) {
    const item = items[(start + k * 7) % items.length];
    const label = pick(registry.formatters, item.index + k)(item);
    rows.push(
      "<tr class=\"row-" + (k % 2) + "\"><td>" + escapeHtml(item.id) + "</td><td>" + escapeHtml(label) +
        "</td><td>" + item.price.toFixed(2) + "</td><td>" + escapeHtml(item.tags.join(", ")) +
        "</td><td style=\"color:" + item.attrs.color + "\">" + escapeHtml(item.note) + "</td></tr>",
    );
  }
  return "<!doctype html><html><head><title>Items " + seed + "</title></head><body><h1>Items from " + start +
    "</h1><table>" + rows.join("") + "</table></body></html>";
}

function stats() {
  return JSON.stringify({
    modules: registry.modules,
    edges: registry.edges.length,
    cacheObjects: state.items.length,
    graphLoadMs: graphLoadMs,
    stateMs: stateMs,
    uptimeMs: Date.now() - startedAt,
    memory: memory(),
  });
}

// `URL` is a web API: the native engine has none, so the path and `seed` are cut by hand there.
function parseRequest(url) {
  const path = url.replace(/^[a-z]+:\/\/[^/]*/, "");
  const [pathname, query = ""] = path.split("?");
  const match = /(?:^|&)seed=(\d+)/.exec(query);
  return { pathname: pathname, seed: match ? Number(match[1]) : 0 };
}

/** Pure request handler: `url` may be absolute or a path with a query. */
export function handle(method, url) {
  const parsed = parseRequest(url);
  const seed = parsed.seed || 1;
  switch (parsed.pathname) {
    case "/health":
      return { status: 200, type: "text/plain", body: "ok" };
    case "/stats":
      return { status: 200, type: "application/json", body: stats() };
    case "/json":
      return { status: 200, type: "application/json", body: json(seed) };
    case "/transform":
      return { status: 200, type: "application/json", body: transform(seed) };
    case "/template":
      return { status: 200, type: "text/html; charset=utf-8", body: template(seed) };
    default:
      return { status: 404, type: "text/plain", body: "not found" };
  }
}
