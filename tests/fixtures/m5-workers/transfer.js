const { port1, port2 } = new MessageChannel();
const worker = new Worker(new URL("./port_worker.js", import.meta.url), { type: "module" });
port1.onmessage = (event) => {
  console.log("direct:", event.data);
  port1.close();
  worker.terminate();
};
worker.postMessage({ port: port2, tag: "x" }, [port2]);
console.log("neutered:", port2.postMessage("late") === undefined);
