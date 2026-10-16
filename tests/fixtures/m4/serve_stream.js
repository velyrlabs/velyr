// GET /stream sends "a", waits for GET /release, then sends "b" and, after a short delay, "c".
let release;
const released = new Promise((resolve) => (release = resolve));
const server = Velyr.serve(
  { hostname: "127.0.0.1", port: 0, onListen: ({ port }) => console.log(`listening ${port}`) },
  (request) => {
    const { pathname } = new URL(request.url);
    if (pathname === "/release") {
      release();
      return new Response("released");
    }
    if (pathname === "/stop") {
      server.shutdown();
      return new Response("bye");
    }
    const body = new ReadableStream({
      async start(controller) {
        const encoder = new TextEncoder();
        controller.enqueue(encoder.encode("a"));
        await released;
        controller.enqueue(encoder.encode("b"));
        await new Promise((resolve) => setTimeout(resolve, 50));
        controller.enqueue(encoder.encode("c"));
        controller.close();
      },
    });
    return new Response(body, { headers: { "content-type": "text/plain" } });
  },
);
await server.finished;
