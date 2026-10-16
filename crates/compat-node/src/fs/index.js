'use strict';
// node:fs entry point. Written for this runtime; no Node source is copied.
//
// Built on the ops in fs_ops.rs (sync, callback and promise forms from one plan table, see
// ops.js). Every call authorizes FsRead/FsWrite on the canonical path in Rust; a denial is an
// Error with code ERR_ACCESS_DENIED whose `cause` has code VELYR_PERMISSION_DENIED. fs.watch,
// fs.watchFile and the link/chown/truncate/fsync family are not provided.
const ops = require('internal/fs/ops');
const { constants, Stats, Dirent, Dir } = require('internal/fs/stats');
const streams = require('internal/fs/streams');

const fs = {
  ...ops.sync,
  ...ops.callback,
  constants,
  Stats,
  Dirent,
  Dir,
  ReadStream: streams.ReadStream,
  WriteStream: streams.WriteStream,
  createReadStream: streams.createReadStream,
  createWriteStream: streams.createWriteStream,
};

Object.defineProperty(fs, 'promises', { enumerable: true, configurable: true, get: () => require('fs/promises') });

module.exports = fs;
