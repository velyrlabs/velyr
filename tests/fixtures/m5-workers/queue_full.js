const { port1, port2 } = new MessageChannel();
let sent = 0;
try {
  while (sent < 20000) {
    port1.postMessage(sent);
    sent++;
  }
} catch (error) {
  console.log(error.code, sent);
}
port1.close();
port2.close();
