const controller = new AbortController();
console.log(controller.signal.aborted === false);
