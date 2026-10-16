'use strict';
// node:fs value types: constants, Stats, Dirent and Dir. Written for this runtime.
//
// Known limits: Stats has no BigInt form; Dir is a snapshot taken by opendir (one readdir), not a
// streaming handle, so entries created after opendir are not seen.
const errors = require('internal/errors');

const S_IFMT = 0o170000;
const constants = Object.freeze({
  UV_FS_SYMLINK_DIR: 1,
  UV_FS_SYMLINK_JUNCTION: 2,
  F_OK: 0,
  R_OK: 4,
  W_OK: 2,
  X_OK: 1,
  UV_FS_COPYFILE_EXCL: 1,
  COPYFILE_EXCL: 1,
  UV_FS_COPYFILE_FICLONE: 2,
  COPYFILE_FICLONE: 2,
  UV_FS_COPYFILE_FICLONE_FORCE: 4,
  COPYFILE_FICLONE_FORCE: 4,
  S_IFMT,
  S_IFREG: 0o100000,
  S_IFDIR: 0o040000,
  S_IFCHR: 0o020000,
  S_IFBLK: 0o060000,
  S_IFIFO: 0o010000,
  S_IFLNK: 0o120000,
  S_IFSOCK: 0o140000,
});

const FIELDS = ['dev', 'mode', 'nlink', 'uid', 'gid', 'rdev', 'blksize', 'ino', 'size', 'blocks', 'atimeMs', 'mtimeMs', 'ctimeMs', 'birthtimeMs'];

class Stats {
  /** `parts` are the 14 decimal-text fields of the `stat` ops, in `FIELDS` order. */
  constructor(parts) {
    FIELDS.forEach((field, index) => { this[field] = Number(parts[index].toString()); });
    this.atime = new Date(this.atimeMs);
    this.mtime = new Date(this.mtimeMs);
    this.ctime = new Date(this.ctimeMs);
    this.birthtime = new Date(this.birthtimeMs);
  }

  isFile() { return (this.mode & S_IFMT) === constants.S_IFREG; }
  isDirectory() { return (this.mode & S_IFMT) === constants.S_IFDIR; }
  isSymbolicLink() { return (this.mode & S_IFMT) === constants.S_IFLNK; }
  isFIFO() { return (this.mode & S_IFMT) === constants.S_IFIFO; }
  isSocket() { return (this.mode & S_IFMT) === constants.S_IFSOCK; }
  isBlockDevice() { return (this.mode & S_IFMT) === constants.S_IFBLK; }
  isCharacterDevice() { return (this.mode & S_IFMT) === constants.S_IFCHR; }
}

// `type` is the letter of the readdir op: f d l p s b c, anything else is unknown.
class Dirent {
  constructor(name, type, parentPath) {
    this.name = name;
    this.parentPath = parentPath;
    this.path = parentPath;
    Object.defineProperty(this, '_type', { value: type });
  }

  isFile() { return this._type === 'f'; }
  isDirectory() { return this._type === 'd'; }
  isSymbolicLink() { return this._type === 'l'; }
  isFIFO() { return this._type === 'p'; }
  isSocket() { return this._type === 's'; }
  isBlockDevice() { return this._type === 'b'; }
  isCharacterDevice() { return this._type === 'c'; }
}

const closedError = () => errors.make(Error, 'ERR_DIR_CLOSED', 'Directory handle was closed');

class Dir {
  constructor(path, entries) {
    Object.defineProperty(this, '_state', { value: { entries, closed: false } });
    Object.defineProperty(this, 'path', { value: path, enumerable: true });
  }

  readSync() {
    if (this._state.closed) throw closedError();
    return this._state.entries.shift() || null;
  }

  read(callback) {
    if (typeof callback !== 'function') {
      return new Promise((resolve, reject) => {
        try { resolve(this.readSync()); } catch (error) { reject(error); }
      });
    }
    let entry; let error = null;
    try { entry = this.readSync(); } catch (caught) { error = caught; }
    require('process').nextTick(callback, error, entry);
  }

  closeSync() {
    if (this._state.closed) throw closedError();
    this._state.closed = true;
  }

  close(callback) {
    if (typeof callback !== 'function') {
      return new Promise((resolve, reject) => {
        try { this.closeSync(); resolve(); } catch (error) { reject(error); }
      });
    }
    let error = null;
    try { this.closeSync(); } catch (caught) { error = caught; }
    require('process').nextTick(callback, error);
  }

  async *entries() {
    try {
      for (let entry = this.readSync(); entry !== null; entry = this.readSync()) yield entry;
    } finally {
      if (!this._state.closed) this.closeSync();
    }
  }

  [Symbol.asyncIterator]() { return this.entries(); }
}

module.exports = { constants, Stats, Dirent, Dir };
