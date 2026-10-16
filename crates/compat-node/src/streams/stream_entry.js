'use strict';
// node:stream: the vendored streams plus the Web Streams adapters.
module.exports = require('internal/streams/web_interop')(require('internal/streams/main'));
