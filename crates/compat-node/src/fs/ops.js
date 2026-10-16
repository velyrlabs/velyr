'use strict';
// node:fs operations. Written for this runtime; no Node source is copied.
//
// Every operation is a "plan": it validates the arguments (throwing synchronously, in all three
// forms) and returns { name, args, finish } for a Rust op in fs_ops.rs. One wrapper then makes
// the sync form (blocks the isolate thread), the callback form and the promise form from it, so
// the three cannot drift apart. Authorization happens in Rust, on the canonical path.
//
// Known limits: readFile(fd) is not supported; open flags are strings only; access() is
// approximated from permission bits; symlink() ignores `type`; writes through a descriptor are
// always complete (no short writes); no fsync/truncate/readv/writev (watch is in watch.js).
const errors = require('internal/errors');
const { Buffer } = require('buffer');
const path = require('path');
const util = require('internal/fs/util');
const { Stats, Dirent, Dir } = require('internal/fs/stats');

const { run, runAsync, toPath, toFd, getOptions, toData, toMode, toMillis } = util;

const decode = (buffer, encoding) => (encoding && encoding !== 'buffer' ? buffer.toString(encoding) : buffer);
const text = ([part]) => part.toString();

function pairs(parts) {
  const out = [];
  for (let i = 0; i < parts.length; i += 2) out.push([parts[i].toString(), parts[i + 1].toString()]);
  return out;
}

const plans = {
  readFile(file, options) {
    const { encoding } = getOptions(options);
    return { name: 'readFile', args: [toPath(file)], finish: ([data]) => decode(data, encoding) };
  },

  writeFile(file, data, options) {
    const { encoding, mode, flag } = getOptions(options, { flag: 'w' });
    const bytes = toData(data, encoding);
    if (typeof file === 'number') return { name: 'write', args: [toFd(file), bytes, -1], finish: () => undefined };
    return { name: 'writeFile', args: [toPath(file), bytes, flag, toMode(mode, 0o666)], finish: () => undefined };
  },

  appendFile(file, data, options) {
    const opts = getOptions(options, { flag: 'a' });
    return plans.writeFile(file, data, opts);
  },

  stat(file, options) {
    const opts = options !== null && typeof options === 'object' ? options : {};
    return { name: 'stat', args: [toPath(file)], finish: (parts) => new Stats(parts), quiet: opts.throwIfNoEntry === false };
  },

  lstat(file, options) {
    return { ...plans.stat(file, options), name: 'lstat' };
  },

  fstat(fd) {
    return { name: 'fstat', args: [toFd(fd)], finish: (parts) => new Stats(parts) };
  },

  readdir(dir, options) {
    const { withFileTypes, recursive, encoding } = getOptions(options);
    const root = toPath(dir);
    const shape = (entries, parent) => entries.map(([name, type]) => {
      if (withFileTypes) return new Dirent(name, type, parent);
      return encoding === 'buffer' ? Buffer.from(name) : name;
    });
    if (!recursive) return { name: 'readdir', args: [root], finish: (parts) => shape(pairs(parts), root) };
    // Recursive listings are one readdir per directory, each authorized on its own.
    const visit = (found, parent, prefix, entries) => {
      const subdirs = [];
      for (const [name, type] of entries) {
        found.push(withFileTypes ? new Dirent(name, type, parent) : prefix + name);
        if (type === 'd') subdirs.push([path.join(parent, name), `${prefix}${name}/`]);
      }
      return subdirs;
    };
    return {
      custom: {
        sync() {
          const found = [];
          const walk = (parent, prefix) => {
            for (const [sub, subPrefix] of visit(found, parent, prefix, pairs(run('readdir', parent)))) walk(sub, subPrefix);
          };
          walk(root, '');
          return found;
        },
        async async() {
          const found = [];
          const walk = async (parent, prefix) => {
            for (const [sub, subPrefix] of visit(found, parent, prefix, pairs(await runAsync('readdir', parent)))) await walk(sub, subPrefix);
          };
          await walk(root, '');
          return found;
        },
      },
    };
  },

  mkdir(dir, options) {
    const opts = typeof options === 'number' || typeof options === 'string' ? { mode: options } : getOptions(options);
    const finish = ([first]) => (first && first.length ? first.toString() : undefined);
    return { name: 'mkdir', args: [toPath(dir), opts.recursive ? '1' : '0', toMode(opts.mode, 0o777)], finish };
  },

  rm(target, options) {
    const { recursive, force } = getOptions(options);
    return { name: 'rm', args: [toPath(target), recursive ? '1' : '0', force ? '1' : '0'], finish: () => undefined };
  },

  rmdir(target) {
    return { name: 'rmdir', args: [toPath(target)], finish: () => undefined };
  },

  unlink(target) {
    return { name: 'unlink', args: [toPath(target)], finish: () => undefined };
  },

  rename(from, to) {
    return { name: 'rename', args: [toPath(from, 'oldPath'), toPath(to, 'newPath')], finish: () => undefined };
  },

  copyFile(from, to, mode = 0) {
    if (typeof mode !== 'number') throw errors.invalidArgType('mode', 'number', mode);
    return { name: 'copyFile', args: [toPath(from, 'src'), toPath(to, 'dest'), mode], finish: () => undefined };
  },

  cp(from, to, options) {
    const { recursive, force, errorOnExist } = getOptions(options, { force: true });
    const flags = [recursive, force, errorOnExist].map((flag) => (flag ? '1' : '0'));
    return { name: 'cp', args: [toPath(from, 'src'), toPath(to, 'dest'), ...flags], finish: () => undefined };
  },

  access(target, mode = 0) {
    if (typeof mode !== 'number') throw errors.invalidArgType('mode', 'integer', mode);
    return { name: 'access', args: [toPath(target), mode], finish: () => undefined };
  },

  realpath(target, options) {
    const { encoding } = getOptions(options);
    return { name: 'realpath', args: [toPath(target)], finish: ([real]) => (encoding === 'buffer' ? real : real.toString()) };
  },

  symlink(target, link) {
    return { name: 'symlink', args: [toPath(target, 'target'), toPath(link)], finish: () => undefined };
  },

  readlink(target) {
    return { name: 'readlink', args: [toPath(target)], finish: text };
  },

  chmod(target, mode) {
    return { name: 'chmod', args: [toPath(target), toMode(mode)], finish: () => undefined };
  },

  utimes(target, atime, mtime) {
    return { name: 'utimes', args: [toPath(target), toMillis(atime, 'atime'), toMillis(mtime, 'mtime')], finish: () => undefined };
  },

  mkdtemp(prefix) {
    return { name: 'mkdtemp', args: [toPath(prefix, 'prefix')], finish: text };
  },

  opendir(dir) {
    const root = toPath(dir);
    return { name: 'readdir', args: [root], finish: (parts) => new Dir(root, pairs(parts).map(([name, type]) => new Dirent(name, type, root))) };
  },

  open(file, flags = 'r', mode) {
    if (typeof flags !== 'string') throw errors.invalidArgValue('flags', flags, 'must be a string (numeric flags are not supported)');
    return { name: 'open', args: [toPath(file), flags, toMode(mode, 0o666)], finish: (parts) => Number(text(parts)) };
  },

  close(fd) {
    return { name: 'close', args: [toFd(fd)], finish: () => undefined };
  },

  read(fd, buffer, offset, length, position) {
    if (offset !== null && typeof offset === 'object') ({ offset, length, position } = offset);
    if (!ArrayBuffer.isView(buffer)) throw errors.invalidArgType('buffer', ['Buffer', 'TypedArray', 'DataView'], buffer);
    offset = offset ?? 0;
    length = length ?? buffer.byteLength - offset;
    if (!Number.isInteger(offset) || offset < 0 || offset > buffer.byteLength) throw errors.outOfRange('offset', `>= 0 && <= ${buffer.byteLength}`, offset);
    if (!Number.isInteger(length) || length < 0 || offset + length > buffer.byteLength) throw errors.outOfRange('length', `>= 0 && <= ${buffer.byteLength - offset}`, length);
    const at = typeof position === 'number' && position >= 0 ? position : -1;
    const finish = ([data]) => {
      new Uint8Array(buffer.buffer, buffer.byteOffset + offset, data.length).set(data);
      return data.length;
    };
    return { name: 'read', args: [toFd(fd), length, at], finish, extra: buffer };
  },

  write(fd, data, a, b, c) {
    let bytes; let position;
    if (typeof data === 'string') {
      bytes = toData(data, typeof b === 'string' ? b : undefined);
      position = a;
    } else {
      if (!ArrayBuffer.isView(data)) throw errors.invalidArgType('buffer', ['string', 'Buffer', 'TypedArray', 'DataView'], data);
      let offset = a; let length = b; position = c;
      if (a !== null && typeof a === 'object') ({ offset, length, position } = a);
      offset = offset ?? 0;
      length = length ?? data.byteLength - offset;
      if (!Number.isInteger(offset) || offset < 0 || offset > data.byteLength) throw errors.outOfRange('offset', `>= 0 && <= ${data.byteLength}`, offset);
      if (!Number.isInteger(length) || length < 0 || offset + length > data.byteLength) throw errors.outOfRange('length', `>= 0 && <= ${data.byteLength - offset}`, length);
      bytes = new Uint8Array(data.buffer, data.byteOffset + offset, length);
    }
    const at = typeof position === 'number' && position >= 0 ? position : -1;
    return { name: 'write', args: [toFd(fd), bytes, at], finish: (parts) => Number(text(parts)), extra: data };
  },
};

