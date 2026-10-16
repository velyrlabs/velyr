// Never answers: only the grace period or a second signal ends it.
export default {
  fetch() {
    console.error("request started");
    return new Promise(() => {});
  },
};
