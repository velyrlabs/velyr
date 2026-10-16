// Echoes the request as JSON; GET /boom throws; GET /stop shuts the server down.
const server = Velyr.serve(
  { hostname: "127.0.0.1", port: 0, onListen: ({ port }) => console.log(`listening ${port}`) },
  async (request, info) => {
    const url = new URL(request.url);
    if (url.pathname === "/stop") {
      server.shutdown();
      return new Response("bye");
    }
    if (url.pathname === "/boom") throw new Error("handler failed");
    if (url.pathname === "/bad") return "not a response";
    const body = request.method === "POST" ? await request.text() : null;
    return Response.json({
      method: request.method,
      target: url.pathname + url.search,
      body,
      remote: info.remoteAddr.hostname,
      addr: server.addr.port > 0,
    });
  },
);
await server.finished;
console.log("finished");
