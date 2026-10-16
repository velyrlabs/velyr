'use strict';
// node:http client: Agent, ClientRequest, request(), get(). Written for this runtime.
//
// An Agent owns one native client (io-http's connection pool, one per distinct `ca` list); a
// request starts on the runtime as soon as its head is fixed. `keepAlive` decides whether the
// request says `Connection: close`; `maxSockets` / `maxTotalSockets` are enforced here by queueing
// requests. Request bodies stream through an op per write that completes when the connection takes
// the bytes (`write()` returns false and 'drain' follows); response bodies are pulled one chunk at
// a time while the Readable wants data. Buffers pass straight through, strings convert once.
//
// A request that sets `createConnection`, `lookup` or `family` runs over a net/tls socket instead
// (see client_socket.js): that is the only path with client-side 'upgrade'.
//
// Known limits: HTTP/1.1 only; no 'upgrade' / 'connect' on the native path (io-http does not hand
// the connection over); no redirects (as in Node); `reusedSocket` is always false and `req.socket`
// only carries addresses; no trailers; `timeout` / `req.setTimeout()` count inactivity between
// request start and the end of the response; `localAddress`, `socketPath` and `insecureHTTPParser`
// are not supported; https cannot skip verification
// (`rejectUnauthorized: false` throws ERR_VELYR_UNSUPPORTED) and takes no client certificates.
const errors = require('internal/errors');
const ops = require('internal/ops');
const process = require('process');
const EventEmitter = require('events');
const net = require('net');
const { Buffer } = require('buffer');
const { ConnInfo, IncomingMessage } = require('internal/http/incoming');
const { OutgoingMessage, STATUS_CODES, METHODS, headerEntries, validateHeaderName } = require('internal/http/common');

const { connectFor, dialWith, io: socketIo, takeUpgrade } = require('internal/http/client_socket');

const { translate } = net._internal;
const BAD_PATH = /[^!-ÿ]/;
const NO_BODY_STATUS = new Set([204, 304]);

function unsupported(what) {
  return errors.make(Error, 'ERR_VELYR_UNSUPPORTED', `${what} is not supported by this runtime`);
}

const pemOf = (value) => (typeof value === 'string' ? value : Buffer.from(value).toString());

/** The PEM strings of a `ca` option (string, Buffer or an array of them). */
function caList(ca) {
  return ca === undefined || ca === null ? [] : [].concat(ca).map(pemOf);
}

class Agent extends EventEmitter {
  constructor(options) {
    super();
    this.options = { ...options, path: null };
    this.keepAlive = options?.keepAlive ?? false;
    this.keepAliveMsecs = options?.keepAliveMsecs ?? 1000;
    this.maxSockets = options?.maxSockets ?? Infinity;
    this.maxTotalSockets = options?.maxTotalSockets ?? Infinity;
    this.maxFreeSockets = options?.maxFreeSockets ?? 256;
    this.scheduling = options?.scheduling ?? 'lifo';
    this.defaultPort = 80;
    this.protocol = 'http:';
    this.sockets = Object.create(null);
    this.freeSockets = Object.create(null);
    this.requests = Object.create(null);
    this._clients = new Map();
    this._waiting = [];
    this._total = 0;
    this._destroyed = false;
    this._ca = caList(options?.ca);
  }

  getName(options) {
    return `${options.host || 'localhost'}:${options.port || ''}:${options.localAddress || ''}${options.family === 4 || options.family === 6 ? `:${options.family}` : ''}`;
  }

  /** The native client for `ca` (the agent's own roots when empty). */
  _client(ca) {
    const list = ca.length > 0 ? ca : this._ca;
    const key = list.join('\n');
    let id = this._clients.get(key);
    if (id === undefined) {
      id = ops.httpClientNew(JSON.stringify({ ca: list }));
      this._clients.set(key, id);
    }
    return id;
  }

  _count(name) {
    return this.sockets[name]?.length ?? 0;
  }

  _take(name) {
    (this.sockets[name] ??= []).push(true);
    this._total += 1;
  }

  /** `undefined` when a slot was taken, otherwise a promise that resolves once one is. */
  _acquire(name) {
    if (this._count(name) < this.maxSockets && this._total < this.maxTotalSockets) {
      this._take(name);
      return undefined;
    }
    return new Promise((resolve) => this._waiting.push({ name, resolve }));
  }

  _release(name) {
    const list = this.sockets[name];
    if (!list) return;
    list.pop();
    if (list.length === 0) delete this.sockets[name];
    this._total -= 1;
    const at = this._waiting.findIndex((entry) => this._count(entry.name) < this.maxSockets);
    if (at !== -1 && this._total < this.maxTotalSockets) {
      const [entry] = this._waiting.splice(at, 1);
      this._take(entry.name);
      entry.resolve();
    }
  }

  destroy() {
    this._destroyed = true;
    for (const id of this._clients.values()) ops.httpClientClose(id);
    this._clients.clear();
  }
}

