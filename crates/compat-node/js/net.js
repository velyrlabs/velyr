'use strict';
// node:net over Velyr.connect / Velyr.listen (io-net). Written for this runtime.
//
// A Socket is a Duplex on one Conn. Reads take chunks straight from io-net's byte stream (one
// read in flight, only while the Readable wants data) and wrap them as Buffer views, so nothing is
// copied or prefetched. Writes go one at a time and complete when io-net has written them, so
// `write()` backpressure (and `drain`) follows the kernel; the Writable buffer is capped by the
// high-water mark. Permissions are io-net's: listen needs net-listen, connect needs net-connect.
//
// Known limits: TCP only. Unix sockets, named pipes, IPC and `fd`/`handle` options throw
// ERR_VELYR_UNSUPPORTED. `host` is resolved by io-net, or by `lookup` / `family` (checked against
// net-connect for the host and for the answer); no `autoSelectFamily` or `localAddress`. `listen()` defaults to 0.0.0.0, not '::', and takes a host name or IP
// (the first answer is used). `ref()`/`unref()` are no-ops, `backlog` and `exclusive` are ignored,
// and `socket.setKeepAlive()` ignores the initial delay.
const errors = require('internal/errors');
const process = require('process');
const EventEmitter = require('events');
const { Buffer } = require('buffer');
const { Duplex } = require('stream');
const { BlockList, isIP, isIPv4, isIPv6 } = require('internal/net/ip');
const dns = require('dns/promises');

const HIGH_WATER_MARK = 64 * 1024;
const ACCEPT_RETRY_MS = 10;
const SYSTEM_ERRORS = [
  [/refused/i, 'ECONNREFUSED'], [/already in use/i, 'EADDRINUSE'], [/reset by peer/i, 'ECONNRESET'],
  [/broken pipe/i, 'EPIPE'], [/timed out/i, 'ETIMEDOUT'], [/assign requested address/i, 'EADDRNOTAVAIL'],
  [/network is unreachable/i, 'ENETUNREACH'], [/permission denied/i, 'EACCES'],
];

function velyr() {
  const api = globalThis.Velyr;
  if (typeof api?.connect !== 'function') throw new Error('velyr node:net: install the net host service (velyr-io-net) first');
  return api;
}

function unsupported(what) {
  return errors.make(Error, 'ERR_VELYR_UNSUPPORTED', `${what} is not supported by this runtime: only TCP over IP is`);
}

/** Node-style error for a kernel error: permission denials and the usual socket failures. */
function translate(error, syscall, address, port) {
  if (error?.code === 'VELYR_PERMISSION_DENIED') {
    const denied = errors.make(Error, 'ERR_ACCESS_DENIED', error.message);
    Object.defineProperty(denied, 'cause', { value: error, writable: true, configurable: true });
    return denied;
  }
  const known = typeof error?.code === 'string' && error.code.startsWith('VELYR_')
    ? SYSTEM_ERRORS.find(([pattern]) => pattern.test(String(error.message))) : undefined;
  if (!known) return error;
  const where = port === undefined ? '' : ` ${address}:${port}`;
  const mapped = errors.make(Error, known[1], `${syscall} ${known[1]}${where}`);
  Object.assign(mapped, { syscall, address, port });
  Object.defineProperty(mapped, 'cause', { value: error, writable: true, configurable: true });
  return mapped;
}

function validatePort(port) {
  const number = typeof port === 'string' && port.trim() !== '' ? Number(port) : port;
  if (!Number.isInteger(number) || number < 0 || number > 65535) {
    throw errors.make(RangeError, 'ERR_SOCKET_BAD_PORT', `Port should be >= 0 and < 65536. Received ${String(port)}.`);
  }
  return number;
}

const isPortLike = (value) => typeof value === 'number' || (typeof value === 'string' && /^\d+$/.test(value));

/** `connect(options | port[, host] | path, [callback])` -> `{ port, host, callback, options }`. */
function normalizeConnect(args) {
  const callback = typeof args[args.length - 1] === 'function' ? args.pop() : undefined;
  const [first, second] = args;
  let options;
  if (first !== null && typeof first === 'object') options = { ...first };
  else if (typeof first === 'string' && !isPortLike(first)) throw unsupported('Unix sockets and pipes');
  else options = { port: first, host: typeof second === 'string' ? second : undefined };
  if (options.path !== undefined) throw unsupported('Unix sockets and pipes');
  if (options.fd !== undefined || options.handle !== undefined) throw unsupported('Socket handles and file descriptors');
  return { port: validatePort(options.port), host: options.host ?? options.hostname ?? 'localhost', callback, options };
}

