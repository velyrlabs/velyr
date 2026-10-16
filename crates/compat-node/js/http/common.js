'use strict';
// Shared pieces of node:http and node:https: METHODS, STATUS_CODES, header validation and
// OutgoingMessage (the header store both ServerResponse and ClientRequest build on). Written for
// this runtime; the HTTP protocol itself is io-http (hyper), never parsed or serialized in JS.
const errors = require('internal/errors');
const { Writable } = require('stream');
const { Buffer } = require('buffer');

const METHODS = ['ACL', 'BIND', 'CHECKOUT', 'CONNECT', 'COPY', 'DELETE', 'GET', 'HEAD', 'LINK', 'LOCK', 'M-SEARCH', 'MERGE',
  'MKACTIVITY', 'MKCALENDAR', 'MKCOL', 'MOVE', 'NOTIFY', 'OPTIONS', 'PATCH', 'POST', 'PROPFIND', 'PROPPATCH', 'PURGE', 'PUT',
  'QUERY', 'REBIND', 'REPORT', 'SEARCH', 'SOURCE', 'SUBSCRIBE', 'TRACE', 'UNBIND', 'UNLINK', 'UNLOCK', 'UNSUBSCRIBE'];

const STATUS_CODES = {
  100: 'Continue', 101: 'Switching Protocols', 102: 'Processing', 103: 'Early Hints',
  200: 'OK', 201: 'Created', 202: 'Accepted', 203: 'Non-Authoritative Information', 204: 'No Content', 205: 'Reset Content',
  206: 'Partial Content', 207: 'Multi-Status', 208: 'Already Reported', 226: 'IM Used',
  300: 'Multiple Choices', 301: 'Moved Permanently', 302: 'Found', 303: 'See Other', 304: 'Not Modified', 305: 'Use Proxy',
  307: 'Temporary Redirect', 308: 'Permanent Redirect',
  400: 'Bad Request', 401: 'Unauthorized', 402: 'Payment Required', 403: 'Forbidden', 404: 'Not Found', 405: 'Method Not Allowed',
  406: 'Not Acceptable', 407: 'Proxy Authentication Required', 408: 'Request Timeout', 409: 'Conflict', 410: 'Gone',
  411: 'Length Required', 412: 'Precondition Failed', 413: 'Payload Too Large', 414: 'URI Too Long', 415: 'Unsupported Media Type',
  416: 'Range Not Satisfiable', 417: 'Expectation Failed', 418: "I'm a Teapot", 421: 'Misdirected Request',
  422: 'Unprocessable Entity', 423: 'Locked', 424: 'Failed Dependency', 425: 'Too Early', 426: 'Upgrade Required',
  428: 'Precondition Required', 429: 'Too Many Requests', 431: 'Request Header Fields Too Large',
  451: 'Unavailable For Legal Reasons',
  500: 'Internal Server Error', 501: 'Not Implemented', 502: 'Bad Gateway', 503: 'Service Unavailable', 504: 'Gateway Timeout',
  505: 'HTTP Version Not Supported', 506: 'Variant Also Negotiates', 507: 'Insufficient Storage', 508: 'Loop Detected',
  509: 'Bandwidth Limit Exceeded', 510: 'Not Extended', 511: 'Network Authentication Required',
};