/** `(input, options, callback)` of request()/get() -> `{ options, callback }` with URL parts merged. */
function normalize(input, options, callback) {
  if (typeof input === 'function') [callback, input, options] = [input, undefined, undefined];
  else if (typeof options === 'function') [callback, options] = [options, undefined];
  let merged = {};
  if (typeof input === 'string' || input instanceof globalThis.URL) {
    const url = typeof input === 'string' ? new globalThis.URL(input) : input;
    merged = {
      protocol: url.protocol,
      hostname: url.hostname.replace(/^\[|\]$/g, ''),
      port: url.port === '' ? undefined : Number(url.port),
      path: `${url.pathname}${url.search}`,
    };
    if (url.username || url.password) merged.auth = `${decodeURIComponent(url.username)}:${decodeURIComponent(url.password)}`;
  } else if (input !== undefined && input !== null) {
    options = input;
  }
  if (options !== undefined && options !== null && typeof options !== 'object') throw errors.invalidArgType('options', 'Object', options);
  return { options: { ...merged, ...options }, callback };
}

class ClientRequest extends OutgoingMessage {
  constructor(input, options, callback, defaultAgent = Agent.globalAgent) {
    super({ autoDestroy: false, emitClose: false });
    const { options: opts, callback: onResponse } = normalize(input, options, callback);
    const agent = opts.agent === false ? new defaultAgent.constructor() : (opts.agent ?? defaultAgent);
    const protocol = opts.protocol ?? agent.protocol;
    if (protocol !== agent.protocol) throw errors.make(TypeError, 'ERR_INVALID_PROTOCOL', `Protocol "${protocol}" not supported. Expected "${agent.protocol}"`);
    if (opts.socketPath || opts.localAddress) throw unsupported('socketPath and localAddress');
    if (opts.rejectUnauthorized === false || agent.options.rejectUnauthorized === false) throw unsupported('rejectUnauthorized: false');
    if (opts.cert || opts.key || opts.pfx || typeof opts.checkServerIdentity === 'function') throw unsupported('Client certificates and checkServerIdentity');
    const method = (opts.method ?? 'GET').toUpperCase();
    validateHeaderName(method, 'Method');
    if (opts.path !== undefined && BAD_PATH.test(opts.path)) throw errors.make(TypeError, 'ERR_UNESCAPED_CHARACTERS', 'Request path contains unescaped characters');
    this.agent = agent;
    this._ownsAgent = opts.agent === false;
    this.method = method;
    this.path = opts.path ?? '/';
    this.host = opts.hostname ?? opts.host ?? 'localhost';
    this.protocol = protocol;
    this.port = Number(opts.port ?? opts.defaultPort ?? agent.defaultPort);
    this.reusedSocket = false;
    this.aborted = false;
    this.res = null;
    this.maxHeadersCount = null;
    this._ca = caList(opts.ca);
    this._name = agent.getName({ host: this.host, port: this.port });
    this._id = -1;
    this._ready = null;
    this._began = false;
    this._ending = false;
    this._closed = false;
    this._timeoutMs = opts.timeout ?? 0;
    this._streaming = false;
    this._timer = null;
    this._socket = null;
    this._dial = null;
    this._io = ops;
    // As in Node, `createConnection` only counts when no agent was given; `lookup` and `family` always do.
    if ((opts.createConnection && opts.agent == null) || opts.lookup || opts.family) {
      this._io = socketIo;
      const ca = this._ca.length > 0 ? this._ca : agent._ca;
      this._dial = () => dialWith(opts.createConnection ?? connectFor(protocol), { ...opts, host: this.host, port: this.port, path: undefined, agent: undefined, ca: ca.length > 0 ? ca : undefined });
    }
    const bracketed = this.host.includes(':') ? `[${this.host}]` : this.host;
    this._authority = this.port === agent.defaultPort ? bracketed : `${bracketed}:${this.port}`;
    this._url = `${protocol}//${this._authority}${this.path}`;
    for (const [name, value] of headerEntries(opts.headers)) this.setHeader(name, value);
    if (opts.setHost !== false && !this.hasHeader('host')) this.setHeader('Host', this._authority);
    if (opts.auth && !this.hasHeader('authorization')) this.setHeader('Authorization', `Basic ${Buffer.from(String(opts.auth)).toString('base64')}`);
    if ((!agent.keepAlive || this._dial) && !this.hasHeader('connection')) this.setHeader('Connection', 'close');
    if (onResponse) this.once('response', onResponse);
    if (opts.signal) this._watchSignal(opts.signal);
    process.nextTick(() => {
      if (this.listenerCount('socket') > 0) this.emit('socket', this.socket);
    });
  }

  get socket() {
    return (this._socket ??= this._socketFor(null));
  }

  get connection() {
    return this.socket;
  }

  _socketFor(response) {
    return new ConnInfo({ remoteAddress: response?._peer?.hostname ?? this.host, remotePort: this.port, encrypted: this.protocol === 'https:', onDestroy: (error) => this.destroy(error) });
  }

