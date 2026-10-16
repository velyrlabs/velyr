'use strict';
// node:tls over Velyr.startTls / Velyr.listenTls (io-tls, rustls). Written for this runtime.
//
// A client connects with io-net (so net-connect is authorized exactly as for node:net) and then
// upgrades the socket; a server hands out sockets whose handshake already succeeded. The peer is
// always verified, by the platform verifier plus `ca`; there is no way to turn that off.
//
// Known limits: `rejectUnauthorized: false`, client certificates (`cert`/`key` on connect,
// `requestCert` on a server), `pfx`, a custom `checkServerIdentity`, and wrapping an existing
// socket (`socket` option, `new TLSSocket(socket)`) throw ERR_VELYR_UNSUPPORTED. `ciphers`,
// `minVersion`, `maxVersion` and session options are ignored (rustls picks the safe defaults:
// TLS 1.2 and 1.3). A server drops a client whose handshake fails or times out without emitting
// 'tlsClientError'. Sockets have `authorized`, `encrypted` and `alpnProtocol` but no
// getPeerCertificate()/getCipher().
const errors = require('internal/errors');
const net = require('net');
const { isIP, parseIp } = require('internal/net/ip');

const { normalizeConnect, unsupported, velyr } = net._internal;

// What rustls (aws-lc-rs) can negotiate, under the names Node uses (lower case, like OpenSSL).
const CIPHERS = [
  'tls_aes_256_gcm_sha384', 'tls_aes_128_gcm_sha256', 'tls_chacha20_poly1305_sha256',
  'ecdhe-ecdsa-aes256-gcm-sha384', 'ecdhe-ecdsa-aes128-gcm-sha256', 'ecdhe-ecdsa-chacha20-poly1305',
  'ecdhe-rsa-aes256-gcm-sha384', 'ecdhe-rsa-aes128-gcm-sha256', 'ecdhe-rsa-chacha20-poly1305',
];

function pem(value, name) {
  if (typeof value === 'string') return value;
  // Built on use: loading this module must not need the encoding globals.
  if (value instanceof Uint8Array || value instanceof ArrayBuffer) return new TextDecoder().decode(value);
  throw errors.invalidArgType(name, ['string', 'Buffer'], value);
}

const pemList = (value, name) => (value === undefined ? [] : (Array.isArray(value) ? value : [value]).map((item) => pem(item, name)));

class SecureContext {
  constructor(options = {}) {
    errors.validateObject(options, 'options');
    if (options.pfx !== undefined) throw unsupported('PKCS#12 (pfx)');
    Object.defineProperties(this, {
      ca: { value: pemList(options.ca, 'options.ca') },
      cert: { value: pemList(options.cert, 'options.cert').join('\n') },
      key: { value: pemList(options.key, 'options.key').join('\n') },
    });
  }
}

const createSecureContext = (options) => new SecureContext(options);

class TLSSocket extends net.Socket {
  constructor(socket, options = {}) {
    if (socket !== undefined && socket !== null) throw unsupported('Wrapping an existing socket in TLSSocket');
    super({ allowHalfOpen: options.allowHalfOpen });
    this._tlsOptions = options.tls;
    this.encrypted = true;
    this.authorized = false;
    this.alpnProtocol = false;
  }

  async _dial(host, port) {
    const plain = await velyr().connect({ hostname: await this._address(host, port), port });
    try {
      return await velyr().startTls(plain, this._tlsOptions);
    } catch (error) {
      plain.close();
      throw error;
    }
  }

  _attach(conn) {
    super._attach(conn);
    this.alpnProtocol = conn.alpnProtocol || false;
    this.authorized = this._tlsOptions !== undefined;
  }

  _connected(conn) {
    super._connected(conn);
    if (!this.destroyed) this.emit('secureConnect');
  }
}

function clientOptions(options, host) {
  if (options.socket !== undefined) throw unsupported('The `socket` option of tls.connect()');
  if (options.rejectUnauthorized === false) throw unsupported('rejectUnauthorized: false (the peer is always verified)');
  if (options.cert !== undefined || options.key !== undefined || options.pfx !== undefined) throw unsupported('Client certificates');
  if (options.checkServerIdentity !== undefined && options.checkServerIdentity !== checkServerIdentity) throw unsupported('A custom checkServerIdentity()');
  const context = options.secureContext;
  return {
    hostname: options.servername || host,
    caCerts: [...(context?.ca ?? []), ...pemList(options.ca, 'options.ca')],
    alpnProtocols: (options.ALPNProtocols ?? []).map(String),
    handshakeTimeoutMs: options.handshakeTimeout,
  };
}

