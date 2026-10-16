'use strict';
// node:fs/promises. Written for this runtime; the operations come from internal/fs/ops.
const { promises, execAsync, plans } = require('internal/fs/ops');
const { FileHandle } = require('internal/fs/handle');
const { constants } = require('internal/fs/stats');
const { watchPromise } = require('internal/fs/watch');

// read, write, close and fstat live on FileHandle, not on the module.
const { read, write, close, fstat, open, ...operations } = promises;

module.exports = {
  ...operations,
  constants,
  watch: watchPromise,
  async open(file, flags, mode) {
    return new FileHandle(await execAsync(plans.open(file, flags, mode)));
  },
};
