'use strict';
// node:http client over a socket the caller supplies (`createConnection`, `lookup`, `family`).
// Written for this runtime. It speaks HTTP/1.1 itself on a node:net / node:tls socket and offers the
// same five operations as the native `ops.httpReq*` calls, so ClientRequest drives either one.
// The native client cannot do this: it opens its own connection and never hands it over (upgrade).
//
// Known limits: one request per socket (the socket is destroyed when the request ends, so no
// keep-alive reuse and `Agent` pooling does not apply); 1xx responses other than 101 are skipped;
// trailers are read and dropped; a head over 64 KiB is refused; response header names are
// lower-cased, as the native client reports them.
const { Buffer } = require('buffer');
const net = require('net');

const MAX_HEAD = 64 * 1024;
const HEAD_END = Buffer.from('\r\n\r\n');
const CRLF = Buffer.from('\r\n');
const BODY_METHODS = new Set(['POST', 'PUT', 'PATCH']);
const entries = new Map();
let nextId = 1;

const parseError = (code, message) => Object.assign(new Error(message), { code });
const truncated = () => parseError('ECONNRESET', 'socket hang up');

/** Pull-style reader over a socket: bytes already received wait in `buf`. */
class Reader {
  constructor(socket) {
    this.socket = socket;
    this.buf = Buffer.alloc(0);
    this.error = null;
    socket.on('error', (error) => { this.error ??= error; });
  }

  /** Appends the next chunk; false once the socket ended. */
  fill() {
    const { socket } = this;
    return new Promise((resolve, reject) => {
      const finish = (settle, value) => {
        for (const [name, listener] of [['readable', attempt], ['end', onEnd], ['close', onEnd], ['error', onError]]) socket.off(name, listener);
        settle(value);
      };
      const onError = (error) => finish(reject, error);
      const onEnd = () => (this.error ? finish(reject, this.error) : finish(resolve, false));
      const attempt = () => {
        if (this.error) return finish(reject, this.error);
        const chunk = socket.read();
        if (chunk !== null) {
          this.buf = Buffer.concat([this.buf, chunk]);
          return finish(resolve, true);
        }
        if (socket.readableEnded || socket.destroyed) return onEnd();
        return undefined;
      };
      for (const [name, listener] of [['readable', attempt], ['end', onEnd], ['close', onEnd], ['error', onError]]) socket.on(name, listener);
      attempt();
    });
  }

  take(count) {
    const out = this.buf.subarray(0, count);
    this.buf = this.buf.subarray(count);
    return out;
  }

  /** Index of `marker` in the buffered bytes, reading until it shows up. */
  async until(marker, limit = Infinity) {
    for (;;) {
      const at = this.buf.indexOf(marker);
      if (at !== -1) return at;
      if (this.buf.length > limit) throw parseError('HPE_HEADER_OVERFLOW', 'Header overflow');
      if (!(await this.fill())) throw truncated();
    }
  }

  async line() {
    const text = this.take(await this.until(CRLF)).toString('latin1');
    this.take(2);
    return text;
  }

  /** Up to `max` bytes, at least one; null at the end of the stream. */
  async some(max) {
    if (this.buf.length === 0 && !(await this.fill())) return null;
    return this.take(Math.min(max, this.buf.length));
  }
}

async function readHead(reader) {
  for (;;) {
    const text = reader.take(await reader.until(HEAD_END, MAX_HEAD)).toString('latin1');
    reader.take(4);
    const [statusLine, ...lines] = text.split('\r\n');
    const match = /^HTTP\/\d\.\d (\d{3})/.exec(statusLine);
    if (!match) throw parseError('HPE_INVALID_CONSTANT', 'Parse Error: Expected HTTP/');
    const status = Number(match[1]);
    if (status >= 100 && status < 200 && status !== 101) continue;
    const headers = lines.map((entry) => {
      const colon = entry.indexOf(':');
      return [entry.slice(0, colon).toLowerCase(), entry.slice(colon + 1).trim()];
    });
    return { status, headers };
  }
}

async function* sized(reader, length) {
  for (let left = length; left > 0;) {
    const part = await reader.some(left);
    if (part === null) throw truncated();
    left -= part.length;
    yield part;
  }
}

