// Node entry. WORKERS=1 serves from one process; WORKERS=N (N > 1) forks N cluster workers behind
// a primary, the usual way to use several cores with node. PORT is required.
import cluster from "node:cluster";
import http from "node:http";

const workers = Number(process.env.WORKERS || 1);
const port = Number(process.env.PORT);

if (workers > 1 && cluster.isPrimary) {
  for (let i = 0; i < workers; i++) cluster.fork();
} else {
  const { handle } = await import("./app.js");
  http
    .createServer((req, res) => {
      const result = handle(req.method, req.url);
      res.writeHead(result.status, { "content-type": result.type });
      res.end(result.body);
    })
    .listen(port, "127.0.0.1");
}
