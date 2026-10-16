const worker = new Worker(new URL("./idle.js", import.meta.url), { type: "module" });
worker.onmessage = (event) => console.log("message", event.data);
Velyr.unrefWorker(worker);
setTimeout(() => console.log("timer done"), 300);
console.log("main done");
