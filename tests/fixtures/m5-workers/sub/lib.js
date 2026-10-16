export function start(onMessage) {
  const worker = new Worker("./w.js");
  worker.onmessage = (event) => {
    onMessage(event.data);
    worker.terminate();
  };
}
