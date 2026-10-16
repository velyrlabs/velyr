// Answers with the id of the server worker that handled the request.
export default {
  fetch(request, info) {
    return new Response(String(info.workerId));
  },
};
