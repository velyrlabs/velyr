Promise.resolve().then(() => console.log("promise"));
process.nextTick(() => console.log("tick"));
setImmediate(() => console.log("immediate"));
console.log("sync", typeof process.argv[0]);
