'use strict';
// node:https over node:http's client and server and the io-tls configuration behind them. Written
// for this runtime. TLS is rustls: HTTP/1.1 is the only protocol offered (ALPN), the peer is always
// verified (`rejectUnauthorized: false` throws ERR_VELYR_UNSUPPORTED), the trust roots are the
// system ones plus `ca`, and there are no client certificates, `pfx` or `requestCert`.
const errors = require('internal/errors');
const { Buffer } = require('buffer');
const { Agent, ClientRequest, unsupported } = require('internal/http/client');
const { Server } = require('internal/http/server');

class HttpsAgent extends Agent {
  constructor(options) {
    super(options);
    this.defaultPort = 443;
    this.protocol = 'https:';
  }
}

let globalAgent = new HttpsAgent({ keepAlive: true, timeout: 5000 });

/** The first PEM of a `cert` / `key` option (a string, a Buffer, `{ pem }` or an array of them). */
function pem(value, name) {
  const first = Array.isArray(value) ? value[0] : value;
  const text = first && typeof first === 'object' && !(first instanceof Uint8Array) ? first.pem : first;
  if (typeof text === 'string') return text;
  if (text instanceof Uint8Array) return Buffer.from(text).toString();
  throw errors.make(TypeError, 'ERR_INVALID_ARG_VALUE', `The argument 'options.${name}' needs a PEM string or Buffer. Received ${String(value)}`);
}

class HttpsServer extends Server {
  constructor(options, listener) {
    if (typeof options === 'function') [listener, options] = [options, {}];
    super(options, listener);
    if (options?.pfx || options?.requestCert) throw unsupported('pfx and requestCert');
    this._tls = { cert: pem(options?.cert, 'cert'), key: pem(options?.key, 'key') };
  }
}

function createServer(options, listener) {
  return new HttpsServer(options, listener);
}

function request(input, options, callback) {
  return new ClientRequest(input, options, callback, globalAgent);
}

function get(input, options, callback) {
  const req = request(input, options, callback);
  req.end();
  return req;
}

module.exports = { Agent: HttpsAgent, Server: HttpsServer, createServer, get, request };
Object.defineProperty(module.exports, 'globalAgent', {
  get: () => globalAgent,
  set(agent) {
    globalAgent = agent;
  },
  enumerable: true,
  configurable: true,
});
