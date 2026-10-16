const url = new URL("./query_worker.js", import.meta.url);
const ask = (options) =>
  new Promise((resolve) => {
    const worker = new Worker(url, options);
    worker.onmessage = (event) => {
      resolve(event.data);
      worker.terminate();
    };
  });
console.log("inherit", await ask({ type: "module" }));
console.log("narrow", await ask({ velyr: { permissions: { "allow-env": ["HOME"] } } }));
console.log("wider", await ask({ velyr: { permissions: { "allow-all": true } } }));
