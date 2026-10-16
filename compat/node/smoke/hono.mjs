import { Hono } from 'hono';

const app = new Hono();
app.get('/hi/:name', (c) => c.json({ ok: true, name: c.req.param('name') }));
app.post('/echo', async (c) => c.text(await c.req.text()));
const res = await app.request('/hi/velyr');
const body = await res.json();
if (res.status !== 200 || body.name !== 'velyr') throw new Error(JSON.stringify(body));
const echo = await app.request('/echo', { method: 'POST', body: 'payload' });
if ((await echo.text()) !== 'payload') throw new Error('echo mismatch');
if ((await app.request('/missing')).status !== 404) throw new Error('expected 404');
console.log('ok');
