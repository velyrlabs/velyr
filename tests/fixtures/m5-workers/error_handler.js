const worker = new Worker(new URL("./throws_in_handler.js", import.meta.url), { type: "module" });
worker.onerror = (event) => {
  console.log("error event:", event.message);
  event.preventDefault();
  worker.terminate();
};
worker.postMessage("go");
