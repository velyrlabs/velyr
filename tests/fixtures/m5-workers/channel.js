const { port1, port2 } = new MessageChannel();
port2.onmessage = (event) => {
  console.log("got", JSON.stringify(event.data), event.ports.length);
  port1.close();
  port2.close();
};
port1.postMessage({ a: 1, list: [1, 2] });
