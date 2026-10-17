'use strict';
// node:http Server over the host's request channel (src/http_ops.rs, io-http / hyper). Written
// for this runtime. Hyper parses and frames HTTP; this file only turns each request the host hands
// over into an IncomingMessage / ServerResponse pair and releases the request when both are done.
//
// Known limits: TCP over IP only (no `path`/`fd`/`handle`); `listen()` defaults to 0.0.0.0, not
// '::'; no 'connection', 'clientError', 'checkContinue' or 'checkExpectation' events (hyper answers
// `Expect: 100-continue` itself); the timeout properties (`timeout`, `keepAliveTimeout`,
// `headersTimeout`, `requestTimeout`) are accepted and ignored: io-http applies its own limits;
// `server.close()` stops accepting and waits up to 5 s for requests in flight. 'upgrade' fires
// only when a listener exists; without one the request is served as a normal request.
const errors = require('internal/errors');
const ops = require('internal/ops');
const process = require('process');
const EventEmitter = require('events');
const { Buffer } = require('buffer');
const net = require('net');
const dns = require('dns/promises');
const { ConnInfo, IncomingMessage } = require('internal/http/incoming');
const { ServerResponse, UpgradeSocket } = require('internal/http/response');

const { translate, unsupported } = net._internal;
const EMPTY = Buffer.alloc(0);

// Head layout (src/http_ops/head.rs): [id, method, uri, version, remoteHost|null, remotePort|null, name0, value0, ...].
function decodeHead(head) {
  const headers = [];
  for (let i = 6; i < head.length; i += 2) headers.push([head[i], head[i + 1]]);
  const remote = head[4] === null ? null : { hostname: head[4], port: head[5] };
  return { id: head[0], method: head[1], uri: head[2], version: head[3], headers, remote };
}

function validatePort(port) {
  const number = typeof port === 'string' && port.trim() !== '' ? Number(port) : port;
  if (!Number.isInteger(number) || number < 0 || number > 65535) {
    throw errors.make(RangeError, 'ERR_SOCKET_BAD_PORT', `Port should be >= 0 and < 65536. Received ${String(port)}.`);
  }
  return number;
}

function hasBody(headers) {
  const length = headers['content-length'];
  return (length !== undefined && length !== '0') || headers['transfer-encoding'] !== undefined;
}

/** `listen(port[, host[, backlog]][, cb])` or `listen(options[, cb])` -> `{ port, host, callback }`. */
function normalizeListen(args) {
  const callback = typeof args[args.length - 1] === 'function' ? args.pop() : undefined;
  const [first, second] = args;
  let options;
  if (first !== null && typeof first === 'object') options = first;
  else if (typeof first === 'string' && !/^\d+$/.test(first)) throw unsupported('Unix sockets and pipes');
  else options = { port: first, host: typeof second === 'string' ? second : undefined };
  if (options.path !== undefined || options.fd !== undefined || options.handle !== undefined) {
    throw unsupported('Unix sockets, file descriptors and handles');
  }
  return { port: validatePort(options.port ?? 0), host: options.host ?? '0.0.0.0', callback };
}

class Server extends EventEmitter {
  constructor(options, listener) {
    super();
    if (typeof options === 'function') [listener, options] = [options, {}];
    options ??= {};
    if (options === null || typeof options !== 'object') throw errors.invalidArgType('options', 'Object', options);
    this._IncomingMessage = options.IncomingMessage ?? IncomingMessage;
    this._ServerResponse = options.ServerResponse ?? ServerResponse;
    this._tls = null;
    this._id = null;
    this._opening = false;
    this._addr = null;
    this._active = new Set();
    this.listening = false;
    this.timeout = 0;
    this.keepAliveTimeout = options.keepAliveTimeout ?? 5000;
    this.headersTimeout = options.headersTimeout ?? 60000;
    this.requestTimeout = options.requestTimeout ?? 300000;
    this.maxHeadersCount = null;
    this.maxRequestsPerSocket = 0;
    if (listener) this.on('request', listener);
  }

  listen(...args) {
    const { port, host, callback } = normalizeListen(args);
    if (this._id !== null || this._opening) throw errors.make(Error, 'ERR_SERVER_ALREADY_LISTEN', 'Listen method has been called more than once without closing.');
    if (callback) this.once('listening', callback);
    this._opening = true;
    this._open(host, port).then(() => {
      this._opening = false;
      this.emit('listening');
    }, (error) => {
      this._opening = false;
      process.nextTick(() => this.emit('error', translate(error, 'listen', host, port)));
    });
    return this;
  }