function execSync(plan) {
  if (plan.custom) return plan.custom.sync();
  try {
    return plan.finish(run(plan.name, ...plan.args));
  } catch (error) {
    if (plan.quiet && error.code === 'ENOENT') return undefined;
    throw error;
  }
}

function execAsync(plan) {
  if (plan.custom) return plan.custom.async();
  return runAsync(plan.name, ...plan.args).then(plan.finish);
}

const sync = {};
const callback = {};
const promises = {};

for (const [name, plan] of Object.entries(plans)) {
  sync[`${name}Sync`] = (...args) => execSync(plan(...args));
  callback[name] = (...args) => {
    const done = args.pop();
    errors.validateFunction(done, 'cb');
    const planned = plan(...args);
    util.settle(execAsync(planned), done, planned.extra);
  };
  promises[name] = (...args) => {
    try {
      return execAsync(plan(...args));
    } catch (error) {
      return Promise.reject(error);
    }
  };
}

// fs.stat(path[, options], cb): the options argument is optional, which `args.pop()` handles.
// fs.exists never reports an error, including a denial.
sync.existsSync = (target) => {
  try {
    execSync(plans.access(target));
    return true;
  } catch {
    return false;
  }
};
callback.exists = (target, done) => {
  errors.validateFunction(done, 'cb');
  let pending;
  try { pending = execAsync(plans.access(target)).then(() => true, () => false); } catch { pending = Promise.resolve(false); }
  pending.then((found) => require('process').nextTick(done, found));
};

// realpath.native is the same operation: there is no libuv fallback to tell apart.
callback.realpath.native = callback.realpath;
sync.realpathSync.native = sync.realpathSync;

module.exports = { plans, execSync, execAsync, sync, callback, promises };
