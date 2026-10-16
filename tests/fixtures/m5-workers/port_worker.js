self.onmessage = (event) => {
  const port = event.data.port;
  port.postMessage(`hello from worker ${event.data.tag} ${event.ports[0] === port}`);
  port.close();
};
