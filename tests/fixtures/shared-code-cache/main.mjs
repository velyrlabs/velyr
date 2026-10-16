// Starts two workers one after the other; both import the same small graph.
const results = [];
for (let i = 0; i < 2; i++) {
  const worker = new Worker(new URL("./worker.mjs", import.meta.url), { type: "module" });
  results.push(await new Promise((resolve) => (worker.onmessage = (event) => resolve(event.data))));
  worker.terminate();
}
console.log(results.join(","));
