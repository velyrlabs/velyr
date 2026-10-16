// GET /slow answers after 300 ms; GET /stop aborts the signal while /slow is still in flight.
const controller = new AbortController();
const server = Velyr.serve(
  {
    hostname: "127.0.0.1",
    port: 0,
    signal: controller.signal,
    onListen: ({ port }) => console.log(`listening ${port}`),
  },
  async (request) => {
    const { pathname } = new URL(request.url);
    if (pathname === "/stop") {
      controller.abort();
      return new Response("stopping");
    }
    await new Promise((resolve) => setTimeout(resolve, 300));
    console.log("slow done");
    return new Response("slow");
  },
);
await server.finished;
console.log("finished");
