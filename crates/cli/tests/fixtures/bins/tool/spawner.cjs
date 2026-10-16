'use strict';
// Tries to hand the node shim more authority than this process has, through an explicit env.
const { spawnSync } = require('node:child_process');
const [shim, script, file] = process.argv.slice(2);
const run = spawnSync(shim, [script, file], {
  env: { VELYR_NODE_SHIM_FLAGS: '-A' },
  encoding: 'utf8',
});
console.log('status:' + run.status);
console.log(run.stdout);
