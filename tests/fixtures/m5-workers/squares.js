const worker = new Worker(new URL("./square.js", import.meta.url), { type: "module" });
const results = [];
worker.onmessage = (event) => {
  results.push(event.data);
  if (results.length === 5) {
    console.log(results.join(","));
    worker.terminate();
  }
};
for (const n of [1, 2, 3, 4, 5]) worker.postMessage(n);
