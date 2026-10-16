import Koa from 'koa';

const app = new Koa();
app.use((ctx) => { ctx.body = { ok: true, path: ctx.path }; });
const server = app.listen(0, '127.0.0.1', async () => {
  try {
    const { port } = server.address();
    const res = await fetch(`http://127.0.0.1:${port}/hi`);
    const body = await res.json();
    if (res.status !== 200 || body.ok !== true || body.path !== '/hi') throw new Error(JSON.stringify(body));
    server.close(() => console.log('ok'));
  } catch (err) {
    console.error(err);
    process.exit(1);
  }
});
