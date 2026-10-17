'use strict';
// node:vm. Written for this runtime; no Node source is copied.
//
// Known limits: the engine API exposes no separate contexts, so only the this-context half of
// node:vm works (runInThisContext, Script#runInThisContext, compileFunction). createContext,
// runInContext, runInNewContext and their Script counterparts throw
// ERR_FEATURE_UNAVAILABLE_ON_PLATFORM; isContext is always false. Code is compiled with indirect
// eval, which this runtime does not gate behind a capability. Script compiles lazily, so a syntax
// error surfaces on the first run, not at construction.
const errors = require('internal/errors');

const indirectEval = eval;

function unavailable(name) {
  return errors.make(TypeError, 'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM',
    `vm.${name} is not available: this runtime has no support for separate contexts`);
}

function filenameOf(options) {
  if (typeof options === 'string') return options;
  return options && typeof options.filename === 'string' ? options.filename : undefined;
}

function runInThisContext(code, options) {
  errors.validateString(code, 'code');
  const filename = filenameOf(options);
  return indirectEval(filename ? `${code}\n//# sourceURL=${filename}` : code);
}

class Script {
  constructor(code, options) {
    errors.validateString(code, 'code');
    this._code = code;
    this._options = options;
  }

  runInThisContext() { return runInThisContext(this._code, this._options); }
  runInContext() { throw unavailable('Script#runInContext'); }
  runInNewContext() { throw unavailable('Script#runInNewContext'); }
}

function compileFunction(code, params = [], options) {
  errors.validateString(code, 'code');
  if (options && options.parsingContext !== undefined) throw unavailable('compileFunction with parsingContext');
  return new Function(...params, code);
}

function isContext(object) {
  if (object === null || typeof object !== 'object') throw errors.invalidArgType('object', 'Object', object);
  return false;
}

module.exports = {
  Script,
  compileFunction,
  createContext() { throw unavailable('createContext'); },
  isContext,
  runInContext() { throw unavailable('runInContext'); },
  runInNewContext() { throw unavailable('runInNewContext'); },
  runInThisContext,
};
