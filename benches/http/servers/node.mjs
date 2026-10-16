// The same three routes as tests/fixtures/m4-acceptance/server.js, on node:http.
import http from "node:http";

const chunk = Buffer.alloc(64, "x");
const port = Number(process.argv[2]);

http
  .createServer((req, res) => {
    const { pathname } = new URL(req.url, "http://x");
    if (pathname === "/plaintext") {
      res.writeHead(200, { "content-type": "text/plain" }).end("Hello, World!");
    } else if (pathname === "/json") {
      res.writeHead(200, { "content-type": "application/json" }).end('{"message":"Hello, World!"}');
    } else if (pathname === "/stream") {
      res.writeHead(200, { "content-type": "application/octet-stream" });
      for (let i = 0; i < 16; i++) res.write(chunk);
      res.end();
    } else {
      res.writeHead(404).end("not found");
    }
  })
  .listen(port, "127.0.0.1");
