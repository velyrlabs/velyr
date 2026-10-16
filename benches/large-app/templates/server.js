// `velyr serve` entry: wraps the runtime neutral handler into the web-standard fetch API.
import { handle } from "./app.js";

export default {
  fetch(request) {
    const result = handle(request.method, request.url);
    return new Response(result.body, {
      status: result.status,
      headers: { "content-type": result.type },
    });
  },
};
