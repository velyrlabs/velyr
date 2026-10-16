// Adapted from nodejs/node test/parallel/test-process-exit-code.js and test-process-beforeexit.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

const events = [];
process.on('beforeExit', (code) => {
  if (events.includes('beforeExit')) return;
  events.push('beforeExit');
  assert.strictEqual(code, 3);
  setImmediate(() => events.push('revived'));
});
process.on('exit', (code) => {
  events.push('exit');
  assert.strictEqual(code, 3);
  assert.deepStrictEqual(events, ['beforeExit', 'revived', 'exit']);
  // exit with a clean code once every assertion has passed: the runner expects 0
  process.exitCode = 0;
});
process.exitCode = 3;
assert.strictEqual(process.exitCode, 3);
