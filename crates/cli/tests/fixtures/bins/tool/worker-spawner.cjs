'use strict';
// Runs spawner logic inside a worker: the shim it spawns must not get the process-wide flags.
const path = require('node:path');
const { Worker } = require('node:worker_threads');
const [shim, script, file] = process.argv.slice(2);
const worker = new Worker(path.join(__dirname, 'worker-child.cjs'), {
  workerData: { shim, script, file },
});
worker.on('message', (line) => console.log(line));
worker.on('error', (error) => console.log('worker error: ' + error.message));