/** `listen(options | port[, host[, backlog]] | path, [callback])` -> `{ port, host, callback }`. */
function normalizeListen(args) {
  const callback = typeof args[args.length - 1] === 'function' ? args.pop() : undefined;
  const [first, second] = args;
  let options;
  if (first !== null && typeof first === 'object') options = first;
  else if (typeof first === 'string' && !isPortLike(first)) throw unsupported('Unix sockets and pipes');
  else options = { port: first, host: typeof second === 'string' ? second : undefined };
  if (options.path !== undefined) throw unsupported('Unix sockets and pipes');
  if (options.fd !== undefined || options.handle !== undefined) throw unsupported('Socket handles and file descriptors');
  return { port: validatePort(options.port ?? 0), host: options.host ?? '0.0.0.0', callback };
}

class Socket extends Duplex {
  constructor(options = {}) {
    if (options === null || typeof options !== 'object') throw errors.invalidArgType('options', 'Object', options);
    if (options.fd !== undefined || options.handle !== undefined) throw unsupported('Socket handles and file descriptors');
    super({
      allowHalfOpen: options.allowHalfOpen === true,
      readableHighWaterMark: HIGH_WATER_MARK,
      writableHighWaterMark: HIGH_WATER_MARK,
    });
    this.connecting = false;
    this._conn = null;
    this._reader = null;
    this._writer = null;
    this._reading = false;
    this._wantRead = false;
    this._bytesRead = 0;
    this._bytesWritten = 0;
    this._noDelay = false;
    this._keepAlive = false;
    this._timeoutMs = 0;
    this._lookup = undefined;
    this._family = undefined;
    this._timer = null;
    this._active = Date.now();
    // Writes issued before the connection exists wait on this.
    this._ready = new Promise((resolve, reject) => { this._open = resolve; this._abort = reject; });
    this._ready.catch(() => {});
  }

  connect(...args) {
    const { port, host, callback, options } = normalizeConnect(args);
    if (this._conn || this.connecting) throw errors.make(Error, 'ERR_SOCKET_CONNECTING', 'Socket is already connecting or connected');
    if (callback) this.once('connect', callback);
    if (options.lookup !== undefined) errors.validateFunction(options.lookup, 'options.lookup');
    this._lookup = options.lookup ?? this._lookup;
    this._family = options.family ?? this._family;
    this.connecting = true;
    this._dial(host, port).then(
      (conn) => this._connected(conn),
      (error) => this.destroy(translate(error, 'connect', host, port)),
    );
    return this;
  }

  async _dial(host, port) {
    return velyr().connect({ hostname: await this._address(host, port), port });
  }

  /**
   * What to dial for `host`: itself, or the answer of a custom `lookup` / of `dns.lookup` for a
   * `family`. io-net authorizes the address it is given, so the host the caller named is checked
   * here (before any lookup runs): a custom lookup cannot reach what `--allow-net` does not allow.
   */
  async _address(host, port) {
    if ((this._lookup === undefined && !this._family) || isIP(host)) return host;
    const query = globalThis.Velyr?.permissions?.query;
    const granted = typeof query === 'function' && (await query({ name: 'net', host, port })).state === 'granted';
    if (!granted) throw errors.make(Error, 'VELYR_PERMISSION_DENIED', `net-connect to ${host}:${port} denied`);
    if (this._lookup === undefined) return (await dns.lookup(host, { family: this._family })).address;
    return new Promise((resolve, reject) => {
      this._lookup(host, { family: this._family ?? 0, all: false }, (error, address) => (error ? reject(error) : resolve(address)));
    });
  }

  _connected(conn) {
    if (this.destroyed) {
      conn.close();
      return;
    }
    this._attach(conn);
    this.emit('connect');
    this.emit('ready');
  }

  // Adopts `conn` (connected, or accepted by a server) and starts the proactive read.
  _attach(conn) {
    this._conn = conn;
    this._reader = conn.readable.getReader();
    this._writer = conn.writable.getWriter();
    this.connecting = false;
    if (this._noDelay) conn.setNoDelay(true);
    if (this._keepAlive) conn.setKeepAlive(true);
    this._open();
    if (this._wantRead) {
      this._wantRead = false;
      this._pump();
    } else {
      this.read(0);
    }
  }

