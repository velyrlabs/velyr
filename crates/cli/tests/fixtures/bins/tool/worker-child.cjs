'use strict';
const { parentPort, workerData } = require('node:worker_threads');
const { spawnSync } = require('node:child_process');
const { shim, script, file } = workerData;
const run = spawnSync(shim, [script, file], { env: {}, encoding: 'utf8' });
parentPort.postMessage('status:' + run.status + ' ' + run.stdout);
