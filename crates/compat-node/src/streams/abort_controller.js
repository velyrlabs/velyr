'use strict';
// Stand-in for the npm `abort-controller` package the vendored streams fall back to when the
// global is missing. Hosts install AbortController (web-core events); without it, streams still
// load and only the features that need an AbortController fail, with this message.
const missing = (name) => class {
  constructor() { throw new Error(`velyr compat-node: ${name} is not installed (install web-core events)`); }
};
exports.AbortController = missing('AbortController');
exports.AbortSignal = missing('AbortSignal');
