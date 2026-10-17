'use strict';
// ServerResponse and the socket handed to 'upgrade' listeners. Written for this runtime.
//
// A response goes to the host in one op when it can: `res.end(body)` without earlier writes sends
// head and body together (the length is then known, so hyper sends Content-Length); otherwise the
// head goes out with the first write and every `write()` is an op that completes when io-http's
// bounded body channel accepts the bytes, so `write()` returns false and 'drain' follows the peer.
// Buffers and views reach the host as they are; strings are converted once by Writable.
//
// Known limits: `statusMessage` is not sent (the canonical reason is), 1xx responses other than 101
// are ignored (hyper answers `Expect: 100-continue` itself), `sendDate` cannot be turned off, there
// are no trailers, and the Transfer-Encoding header is owned by hyper.
const errors = require('internal/errors');
const ops = require('internal/ops');
const { Buffer } = require('buffer');
const { Duplex } = require('stream');
const { OutgoingMessage, headerEntries } = require('internal/http/common');

const NO_BODY = new Set([204, 304]);
const MAX_UPGRADE_HEAD = 64 * 1024;

/** `[[name, value], ...]` as `name\0value\0...` for the host; NUL cannot be a field byte. */
function encodeHeaders(pairs) {
  const flat = [];
  for (const [name, value] of pairs) {
    if (name.includes('\0') || value.includes('\0')) throw errors.make(TypeError, 'ERR_INVALID_CHAR', `Invalid character in header content ["${name}"]`);
    flat.push(name, value);
  }
  return flat.join('\0');
}

class ServerResponse extends OutgoingMessage {
  constructor(req, options) {
    super(options);
    this.req = req;
    this.statusCode = 200;
    this.statusMessage = undefined;
    this._rid = -1;
    this._responded = false;
    this._ending = false;
    this._bodyDone = false;
    this._onClose = null;
  }

  get socket() {
    return this.req?.socket;
  }

  get connection() {
    return this.req?.socket;
  }

  writeHead(statusCode, reason, headers) {
    if (this.headersSent) throw errors.make(Error, 'ERR_HTTP_HEADERS_SENT', 'Cannot write headers after they are sent to the client');
    const code = Number(statusCode);
    if (!Number.isInteger(code) || code < 100 || code > 999) {
      throw errors.make(RangeError, 'ERR_HTTP_INVALID_STATUS_CODE', `Invalid status code: ${statusCode}`);
    }
    if (typeof reason === 'string') this.statusMessage = reason;
    else headers = reason;
    for (const [name, value] of headerEntries(headers)) this.setHeader(name, value);
    if (code < 200 && code !== 101) return this;
    this.statusCode = code;
    this.headersSent = true;
    return this;
  }

  flushHeaders() {
    this._ensureHead();
  }

  writeContinue() {}

  writeProcessing() {}

  writeEarlyHints() {}

  _noBody() {
    return this.req?.method === 'HEAD' || NO_BODY.has(this.statusCode) || this.statusCode < 200;
  }

  /** Sends the head: `body` is bytes (the whole body), true (a body written later) or null (none). */
  _respond(body) {
    this._responded = true;
    this.headersSent = true;
    const code = this.statusCode;
    if (code < 100 || code > 599) throw errors.make(RangeError, 'ERR_HTTP_INVALID_STATUS_CODE', `Invalid status code: ${code}`);
    ops.httpRespond(this._rid, code, encodeHeaders(this._headerPairs()), body);
  }

  _ensureHead() {
    if (!this._responded) this._respond(this._noBody() ? null : true);
  }

  end(chunk, encoding, callback) {
    if (typeof chunk === 'function') [callback, chunk, encoding] = [chunk, undefined, undefined];
    else if (typeof encoding === 'function') [callback, encoding] = [encoding, undefined];
    if (!this.writableEnded) {
      this._ending = true;
      this.finished = true;
    }
    return super.end(chunk, encoding, callback);
  }

  _write(chunk, encoding, callback) {
    if (this._noBody()) {
      this._ensureHead();
      callback();
    } else if (this._ending && !this._responded) {
      this._bodyDone = true;
      this._respond(chunk);
      callback();
    } else {
      this._ensureHead();
      ops.httpWrite(this._rid, chunk, false).then(() => callback(), (error) => this._fail(error, callback));
    }
  }

  _writev(entries, callback) {
    this._write(Buffer.concat(entries.map((entry) => entry.chunk)), 'buffer', callback);
  }