  _watchSignal(signal) {
    const abort = () => this.destroy(errors.make(Error, 'ABORT_ERR', 'The operation was aborted'));
    if (signal.aborted) process.nextTick(abort);
    else signal.addEventListener('abort', abort, { once: true });
  }

  /** Fixes the head and starts the request; `body` is bytes, true (streamed) or null. */
  _begin(body) {
    this.headersSent = true;
    this._began = true;
    this._streaming = body === true;
    const start = () => {
      try {
        this._id = this._dial
          ? socketIo.httpReqBegin(this._dial, this.method, this.path, this._headerPairs(), body)
          : ops.httpReqBegin(this.agent._client(this._ca), this.method, this._url, JSON.stringify(this._headerPairs()), body);
      } catch (error) {
        this.destroy(translate(error, 'connect', this.host, this.port));
        return;
      }
      this._armTimeout();
      this._io.httpReqResponse(this._id).then((json) => this._onHead(JSON.parse(json)), (error) => this.destroy(translate(error, 'connect', this.host, this.port)));
    };
    const slot = this.agent._acquire(this._name);
    if (slot === undefined) {
      start();
    } else {
      this._ready = slot.then(() => {
        if (this._closed) this.agent._release(this._name);
        else start();
      });
    }
  }

  flushHeaders() {
    if (!this._began) this._begin(true);
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
    if (this._ending && !this._began) {
      this._begin(chunk);
      callback();
      return;
    }
    if (!this._began) this._begin(true);
    const send = () => {
      if (this._closed) callback();
      else this._io.httpReqWrite(this._id, chunk).then(() => callback(), (error) => callback(translate(error, 'write')));
    };
    if (this._id !== -1) send();
    else (this._ready ?? Promise.resolve()).then(send);
  }

  _writev(entries, callback) {
    this._write(Buffer.concat(entries.map((entry) => entry.chunk)), 'buffer', callback);
  }

  _final(callback) {
    if (!this._began) this._begin(null);
    else if (this._streaming && this._id !== -1 && !this._closed) this._io.httpReqEnd(this._id);
    else if (this._streaming && this._ready) this._ready.then(() => { if (!this._closed) this._io.httpReqEnd(this._id); });
    callback();
  }

  _onHead({ status, headers, upgrade }) {
    if (this._closed) return;
    const id = this._id;
    const res = new IncomingMessage(null);
    res._owner = this;
    res._peer = { hostname: this.host, port: this.port };
    const empty = upgrade || this.method === 'HEAD' || NO_BODY_STATUS.has(status) || status < 200;
    res._init({ version: 'HTTP/1.1', statusCode: status, statusMessage: STATUS_CODES[status] ?? '', pairs: headers }, null);
    if (upgrade) {
      this._handOver(res);
      return;
    }
    if (!empty) {
      res._pull = this._timeoutMs > 0
        ? () => this._io.httpReqRead(id).then((chunk) => { this._armTimeout(); return chunk; })
        : () => this._io.httpReqRead(id);
    }
    res.req = this;
    this.res = res;
    res._onClose = () => this._cleanup();
    if (this.listenerCount('response') === 0) res._dump();
    this.emit('response', res);
  }

  /** A 101 on a caller-supplied socket: 'upgrade' gets the socket and the bytes after the head. */
  _handOver(res) {
    const { socket, head } = takeUpgrade(this._id);
    clearTimeout(this._timer);
    if (this.listenerCount('upgrade') > 0) this.emit('upgrade', res, socket, head);
    else socket.destroy();
    this._cleanup();
  }

  _armTimeout() {
    if (this._timeoutMs <= 0) return;
    clearTimeout(this._timer);
    this._timer = setTimeout(() => this.emit('timeout'), this._timeoutMs);
    this._timer.unref?.();
  }

  setTimeout(ms, callback) {
    if (callback) this.once('timeout', callback);
    this._timeoutMs = ms;
    if (this._began && !this._closed) this._armTimeout();
    return this;
  }

  setNoDelay() {}

  setSocketKeepAlive() {}

  abort() {
    if (this.aborted) return;
    this.aborted = true;
    process.nextTick(() => this.emit('abort'));
    this.destroy();
  }

  _cleanup() {
    if (this._closed) return;
    this._closed = true;
    clearTimeout(this._timer);
    if (this._id !== -1) this._io.httpReqClose(this._id);
    if (this._began) this.agent._release(this._name);
    if (this._ownsAgent) this.agent.destroy();
    process.nextTick(() => this.emit('close'));
  }

  _destroy(error, callback) {
    if (!error && this._began && !this.res && !this._closed) {
      error = errors.make(Error, 'ECONNRESET', 'socket hang up');
    }
    this.res?.destroy();
    this._cleanup();
    callback(error);
  }
}

function request(input, options, callback) {
  return new ClientRequest(input, options, callback);
}

function get(input, options, callback) {
  const req = request(input, options, callback);
  req.end();
  return req;
}

Agent.globalAgent = new Agent({ keepAlive: true, timeout: 5000 });

module.exports = { Agent, ClientRequest, METHODS, caList, get, request, unsupported };
