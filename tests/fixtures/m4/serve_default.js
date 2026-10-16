export default {
  fetch(request) {
    return new Response(`hello ${new URL(request.url).pathname}`);
  },
};
