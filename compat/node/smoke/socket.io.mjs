// The client package is not installed, so the check is the engine.io polling handshake over HTTP.
import http from 'node:http';
import { Server } from 'socket.io';

const httpServer = http.createServer();
const io = new Server(httpServer);
io.on('connection', () => {});
await new Promise((resolve) => httpServer.listen(0, '127.0.0.1', resolve));
try {
  const url = `http://127.0.0.1:${httpServer.address().port}/socket.io/?EIO=4&transport=polling`;
  const res = await fetch(url);
  const text = await res.text();
  if (res.status !== 200 || !text.startsWith('0{') || !text.includes('"sid"')) throw new Error(`${res.status} ${text}`);
  console.log('ok');
} finally {
  await io.close();
}
