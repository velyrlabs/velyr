// GET /slow takes 400 ms; "request started" is the cue that the handler is running.
export default {
  async fetch(request) {
    console.error("request started");
    await new Promise((resolve) => setTimeout(resolve, 400));
    return new Response("done");
  },
};
