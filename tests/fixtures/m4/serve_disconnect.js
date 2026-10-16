// Waits for the client to leave and reports what request.signal says.
export default {
  async fetch(request) {
    console.error("request started");
    await new Promise((resolve) => request.signal.addEventListener("abort", resolve, { once: true }));
    console.error(`signal aborted=${request.signal.aborted}`);
    return new Response("gone");
  },
};