  async _open(host, port) {
    const hostname = net.isIP(host) ? host : (await dns.lookup(host)).address;
    const bound = JSON.parse(ops.httpListen(JSON.stringify({ hostname, port, ...this._tls })));
    this._id = bound.id;
    this._addr = bound;
    this.listening = true;
    this._accept(bound.id);
  }

  // Runs until the server was shut down and every connection finished.
  async _accept(id) {
    try {
      while ((await ops.httpAccept(id)) !== null) {
        for (const head of ops.httpTake(id)) if (head[0] !== null) this._dispatch(decodeHead(head));
      }
    } catch (error) {
      process.nextTick(() => this.emit('error', error));
    } finally {
      try {
        ops.httpClose(id);
      } catch {
        // Already closed.
      }
      this.listening = false;
      this._id = null;
      this.emit('close');
    }
  }

  _dispatch(info) {
    const rid = info.id;
    const req = new this._IncomingMessage(null);
    req._owner = this;
    req._peer = info.remote;
    req._init({ version: info.version, method: info.method, url: info.uri, pairs: info.headers }, null);
    if (hasBody(req.headers)) req._pull = () => ops.httpRead(rid);
    const headers = req.headers;
    if (headers.upgrade !== undefined && /\bupgrade\b/i.test(headers.connection ?? '') && this.listenerCount('upgrade') > 0) {
      this._upgrade(req, info);
      return;
    }
    const res = new this._ServerResponse(req);
    res._rid = rid;
    this._track(req, res);
    try {
      this.emit('request', req, res);
    } catch (error) {
      this._handlerFailed(rid, error);
      return;
    }
    if (!res.headersSent && !res.writableEnded) this._watch(req, res);
  }

  // The request resource is released once the response is closed and the request body is consumed
  // (or dropped: an unread body is drained so the connection can be reused).
  _track(req, res) {
    const rid = res._rid;
    let requestDone = false;
    let responseDone = false;
    const release = () => {
      if (!requestDone || !responseDone) return;
      this._active.delete(res);
      try {
        ops.httpClose(rid);
      } catch {
        // Already closed.
      }
    };
    req._onClose = () => {
      requestDone = true;
      release();
    };
    res._onClose = () => {
      responseDone = true;
      if (req._pull === null) requestDone = true;
      else req._dump();
      release();
    };
    this._active.add(res);
  }

  _watch(req, res) {
    ops.httpDisconnected(res._rid).then((gone) => {
      if (!gone) return;
      req.destroy(req.listenerCount('error') > 0 ? errors.make(Error, 'ECONNRESET', 'aborted') : undefined);
      res.destroy();
    }, () => {});
  }

  _handlerFailed(rid, error) {
    try {
      ops.httpFail(rid);
      ops.httpClose(rid);
    } catch {
      // The request is already gone.
    }
    process.nextTick(() => {
      throw error;
    });
  }

  _upgrade(req, info) {
    const rid = info.id;
    ops.httpUpgrade(rid);
    const socket = new UpgradeSocket(rid, info, this._addr, this._tls !== null);
    try {
      this.emit('upgrade', req, socket, EMPTY);
    } catch (error) {
      socket.destroy();
      process.nextTick(() => {
        throw error;
      });
    }
  }

  _socketFor(req) {
    return new ConnInfo({
      remoteAddress: req._peer?.hostname,
      remotePort: req._peer?.port,
      localAddress: this._addr?.hostname,
      localPort: this._addr?.port,
      encrypted: this._tls !== null,
      onDestroy: () => req.destroy(),
    });
  }

  address() {
    if (!this.listening) return null;
    return { address: this._addr.hostname, family: this._addr.hostname.includes(':') ? 'IPv6' : 'IPv4', port: this._addr.port };
  }

  close(callback) {
    if (callback) {
      if (this._id === null) process.nextTick(callback, errors.make(Error, 'ERR_SERVER_NOT_RUNNING', 'Server is not running.'));
      else this.once('close', callback);
    }
    if (this._id !== null) ops.httpShutdown(this._id);
    return this;
  }

  closeAllConnections() {
    for (const res of [...this._active]) res.destroy();
  }

  closeIdleConnections() {}

  setTimeout(ms, callback) {
    this.timeout = ms;
    if (callback) this.on('timeout', callback);
    return this;
  }

  ref() {
    return this;
  }

  unref() {
    return this;
  }

  getConnections(callback) {
    process.nextTick(callback, null, this._active.size);
    return this;
  }

  [Symbol.asyncDispose]() {
    return new Promise((resolve) => this.close(() => resolve()));
  }
}

module.exports = { Server };
