const channel = new BroadcastChannel("m5");
channel.onmessage = (event) => {
  console.log("main got", event.data);
  channel.close();
};
new Worker(new URL("./broadcast_worker.js", import.meta.url), { type: "module" });