  _read() {
    this._pump();
  }

  _pump() {
    if (!this._reader) {
      this._wantRead = true;
      return;
    }
    if (this._reading || this.destroyed) return;
    this._reading = true;
    this._reader.read().then(({ done, value }) => {
      this._reading = false;
      if (this.destroyed) return;
      if (done) {
        this.push(null);
        return;
      }
      this._bytesRead += value.length;
      this._touch();
      if (this.push(Buffer.from(value.buffer, value.byteOffset, value.byteLength))) this._pump();
    }, (error) => {
      this._reading = false;
      if (!this.destroyed) this.destroy(translate(error, 'read'));
    }).catch((error) => this.destroy(error));
  }

  _write(chunk, encoding, callback) {
    const send = () => this._writer.write(chunk).then(() => {
      this._bytesWritten += chunk.length;
      this._touch();
      callback();
    }, (error) => callback(translate(error, 'write')));
    if (this._conn) send();
    else this._ready.then(send, callback);
  }

  _final(callback) {
    const finish = () => this._writer.close().then(() => callback(), () => callback());
    if (this._conn) finish();
    else this._ready.then(finish, () => callback());
  }

  _destroy(error, callback) {
    this.connecting = false;
    clearTimeout(this._timer);
    this._abort(error ?? errors.make(Error, 'ERR_STREAM_DESTROYED', 'Socket was destroyed'));
    try {
      this._conn?.close();
    } catch {
      // Already closed.
    }
    callback(error);
  }

  _touch() {
    this._active = Date.now();
  }

  _arm(delay) {
    this._timer = setTimeout(() => {
      const idle = Date.now() - this._active;
      if (idle >= this._timeoutMs) this.emit('timeout');
      else this._arm(this._timeoutMs - idle);
    }, delay);
    this._timer.unref?.();
  }

  setTimeout(ms, callback) {
    if (!Number.isFinite(ms) || ms < 0) throw errors.outOfRange('timeout', 'a non-negative finite number', ms);
    if (callback) this.once('timeout', callback);
    clearTimeout(this._timer);
    this._timeoutMs = ms;
    if (ms > 0) this._arm(ms);
    return this;
  }

  setNoDelay(noDelay = true) {
    this._noDelay = Boolean(noDelay);
    this._conn?.setNoDelay(this._noDelay);
    return this;
  }

  setKeepAlive(enable = false) {
    this._keepAlive = Boolean(enable);
    this._conn?.setKeepAlive(this._keepAlive);
    return this;
  }

  ref() {
    return this;
  }

  unref() {
    return this;
  }

  resetAndDestroy() {
    return this.destroy();
  }

  address() {
    const local = this._conn?.localAddr;
    return local ? { address: local.hostname, family: isIPv6(local.hostname) ? 'IPv6' : 'IPv4', port: local.port } : {};
  }

  get pending() {
    return this._conn === null;
  }

  get bytesRead() {
    return this._bytesRead;
  }

  get bytesWritten() {
    return this._bytesWritten + this.writableLength;
  }

  get bufferSize() {
    return this.writableLength;
  }

  get readyState() {
    if (this.connecting) return 'opening';
    if (this.destroyed || this._conn === null) return 'closed';
    if (this.readable && this.writable) return 'open';
    return this.readable ? 'readOnly' : 'writeOnly';
  }

  get localAddress() {
    return this._conn?.localAddr.hostname;
  }

  get localPort() {
    return this._conn?.localAddr.port;
  }

  get localFamily() {
    return this._conn && (isIPv6(this._conn.localAddr.hostname) ? 'IPv6' : 'IPv4');
  }

  get remoteAddress() {
    return this._conn?.remoteAddr.hostname;
  }

  get remotePort() {
    return this._conn?.remoteAddr.port;
  }

  get remoteFamily() {
    return this._conn && (isIPv6(this._conn.remoteAddr.hostname) ? 'IPv6' : 'IPv4');
  }
}

