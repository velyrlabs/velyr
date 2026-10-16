// The first start fails once, a marker file in the working directory remembers it, so the
// restarted worker (and every later one) serves.
let first = false;
try {
  await Velyr.readTextFile("crash-once.marker");
} catch {
  first = true;
}
if (first) {
  await Velyr.writeTextFile("crash-once.marker", "crashed");
  throw new Error("worker crash while starting");
}

export default {
  fetch(request, info) {
    return new Response(String(info.workerId));
  },
};
