self.onmessage = (event) => {
  self.postMessage(event.data * event.data);
};
