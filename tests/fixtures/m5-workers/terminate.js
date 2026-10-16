const worker = new Worker(new URL("./busy.js", import.meta.url), { type: "module" });
worker.onmessage = (event) => {
  console.log("worker said", event.data);
  worker.terminate();
  console.log("terminated");
};