  _final(callback) {
    if (!this._responded) {
      this._bodyDone = true;
      this._respond(null);
      callback();
    } else if (this._bodyDone) {
      callback();
    } else {
      ops.httpEnd(this._rid).then(() => callback(), (error) => this._fail(error, callback));
    }
  }

  // A client that left mid-body is not an error of the handler: the response just closes.
  _fail(error, callback) {
    callback();
    this.destroy();
  }

  _destroy(error, callback) {
    this._onClose?.(error);
    callback(error);
  }
}

/**
 * The socket of an 'upgrade' event. HTTP/1.1 upgrade libraries (ws) write the raw `101` response
 * to it; io-http wants the status and headers as data instead, so the writes are held until the
 * head is complete, parsed and sent through `respond`; after that the socket is the upgraded
 * connection itself. A head with another status (a refusal) is sent as a plain response.
 */
class UpgradeSocket extends Duplex {
  constructor(rid, info, local, secure) {
    super({ allowHalfOpen: false, readableHighWaterMark: 64 * 1024, writableHighWaterMark: 64 * 1024 });
    this._rid = rid;
    this._state = 'head';
    this._buf = null;
    this._wantRead = false;
    this._reading = false;
    this.remoteAddress = info.remote?.hostname;
    this.remotePort = info.remote?.port;
    this.remoteFamily = this.remoteAddress?.includes(':') ? 'IPv6' : 'IPv4';
    this.localAddress = local?.hostname;
    this.localPort = local?.port;
    this.encrypted = secure ? true : undefined;
  }

  _write(chunk, encoding, callback) {
    if (this._state === 'open') {
      ops.httpUpWrite(this._rid, chunk).then(() => callback(), callback);
      return;
    }
    if (this._state !== 'head') {
      callback(errors.make(Error, 'ERR_STREAM_WRITE_AFTER_END', 'The upgrade was refused: the socket is closed'));
      return;
    }
    this._buf = this._buf === null ? Buffer.from(chunk) : Buffer.concat([this._buf, chunk]);
    const end = this._buf.indexOf('\r\n\r\n');
    if (end === -1) {
      callback(this._buf.length > MAX_UPGRADE_HEAD ? errors.make(Error, 'ERR_HTTP_HEADERS_SENT', 'The upgrade response head is too large') : undefined);
      return;
    }
    const head = this._buf.toString('latin1', 0, end);
    const rest = this._buf.subarray(end + 4);
    this._buf = null;
    this._answer(head, rest, callback);
  }

  _answer(head, rest, callback) {
    const [statusLine, ...lines] = head.split('\r\n');
    const status = Number(/^HTTP\/1\.[01] (\d{3})/.exec(statusLine)?.[1]);
    const pairs = lines.map((line) => {
      const at = line.indexOf(':');
      return [line.slice(0, at).trim(), line.slice(at + 1).trim()];
    });
    try {
      if (status === 101) {
        this._state = 'opening';
        ops.httpRespond(this._rid, 101, encodeHeaders(pairs), null);
        ops.httpUpReady(this._rid).then(() => {
          this._state = 'open';
          if (this._wantRead) this._pump();
          if (rest.length > 0) ops.httpUpWrite(this._rid, rest).then(() => callback(), callback);
          else callback();
        }, callback);
        return;
      }
      this._state = 'refused';
      ops.httpRespond(this._rid, status >= 100 && status <= 599 ? status : 400, encodeHeaders(pairs), rest.length > 0 ? rest : null);
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _read() {
    if (this._state === 'open') this._pump();
    else this._wantRead = true;
  }

  _pump() {
    if (this._reading || this.destroyed) return;
    this._reading = true;
    ops.httpUpRead(this._rid).then((chunk) => {
      this._reading = false;
      if (this.destroyed) return;
      if (chunk === null) this.push(null);
      else if (this.push(Buffer.from(chunk))) this._pump();
    }, (error) => {
      this._reading = false;
      if (!this.destroyed) this.destroy(error);
    });
  }

  _final(callback) {
    if (this._state === 'open') ops.httpUpShutdown(this._rid).then(() => callback(), () => callback());
    else callback();
  }

  _destroy(error, callback) {
    this._state = 'closed';
    try {
      ops.httpClose(this._rid);
    } catch {
      // Already closed.
    }
    callback(error);
  }

  address() {
    return { address: this.localAddress, family: this.localAddress?.includes(':') ? 'IPv6' : 'IPv4', port: this.localPort };
  }

  setTimeout(ms, callback) {
    if (callback) this.once('timeout', callback);
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
}

module.exports = { ServerResponse, UpgradeSocket };
