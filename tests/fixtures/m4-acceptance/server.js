// Web-standard server for the M4 acceptance test and the HTTP benchmarks (`velyr serve`).
// GET /plaintext, /json and /stream (16 chunks of 64 bytes); /redirect goes to another host name.
const encoder = new TextEncoder();
const chunk = encoder.encode("x".repeat(64));

export default {
  fetch(request) {
    const url = new URL(request.url);
    switch (url.pathname) {
      case "/plaintext":
        return new Response("Hello, World!", { headers: { "content-type": "text/plain" } });
      case "/json":
        return Response.json({ message: "Hello, World!" });
      case "/stream": {
        let sent = 0;
        const body = new ReadableStream({
          pull(controller) {
            controller.enqueue(chunk);
            if (++sent === 16) controller.close();
          },
        });
        return new Response(body, { headers: { "content-type": "application/octet-stream" } });
      }
      case "/redirect":
        return Response.redirect(`http://localhost:${url.port}/plaintext`, 302);
      default:
        return new Response("not found", { status: 404 });
    }
  },
};
