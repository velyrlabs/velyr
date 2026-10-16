'use strict';
// Minimal `ws` look-alike, written for these tests (the real package is not downloaded).
// Kept from ws: WebSocketServer({ noServer } | { server }) with handleUpgrade(req, socket, head, cb),
// 'connection' / 'message' / 'close' / 'ping' / 'pong' events, send() of text and binary frames,
// ping(), close(code), the raw `101` handshake written to the upgrade socket, and a refusal that
// ends the socket with a plain HTTP response (ws's abortHandshake).
// Left out: extensions (permessage-deflate), subprotocols, fragmented messages, UTF-8 validation,
// frames over 4 GiB, backpressure on send(), verifyClient, and a client over node:http: the client
// here runs on node:net because node:http clients cannot upgrade in this runtime.
const crypto = require('crypto');
const net = require('net');
const { EventEmitter } = require('events');
const { Buffer } = require('buffer');

const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
const acceptKey = (key) => crypto.createHash('sha1').update(key + GUID).digest('base64');

function frame(opcode, payload, mask) {
  const length = payload.length;
  const head = [0x80 | opcode];
  const bit = mask ? 0x80 : 0;
  if (length < 126) head.push(bit | length);
  else if (length < 65536) head.push(bit | 126, length >> 8, length & 255);
  else head.push(bit | 127, 0, 0, 0, 0, (length >>> 24) & 255, (length >>> 16) & 255, (length >>> 8) & 255, length & 255);
  if (!mask) return Buffer.concat([Buffer.from(head), payload]);
  const key = crypto.randomBytes(4);
  const body = Buffer.from(payload);
  for (let i = 0; i < body.length; i++) body[i] ^= key[i & 3];
  return Buffer.concat([Buffer.from(head), key, body]);
}

class WebSocket extends EventEmitter {
  constructor(address, options, internals) {
    super();
    this.readyState = 0;
    this._buffer = Buffer.alloc(0);
    this._mask = false;
    this._socket = null;
    if (internals) {
      this._attach(internals.socket, internals.head, false);
    } else {
      this._connect(new URL(address));
    }
  }

  _attach(socket, head, mask) {
    this._socket = socket;
    this._mask = mask;
    this.readyState = 1;
    socket.on('data', (data) => this._feed(data));
    socket.on('close', () => {
      this.readyState = 3;
      this.emit('close', this._code ?? 1006);
    });
    socket.on('error', (error) => this.emit('error', error));
    if (head && head.length) this._feed(head);
  }

  _connect(url) {
    this._mask = true;
    const key = crypto.randomBytes(16).toString('base64');
    const socket = net.connect({ host: url.hostname, port: Number(url.port) }, () => {
      socket.write(`GET ${url.pathname}${url.search} HTTP/1.1\r\nHost: ${url.host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ${key}\r\nSec-WebSocket-Version: 13\r\n\r\n`);
    });
    let pending = Buffer.alloc(0);
    const onHandshake = (data) => {
      pending = Buffer.concat([pending, data]);
      const end = pending.indexOf('\r\n\r\n');
      if (end === -1) return;
      socket.off('data', onHandshake);
      const head = pending.subarray(0, end).toString();
      const status = Number(head.split(' ')[1]);
      if (status !== 101 || !head.toLowerCase().includes(`sec-websocket-accept: ${acceptKey(key).toLowerCase()}`)) {
        this.emit('unexpected-response', status, head);
        socket.destroy();
        return;
      }
      this._attach(socket, pending.subarray(end + 4), true);
      this.emit('open');
    };
    socket.on('data', onHandshake);
    socket.on('error', (error) => this.emit('error', error));
  }

  _feed(data) {
    this._buffer = Buffer.concat([this._buffer, data]);
    for (;;) {
      const buf = this._buffer;
      if (buf.length < 2) return;
      let length = buf[1] & 127;
      let offset = 2;
      if (length === 126) {
        if (buf.length < 4) return;
        length = buf.readUInt16BE(2);
        offset = 4;
      } else if (length === 127) {
        if (buf.length < 10) return;
        length = buf.readUInt32BE(6);
        offset = 10;
      }
      const masked = (buf[1] & 128) !== 0;
      if (buf.length < offset + (masked ? 4 : 0) + length) return;
      const key = masked ? buf.subarray(offset, offset + 4) : null;
      if (masked) offset += 4;
      const payload = Buffer.from(buf.subarray(offset, offset + length));
      if (key) for (let i = 0; i < payload.length; i++) payload[i] ^= key[i & 3];
      this._buffer = buf.subarray(offset + length);
      this._frame(buf[0] & 15, payload);
    }
  }

  _frame(opcode, payload) {
    if (opcode === 1) this.emit('message', payload.toString(), false);
    else if (opcode === 2) this.emit('message', payload, true);
    else if (opcode === 8) {
      this._code = payload.length >= 2 ? payload.readUInt16BE(0) : 1005;
      if (this.readyState === 1) this._socket.write(frame(8, payload, this._mask));
      this.readyState = 2;
      this._socket.end();
    } else if (opcode === 9) {
      this.emit('ping', payload);
      this._socket.write(frame(10, payload, this._mask));
    } else if (opcode === 10) this.emit('pong', payload);
  }

  send(data) {
    if (this.readyState !== 1) throw new Error('WebSocket is not open');
    const binary = typeof data !== 'string';
    this._socket.write(frame(binary ? 2 : 1, binary ? Buffer.from(data) : Buffer.from(data), this._mask));
  }

  ping(data = Buffer.alloc(0)) {
    this._socket.write(frame(9, Buffer.from(data), this._mask));
  }

  close(code = 1000) {
    if (this.readyState !== 1) return;
    this.readyState = 2;
    const payload = Buffer.alloc(2);
    payload.writeUInt16BE(code);
    this._socket.write(frame(8, payload, this._mask));
  }
}

class WebSocketServer extends EventEmitter {
  constructor(options = {}) {
    super();
    this.clients = new Set();
    if (options.server) {
      this._server = options.server;
      this._onUpgrade = (request, socket, head) => this.handleUpgrade(request, socket, head, (ws) => this.emit('connection', ws, request));
      options.server.on('upgrade', this._onUpgrade);
    }
  }

  handleUpgrade(request, socket, head, callback) {
    const key = request.headers['sec-websocket-key'];
    if (request.method !== 'GET' || String(request.headers.upgrade).toLowerCase() !== 'websocket' || !key || request.headers['sec-websocket-version'] !== '13') {
      const body = 'Bad Request';
      socket.once('finish', socket.destroy);
      socket.end(`HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Type: text/plain\r\nContent-Length: ${body.length}\r\n\r\n${body}`);
      return;
    }
    socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${acceptKey(key)}\r\n\r\n`);
    const ws = new WebSocket(null, null, { socket, head });
    this.clients.add(ws);
    ws.on('close', () => this.clients.delete(ws));
    callback(ws, request);
  }

  close(callback) {
    if (this._server) this._server.off('upgrade', this._onUpgrade);
    for (const ws of this.clients) ws.close(1001);
    if (callback) process.nextTick(callback);
  }
}

WebSocket.WebSocket = WebSocket;
WebSocket.WebSocketServer = WebSocketServer;
WebSocket.Server = WebSocketServer;
module.exports = WebSocket;
