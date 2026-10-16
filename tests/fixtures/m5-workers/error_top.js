const worker = new Worker(new URL("./throws.js", import.meta.url), { type: "module" });
worker.addEventListener("error", (event) => {
  console.log("error event:", event.message);
  event.preventDefault();
});
