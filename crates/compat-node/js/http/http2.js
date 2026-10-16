'use strict';
// node:http2 is not supported: the module loads (libraries require it at the top) and every entry
// point that would open a session throws ERR_VELYR_UNSUPPORTED.
const errors = require('internal/errors');

function unsupported() {
  throw errors.make(Error, 'ERR_VELYR_UNSUPPORTED', 'node:http2 is not supported by this runtime; use node:http or node:https (HTTP/1.1)');
}

class Http2ServerRequest {
  constructor() {
    unsupported();
  }
}

class Http2ServerResponse {
  constructor() {
    unsupported();
  }
}

module.exports = {
  Http2ServerRequest,
  Http2ServerResponse,
  connect: unsupported,
  constants: Object.freeze({
    HTTP2_HEADER_AUTHORITY: ':authority',
    HTTP2_HEADER_METHOD: ':method',
    HTTP2_HEADER_PATH: ':path',
    HTTP2_HEADER_SCHEME: ':scheme',
    HTTP2_HEADER_STATUS: ':status',
    HTTP2_HEADER_CONTENT_LENGTH: 'content-length',
    HTTP2_HEADER_CONTENT_TYPE: 'content-type',
    HTTP2_HEADER_COOKIE: 'cookie',
    HTTP2_HEADER_HOST: 'host',
    HTTP2_HEADER_SET_COOKIE: 'set-cookie',
    HTTP2_METHOD_GET: 'GET',
    HTTP2_METHOD_POST: 'POST',
    NGHTTP2_CANCEL: 8,
    NGHTTP2_NO_ERROR: 0,
  }),
  createSecureServer: unsupported,
  createServer: unsupported,
  getDefaultSettings: unsupported,
  getPackedSettings: unsupported,
  getUnpackedSettings: unsupported,
  sensitiveHeaders: Symbol('nodejs.http2.sensitiveHeaders'),
};
