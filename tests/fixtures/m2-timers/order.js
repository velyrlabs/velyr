setTimeout(() => console.log(2), 5); queueMicrotask(() => console.log(1)); console.log(0);