async function* bodyOf(reader, headers) {
  const header = (name) => headers.find(([key]) => key === name)?.[1];
  if (/chunked/i.test(header('transfer-encoding') ?? '')) {
    for (;;) {
      const size = parseInt((await reader.line()).split(';')[0], 16);
      if (Number.isNaN(size)) throw parseError('HPE_INVALID_CHUNK_SIZE', 'Invalid character in chunk size');
      if (size === 0) {
        while ((await reader.line()) !== '');
        return;
      }
      yield* sized(reader, size);
      await reader.line();
    }
  }
  if (header('content-length') !== undefined) {
    yield* sized(reader, Number(header('content-length')));
    return;
  }
  for (let part = await reader.some(Infinity); part !== null; part = await reader.some(Infinity)) yield part;
}

/** The socket `create(options, callback)` returns, or hands to its callback. */
function dialWith(create, options) {
  return new Promise((resolve, reject) => {
    const socket = create(options, (error, given) => (error ? reject(error) : resolve(given)));
    if (socket) resolve(socket);
  });
}

/** Default `createConnection` of a request that only set `lookup` / `family`. */
function connectFor(protocol) {
  return (options) => (protocol === 'https:' ? require('tls') : net).connect(options);
}

function headBytes(method, target, pairs, body) {
  const has = (name) => pairs.some(([key]) => key.toLowerCase() === name);
  const framed = has('content-length') || has('transfer-encoding');
  const all = [...pairs];
  let chunked = false;
  if (body === true) {
    chunked = !framed;
    if (chunked) all.push(['Transfer-Encoding', 'chunked']);
  } else if (!framed && (body !== null || BODY_METHODS.has(method))) {
    all.push(['Content-Length', String(body === null ? 0 : Buffer.byteLength(body))]);
  }
  const lines = all.map(([name, value]) => `${name}: ${value}\r\n`).join('');
  return { chunked, bytes: Buffer.from(`${method} ${target} HTTP/1.1\r\n${lines}\r\n`, 'latin1') };
}

const writeTo = (entry, data) => {
  entry.tail = entry.tail.then(() => new Promise((resolve, reject) => {
    entry.socket.write(data, (error) => (error ? reject(error) : resolve()));
  }));
  return entry.tail;
};

/** Starts a request: `dial()` resolves to the socket; `body` is bytes, true (streamed) or null. */
function httpReqBegin(dial, method, target, pairs, body) {
  const id = nextId++;
  const { chunked, bytes } = headBytes(method, target, pairs, body);
  const entry = { chunked, upgraded: false, socket: null, reader: null, body: null, tail: null, ready: null };
  entry.ready = dial().then((socket) => {
    entry.socket = socket;
    entry.reader = new Reader(socket);
  });
  entry.tail = entry.ready;
  entry.ready.catch(() => {});
  writeTo(entry, body === true || body === null ? bytes : Buffer.concat([bytes, Buffer.isBuffer(body) ? body : Buffer.from(body)])).catch(() => {});
  entries.set(id, entry);
  return id;
}

async function httpReqResponse(id) {
  const entry = entries.get(id);
  await entry.ready;
  const { status, headers } = await readHead(entry.reader);
  const upgrade = status === 101;
  if (!upgrade) entry.body = bodyOf(entry.reader, headers);
  return JSON.stringify({ status, headers, upgrade });
}

async function httpReqRead(id) {
  const { value, done } = await entries.get(id).body.next();
  return done ? null : value;
}

function httpReqWrite(id, chunk) {
  const entry = entries.get(id);
  const data = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
  return writeTo(entry, entry.chunked ? Buffer.concat([Buffer.from(`${data.length.toString(16)}\r\n`), data, CRLF]) : data);
}

function httpReqEnd(id) {
  const entry = entries.get(id);
  if (entry.chunked) writeTo(entry, Buffer.from('0\r\n\r\n')).catch(() => {});
}

/** The socket and the bytes read past the head, after a 101: the caller owns the socket now. */
function takeUpgrade(id) {
  const entry = entries.get(id);
  entry.upgraded = true;
  return { socket: entry.socket, head: Buffer.from(entry.reader.buf) };
}

function httpReqClose(id) {
  const entry = entries.get(id);
  entries.delete(id);
  if (entry && !entry.upgraded) entry.ready.then(() => entry.socket.destroy(), () => {});
}

module.exports = { connectFor, dialWith, takeUpgrade, io: { httpReqBegin, httpReqClose, httpReqEnd, httpReqRead, httpReqResponse, httpReqWrite } };
