'use strict';
// node:module. Written for this runtime; no Node source is copied.
//
// Known limits: createRequire needs the CommonJS loader (velyr-compat-commonjs installed first);
// builtinModules lists the builtins this runtime has, not Node's full list; register() and the
// other loader-hook APIs are unsupported and throw ERR_VELYR_UNSUPPORTED.
const errors = require('internal/errors');
const node = globalThis[Symbol.for('velyr.node')];

const isUrlLike = (value) => (typeof value === 'object' && value !== null && typeof value.href === 'string')
  || (typeof value === 'string' && value.startsWith('file:'));

// fileURLToPath returns native paths, so a Windows path must be checked with path.win32.
const platformPath = () => (require('process').platform === 'win32' ? require('path').win32 : require('path'));

function createRequire(filename) {
  let path = filename;
  if (isUrlLike(filename)) path = require('url').fileURLToPath(filename);
  if (typeof path !== 'string' || !platformPath().isAbsolute(path)) {
    throw errors.invalidArgValue('filename', filename, 'must be a file URL object, file URL string, or absolute path string');
  }
  const loader = globalThis[Symbol.for('velyr.commonjs')];
  if (loader === undefined || typeof loader.createRequire !== 'function') {
    throw errors.make(Error, 'ERR_VELYR_UNSUPPORTED', 'module.createRequire() needs the CommonJS loader, which is not installed');
  }
  return loader.createRequire(path);
}

function isBuiltin(name) {
  if (typeof name !== 'string') return false;
  return node.has(name.startsWith('node:') ? name.slice(5) : name);
}

const unsupported = (name) => () => {
  throw errors.make(Error, 'ERR_VELYR_UNSUPPORTED', `module.${name}() is not supported by this runtime`);
};

class Module {
  static createRequire = createRequire;
  static isBuiltin = isBuiltin;
  static register = unsupported('register');
  static registerHooks = unsupported('registerHooks');
  static syncBuiltinESMExports = () => {}; // field, so it is enumerable like the other exports
}
Module.Module = Module;
Module.builtinModules = node.names();

module.exports = Module;
