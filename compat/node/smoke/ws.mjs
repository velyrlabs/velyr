import { WebSocket, WebSocketServer } from 'ws';

const wss = new WebSocketServer({ port: 0, host: '127.0.0.1' });
await new Promise((resolve) => wss.on('listening', resolve));
wss.on('connection', (socket) => socket.on('message', (data) => socket.send(`echo:${data}`)));
const client = new WebSocket(`ws://127.0.0.1:${wss.address().port}`);
const reply = await new Promise((resolve, reject) => {
  client.on('open', () => client.send('ping'));
  client.on('message', (data) => resolve(data.toString()));
  client.on('error', reject);
});
if (reply !== 'echo:ping') throw new Error(reply);
await new Promise((resolve) => { client.on('close', resolve); client.close(); });
await new Promise((resolve) => wss.close(resolve));
console.log('ok');