const TOKEN = /^[\^_`a-zA-Z\-0-9!#$%&'*+.|~]+$/;
const BAD_VALUE_CHAR = /[^\t\x20-\x7e\x80-\xff]/;
// The body framing belongs to hyper: a Transfer-Encoding set here would be sent twice.
const HYPER_OWNED = new Set(['transfer-encoding']);

function validateHeaderName(name, label = 'Header name') {
  if (typeof name !== 'string' || !name || !TOKEN.test(name)) {
    throw errors.make(TypeError, 'ERR_INVALID_HTTP_TOKEN', `${label} must be a valid HTTP token ["${name}"]`);
  }
}

function validateHeaderValue(name, value) {
  if (value === undefined) throw errors.make(TypeError, 'ERR_HTTP_INVALID_HEADER_VALUE', `Invalid value "${value}" for header "${name}"`);
  const values = Array.isArray(value) ? value : [value];
  for (const item of values) {
    if (BAD_VALUE_CHAR.test(String(item))) throw errors.make(TypeError, 'ERR_INVALID_CHAR', `Invalid character in header content ["${name}"]`);
  }
}

/** The bytes of a body chunk: Buffers and views pass straight through, strings convert once. */
function toBytes(chunk, encoding) {
  if (typeof chunk === 'string') return Buffer.from(chunk, encoding);
  if (chunk instanceof Uint8Array) return chunk;
  throw errors.invalidArgType('chunk', ['string', 'Buffer', 'Uint8Array'], chunk);
}

const kHeaders = Symbol('headers');

class OutgoingMessage extends Writable {
  constructor(options) {
    super({
      highWaterMark: options?.highWaterMark ?? 16 * 1024,
      decodeStrings: true,
      autoDestroy: options?.autoDestroy ?? true,
      emitClose: options?.emitClose ?? true,
    });
    this[kHeaders] = null;
    this.headersSent = false;
    this.finished = false;
    this.sendDate = true;
    this.chunkedEncoding = false;
    this.shouldKeepAlive = true;
    this.useChunkedEncodingByDefault = true;
  }

  get _headers() {
    return this.getHeaders();
  }

  _assertMutable() {
    if (this.headersSent) throw errors.make(Error, 'ERR_HTTP_HEADERS_SENT', 'Cannot set headers after they are sent to the client');
  }

  setHeader(name, value) {
    validateHeaderName(name);
    validateHeaderValue(name, value);
    this._assertMutable();
    (this[kHeaders] ??= new Map()).set(name.toLowerCase(), [name, value]);
    return this;
  }

  appendHeader(name, value) {
    validateHeaderName(name);
    validateHeaderValue(name, value);
    this._assertMutable();
    const headers = (this[kHeaders] ??= new Map());
    const key = name.toLowerCase();
    const current = headers.get(key);
    if (!current) headers.set(key, [name, value]);
    else headers.set(key, [current[0], [].concat(current[1], value)]);
    return this;
  }

  setHeaders(headers) {
    const entries = typeof headers.entries === 'function' ? headers.entries() : Object.entries(headers);
    for (const [name, value] of entries) this.setHeader(name, value);
    return this;
  }

  getHeader(name) {
    if (typeof name !== 'string') throw errors.invalidArgType('name', 'string', name);
    return this[kHeaders]?.get(name.toLowerCase())?.[1];
  }

  getHeaders() {
    const out = Object.create(null);
    if (this[kHeaders]) for (const [key, [, value]] of this[kHeaders]) out[key] = value;
    return out;
  }

  getHeaderNames() {
    return this[kHeaders] ? [...this[kHeaders].keys()] : [];
  }

  getRawHeaderNames() {
    return this[kHeaders] ? [...this[kHeaders].values()].map(([name]) => name) : [];
  }

  hasHeader(name) {
    if (typeof name !== 'string') throw errors.invalidArgType('name', 'string', name);
    return this[kHeaders]?.has(name.toLowerCase()) ?? false;
  }

  removeHeader(name) {
    if (typeof name !== 'string') throw errors.invalidArgType('name', 'string', name);
    this._assertMutable();
    this[kHeaders]?.delete(name.toLowerCase());
  }

  /** `[[name, value], ...]` for the host: one pair per value, hyper-owned headers left out. */
  _headerPairs(extra) {
    const pairs = [];
    const add = (name, value) => {
      if (HYPER_OWNED.has(name.toLowerCase())) return;
      if (Array.isArray(value)) for (const item of value) pairs.push([name, String(item)]);
      else pairs.push([name, String(value)]);
    };
    if (this[kHeaders]) for (const [name, value] of this[kHeaders].values()) add(name, value);
    if (extra) for (const [name, value] of extra) add(name, value);
    return pairs;
  }

  addTrailers() {
    throw errors.make(Error, 'ERR_VELYR_UNSUPPORTED', 'HTTP trailers are not supported by this runtime: io-http does not carry them');
  }

  flushHeaders() {}

  setTimeout(ms, callback) {
    if (callback) this.once('timeout', callback);
    return this;
  }
}

/** `writeHead`-style header arguments: an object, an array of pairs, or a flat `[k, v, k, v]` array. */
function headerEntries(headers) {
  if (headers === undefined || headers === null) return [];
  if (!Array.isArray(headers)) return Object.entries(headers);
  if (headers.length > 0 && Array.isArray(headers[0])) return headers;
  if (headers.length % 2 !== 0) throw errors.make(TypeError, 'ERR_INVALID_ARG_VALUE', `The argument 'headers' is invalid. Received ${headers}`);
  const entries = [];
  for (let i = 0; i < headers.length; i += 2) entries.push([headers[i], headers[i + 1]]);
  return entries;
}

module.exports = { METHODS, STATUS_CODES, OutgoingMessage, headerEntries, kHeaders, toBytes, validateHeaderName, validateHeaderValue };
