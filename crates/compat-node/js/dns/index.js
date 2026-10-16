'use strict';
// node:dns, callback API over node:dns/promises (see that file for what is and is not supported).
// Written for this runtime.
const errors = require('internal/errors');
const process = require('process');
const promises = require('dns/promises');

const RESOLVE = ['resolve', 'resolve4', 'resolve6', 'resolveAny', 'resolveCaa', 'resolveCname', 'resolveMx',
  'resolveNaptr', 'resolveNs', 'resolvePtr', 'resolveSoa', 'resolveSrv', 'resolveTxt', 'reverse'];

// Callbacks run on nextTick, so an exception in one is uncaught instead of a rejected promise.
function callbackify(call) {
  return function (...args) {
    const callback = args.pop();
    errors.validateFunction(callback, 'callback');
    call.apply(this, args).then(
      (result) => process.nextTick(callback, null, result),
      (error) => process.nextTick(callback, error),
    );
  };
}

function lookup(hostname, options, callback) {
  if (typeof options === 'function') [options, callback] = [{}, options];
  errors.validateFunction(callback, 'callback');
  promises.lookup(hostname, options).then(
    (result) => process.nextTick(callback, null, ...(options?.all ? [result] : [result.address, result.family])),
    (error) => process.nextTick(callback, error),
  );
}

class Resolver extends promises.Resolver {
  constructor() {
    super();
    for (const name of RESOLVE) this[name] = callbackify(this[name]);
  }
}

const api = {
  ADDRCONFIG: 1024,
  ALL: 256,
  Resolver,
  V4MAPPED: 2048,
  getDefaultResultOrder: promises.getDefaultResultOrder,
  getServers: promises.getServers,
  lookup,
  promises,
  setDefaultResultOrder: promises.setDefaultResultOrder,
  setServers: promises.setServers,
};
for (const name of RESOLVE) api[name] = callbackify(promises[name]);

module.exports = api;
