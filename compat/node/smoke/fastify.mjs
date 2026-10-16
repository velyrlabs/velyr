import Fastify from 'fastify';

const app = Fastify();
app.get('/hi', async () => ({ ok: true }));
await app.listen({ port: 0, host: '127.0.0.1' });
const { port } = app.server.address();
const res = await fetch(`http://127.0.0.1:${port}/hi`);
const body = await res.json();
await app.close();
if (res.status !== 200 || body.ok !== true) throw new Error(JSON.stringify(body));
console.log('ok');