class Server extends EventEmitter {
  constructor(options, listener) {
    super();
    if (typeof options === 'function') [options, listener] = [{}, options];
    options = options ?? {};
    if (typeof options !== 'object') throw errors.invalidArgType('options', 'Object', options);
    this._allowHalfOpen = options.allowHalfOpen === true;
    this._listener = null;
    this._opening = false;
    this._closing = false;
    this._closeAfterOpen = false;
    this._closed = false;
    this._count = 0;
    this._connectionEvent = 'connection';
    this.maxConnections = undefined;
    if (listener) this.on(this._connectionEvent, listener);
  }

  // Overridden by tls.Server.
  async _bind(hostname, port) {
    return velyr().listen({ hostname, port });
  }

  _wrap(conn) {
    const socket = new Socket({ allowHalfOpen: this._allowHalfOpen });
    socket._attach(conn);
    return socket;
  }

  listen(...args) {
    const { port, host, callback } = normalizeListen(args);
    if (this._listener || this._opening) throw errors.make(Error, 'ERR_SERVER_ALREADY_LISTEN', 'Listen method has been called more than once without closing.');
    if (callback) this.once('listening', callback);
    this._opening = true;
    this._closing = false;
    this._closed = false;
    (async () => {
      const hostname = isIP(host) ? host : (await dns.lookup(host)).address;
      return this._bind(hostname, port);
    })().then((listener) => {
      this._opening = false;
      this._listener = listener;
      process.nextTick(() => this.emit('listening'));
      this._accept(listener);
      if (this._closeAfterOpen) this.close();
    }, (error) => {
      this._opening = false;
      process.nextTick(() => this.emit('error', translate(error, 'listen', host, port)));
    });
    return this;
  }

  // An accept error (a connection dropped over the sockets quota, EMFILE) is reported and the loop
  // goes on; the short pause keeps a persistent failure from spinning. Closing ends the loop.
  async _accept(listener) {
    for (;;) {
      try {
        const { done, value } = await listener.next();
        if (done) return;
        this._adopt(value);
      } catch (error) {
        if (this._listener !== listener) return;
        process.nextTick(() => this.emit('error', translate(error, 'accept')));
        await new Promise((resolve) => setTimeout(resolve, ACCEPT_RETRY_MS));
      }
    }
  }

  _adopt(conn) {
    if (this.maxConnections !== undefined && this._count >= this.maxConnections) {
      conn.close();
      this.emit('drop', { localAddress: conn.localAddr.hostname, localPort: conn.localAddr.port, remoteAddress: conn.remoteAddr.hostname, remotePort: conn.remoteAddr.port });
      return;
    }
    const socket = this._wrap(conn);
    this._count += 1;
    socket.once('close', () => {
      this._count -= 1;
      this._settle();
    });
    this.emit(this._connectionEvent, socket);
  }

  _settle() {
    if (!this._closing || this._count > 0 || this._closed) return;
    this._closed = true;
    process.nextTick(() => this.emit('close'));
  }

  close(callback) {
    if (typeof callback === 'function') {
      if (this._listener || this._opening) this.once('close', callback);
      else this.once('close', () => callback(errors.make(Error, 'ERR_SERVER_NOT_RUNNING', 'Server is not running.')));
    }
    if (this._opening) {
      this._closeAfterOpen = true;
      return this;
    }
    const listener = this._listener;
    if (!listener) {
      if (typeof callback === 'function') process.nextTick(() => this.emit('close'));
      return this;
    }
    this._listener = null;
    this._closing = true;
    this._closeAfterOpen = false;
    listener.close();
    this._settle();
    return this;
  }

  address() {
    const addr = this._listener?.addr;
    return addr ? { address: addr.hostname, family: isIPv6(addr.hostname) ? 'IPv6' : 'IPv4', port: addr.port } : null;
  }

  getConnections(callback) {
    errors.validateFunction(callback, 'callback');
    process.nextTick(callback, null, this._count);
    return this;
  }

  get listening() {
    return this._listener !== null;
  }

  ref() {
    return this;
  }

  unref() {
    return this;
  }
}

function connect(...args) {
  const { options } = normalizeConnect(args.slice());
  return new Socket(options).connect(...args);
}

module.exports = {
  BlockList,
  Server,
  Socket,
  Stream: Socket,
  connect,
  createConnection: connect,
  createServer: (options, listener) => new Server(options, listener),
  isIP,
  isIPv4,
  isIPv6,
};
// Shared with node:tls; not enumerable, so it is not part of the public export list.
Object.defineProperty(module.exports, '_internal', { value: { normalizeConnect, translate, unsupported, velyr } });