function connect(...args) {
  const callback = typeof args[args.length - 1] === 'function' ? args.pop() : undefined;
  const [first, second, third] = args;
  const options = first !== null && typeof first === 'object' ? { ...first }
    : { ...(typeof second === 'object' ? second : third), port: first, host: typeof second === 'string' ? second : undefined };
  const { port, host } = normalizeConnect([{ port: options.port, host: options.host }]);
  const tls = clientOptions(options, host);
  const socket = new TLSSocket(undefined, { allowHalfOpen: options.allowHalfOpen, tls });
  if (callback) socket.once('secureConnect', callback);
  if (options.timeout) socket.setTimeout(options.timeout);
  return socket.connect({ port, host, lookup: options.lookup, family: options.family });
}

class Server extends net.Server {
  constructor(options, listener) {
    if (typeof options === 'function') [options, listener] = [{}, options];
    options = options ?? {};
    errors.validateObject(options, 'options');
    super({ allowHalfOpen: options.allowHalfOpen });
    if (options.requestCert) throw unsupported('Client certificate authentication (requestCert)');
    const context = options.secureContext ?? new SecureContext(options);
    if (!context.cert || !context.key) throw errors.invalidArgValue('options', 'cert/key', 'needs a PEM cert and key (or a secureContext with both)');
    this._context = context;
    this._alpn = (options.ALPNProtocols ?? []).map(String);
    this._handshakeTimeout = options.handshakeTimeout;
    this._connectionEvent = 'secureConnection';
    if (listener) this.on('secureConnection', listener);
  }

  async _bind(hostname, port) {
    return velyr().listenTls({
      hostname, port, cert: this._context.cert, key: this._context.key,
      alpnProtocols: this._alpn, handshakeTimeoutMs: this._handshakeTimeout,
    });
  }

  _wrap(conn) {
    const socket = new TLSSocket(undefined, { allowHalfOpen: this._allowHalfOpen });
    socket._attach(conn);
    return socket;
  }
}

function unfqdn(host) {
  return host.replace(/\.$/, '');
}

function matches(pattern, host) {
  pattern = pattern.toLowerCase();
  if (!pattern.includes('*')) return pattern === host;
  const [first, ...rest] = pattern.split('.');
  const labels = host.split('.');
  if (rest.length < 2 || first.split('*').length !== 2 || labels.length !== rest.length + 1) return false;
  if (rest.join('.') !== labels.slice(1).join('.') || labels[0] === '') return false;
  const [head, tail] = first.split('*');
  return labels[0].length >= head.length + tail.length && labels[0].startsWith(head) && labels[0].endsWith(tail);
}

// Same contract as Node: the Error to report, or undefined when `cert` is valid for `hostname`.
// With any DNS altname present the subject CN is ignored; IPs only match IP altnames.
function checkServerIdentity(hostname, cert) {
  const host = unfqdn(String(hostname));
  const altnames = typeof cert.subjectaltname === 'string' ? cert.subjectaltname.split(', ') : [];
  const dnsNames = altnames.filter((name) => name.startsWith('DNS:')).map((name) => name.slice(4));
  const ips = altnames.filter((name) => name.startsWith('IP Address:')).map((name) => name.slice(11));
  const cn = cert.subject?.CN;
  let reason = 'Cert does not contain a DNS name';
  if (isIP(host)) {
    const wanted = parseIp(host).value;
    if (ips.some((ip) => parseIp(ip)?.value === wanted)) return undefined;
    reason = `IP: ${host} is not in the cert's list: ${ips.join(', ')}`;
  } else if (dnsNames.length > 0 || cn) {
    const names = dnsNames.length > 0 ? dnsNames : [cn];
    if (names.some((name) => matches(name, host.toLowerCase()))) return undefined;
    reason = `Host: ${host}. is not in the cert's altnames: ${names.map((name) => `DNS:${name}`).join(', ')}`;
  }
  const error = errors.make(Error, 'ERR_TLS_CERT_ALTNAME_INVALID', `Hostname/IP does not match certificate's altnames: ${reason}`);
  return Object.assign(error, { reason, host, cert });
}

module.exports = {
  DEFAULT_MAX_VERSION: 'TLSv1.3',
  DEFAULT_MIN_VERSION: 'TLSv1.2',
  SecureContext,
  Server,
  TLSSocket,
  checkServerIdentity,
  connect,
  createSecureContext,
  createServer: (options, listener) => new Server(options, listener),
  getCiphers: () => [...CIPHERS],
};
