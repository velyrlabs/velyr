'use strict';
// IncomingMessage (a Readable over the host's body channel) and the socket-shaped object that
// `req.socket` / `res.socket` expose. The body is pulled from the host one chunk at a time, only
// while the Readable wants data, so a slow consumer stops the peer (io-http's bounded channel).
// Chunks arrive as ArrayBuffers and are wrapped, not copied.
//
// Known limits: header names are lower case (hyper normalizes them, so `rawHeaders` is too);
// there are no trailers (`trailers` stays empty); `req.socket` is not a net.Socket, it only carries
// the addresses and no-op socket options.
const { Readable } = require('stream');
const { Buffer } = require('buffer');
const EventEmitter = require('events');

// Duplicates of these are dropped, the first value wins (RFC 9110 / Node's behavior).
const SINGLE = new Set(['content-type', 'content-length', 'user-agent', 'referer', 'host', 'authorization', 'proxy-authorization',
  'if-modified-since', 'if-unmodified-since', 'from', 'location', 'max-forwards', 'retry-after', 'etag', 'last-modified', 'server', 'age', 'expires']);

/** `{ headers, rawHeaders }` from `[[name, value], ...]`. */
function distribute(pairs) {
  const headers = {};
  const rawHeaders = [];
  for (const [name, value] of pairs) {
    rawHeaders.push(name, value);
    const current = headers[name];
    if (current === undefined) headers[name] = name === 'set-cookie' ? [value] : value;
    else if (name === 'set-cookie') current.push(value);
    else if (!SINGLE.has(name)) headers[name] = `${current}${name === 'cookie' ? '; ' : ', '}${value}`;
  }
  return { headers, rawHeaders };
}

function familyOf(address) {
  return address === undefined ? undefined : address.includes(':') ? 'IPv6' : 'IPv4';
}

class ConnInfo extends EventEmitter {
  constructor({ remoteAddress, remotePort, localAddress, localPort, encrypted, onDestroy }) {
    super();
    this.remoteAddress = remoteAddress;
    this.remotePort = remotePort;
    this.remoteFamily = familyOf(remoteAddress);
    this.localAddress = localAddress;
    this.localPort = localPort;
    this.encrypted = encrypted ? true : undefined;
    this.destroyed = false;
    this.readable = true;
    this.writable = true;
    this._onDestroy = onDestroy;
  }

  address() {
    return { address: this.localAddress, family: familyOf(this.localAddress), port: this.localPort };
  }

  setTimeout() {
    return this;
  }

  setNoDelay() {
    return this;
  }

  setKeepAlive() {
    return this;
  }

  ref() {
    return this;
  }

  unref() {
    return this;
  }

  destroy(error) {
    if (this.destroyed) return this;
    this.destroyed = true;
    this._onDestroy?.(error);
    return this;
  }
}

class IncomingMessage extends Readable {
  constructor(socket) {
    super({ highWaterMark: 64 * 1024 });
    this._socket = socket;
    this._owner = null;
    this._peer = null;
    this.httpVersion = '1.1';
    this.httpVersionMajor = 1;
    this.httpVersionMinor = 1;
    this.complete = false;
    this.aborted = false;
    this.headers = {};
    this.rawHeaders = [];
    this.trailers = {};
    this.rawTrailers = [];
    this.method = undefined;
    this.url = '';
    this.statusCode = null;
    this.statusMessage = null;
    this._pull = null;
    this._reading = false;
    this._onClose = null;
  }

  // Built on first use: most handlers never look at it. `_owner` (a Server or ClientRequest) makes it.
  get socket() {
    return (this._socket ??= this._owner?._socketFor(this));
  }

  set socket(value) {
    this._socket = value;
  }

  get connection() {
    return this.socket;
  }

  set connection(value) {
    this._socket = value;
  }

  get headersDistinct() {
    const out = {};
    for (let i = 0; i < this.rawHeaders.length; i += 2) (out[this.rawHeaders[i]] ??= []).push(this.rawHeaders[i + 1]);
    return out;
  }

  get trailersDistinct() {
    return {};
  }

  /** Fills the message from a host head; `pull` yields ArrayBuffers and then null, or is null when there is no body. */
  _init({ version, method, url, statusCode, statusMessage, pairs }, pull) {
    const [, major, minor] = /^HTTP\/(\d)(?:\.(\d))?/.exec(version ?? '') ?? [];
    if (major) {
      this.httpVersionMajor = Number(major);
      this.httpVersionMinor = Number(minor ?? 0);
      this.httpVersion = `${this.httpVersionMajor}.${this.httpVersionMinor}`;
    }
    this.method = method;
    this.url = url ?? '';
    this.statusCode = statusCode ?? null;
    this.statusMessage = statusMessage ?? null;
    Object.assign(this, distribute(pairs));
    this._pull = pull;
    return this;
  }

  _read() {
    if (this._reading || this.destroyed) return;
    if (this._pull === null) {
      this.complete = true;
      this.push(null);
      return;
    }
    this._reading = true;
    this._pull().then((chunk) => {
      this._reading = false;
      if (this.destroyed) return;
      if (chunk === null) {
        this.complete = true;
        this.push(null);
      } else if (this.push(Buffer.from(chunk))) {
        this._read();
      }
    }, (error) => {
      this._reading = false;
      if (!this.destroyed) this.destroy(error);
    });
  }

  _destroy(error, callback) {
    if (!this.complete && !this.aborted) {
      this.aborted = true;
      this.emit('aborted');
    }
    this._onClose?.(error);
    callback(error);
  }

  setTimeout(ms, callback) {
    if (callback) this.once('timeout', callback);
    return this;
  }

  /** Reads and drops what is left of the body (a response was sent without consuming it). */
  _dump() {
    if (this.complete || this.destroyed) return;
    this.on('error', () => {});
    this.resume();
  }
}

module.exports = { ConnInfo, IncomingMessage, distribute };
