const worker = new Worker(new URL("./throws.js", import.meta.url), { type: "module" });
worker.onerror = (event) => {
  console.log(event.message, event.filename.endsWith("/throws.js"), event.lineno, event.colno > 0);
  event.preventDefault();
};
