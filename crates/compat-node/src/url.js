'use strict';
// node:url. URL/URLSearchParams are the realm's WHATWG globals; the legacy parse/format/resolve API
// is implemented here (simplified: no IDNA, no punycode, ASCII-only hostnames are lowercased).
const errors = require('internal/errors');
const path = require('path');

const WHATWG = { URL: globalThis.URL, URLSearchParams: globalThis.URLSearchParams };

// node:path stays posix on every host, so the default follows the real platform instead of path.sep.
const isWindows = () => require('process').platform === 'win32';

const isURL = (value) => value !== null && typeof value === 'object' && typeof value.href === 'string' && typeof value.protocol === 'string' && value.auth === undefined && value.path === undefined;

function fileURLToPath(input, options) {
  const windows = options && options.windows !== undefined ? options.windows : isWindows();
  let url = input;
  if (typeof url === 'string') url = new WHATWG.URL(url);
  else if (!isURL(url)) throw errors.invalidArgType('path', ['string', 'URL'], input);
  if (url.protocol !== 'file:') throw errors.make(TypeError, 'ERR_INVALID_URL_SCHEME', 'The URL must be of scheme file');
  if (windows) {
    const pathname = url.pathname;
    for (let n = 0; n < pathname.length; n++) {
      if (pathname[n] === '%') {
        const third = pathname.codePointAt(n + 2) | 0x20;
        if ((pathname[n + 1] === '2' && third === 102) || (pathname[n + 1] === '5' && third === 99)) {
          throw errors.make(TypeError, 'ERR_INVALID_FILE_URL_PATH', 'File URL path must not include encoded \\ or / characters');
        }
      }
    }
    let decoded = decodeURIComponent(pathname.replace(/\//g, '\\'));
    if (url.hostname !== '') return `\\\\${url.hostname}${decoded}`;
    decoded = decoded.slice(1);
    if (!/^[a-zA-Z]:/.test(decoded) || decoded.length < 2 || (decoded.length > 2 && decoded[2] !== '\\')) {
      throw errors.make(TypeError, 'ERR_INVALID_FILE_URL_PATH', 'File URL path must be absolute');
    }
    return decoded;
  }
  if (url.hostname !== '') throw errors.make(TypeError, 'ERR_INVALID_FILE_URL_HOST', 'File URL host must be "localhost" or empty on posix');
  const pathname = url.pathname;
  if (/%2f/i.test(pathname)) throw errors.make(TypeError, 'ERR_INVALID_FILE_URL_PATH', 'File URL path must not include encoded / characters');
  return decodeURIComponent(pathname);
}

function encodePathChars(text, windows) {
  let out = text.replace(/%/g, '%25');
  if (!windows) out = out.replace(/\\/g, '%5C');
  return out.replace(/\n/g, '%0A').replace(/\r/g, '%0D').replace(/\t/g, '%09');
}

function pathToFileURL(filepath, options) {
  errors.validateString(filepath, 'path');
  const windows = options && options.windows !== undefined ? options.windows : isWindows();
  if (windows && filepath.startsWith('\\\\')) {
    const rest = filepath.slice(2);
    const slash = rest.indexOf('\\');
    if (slash === -1) throw errors.make(TypeError, 'ERR_INVALID_ARG_VALUE', `The argument 'path' must be a UNC path with a share. Received '${filepath}'`);
    const url = new WHATWG.URL('file://x/');
    url.hostname = rest.slice(0, slash);
    url.pathname = encodePathChars(rest.slice(slash).replace(/\\/g, '/'), true).replace(/\?/g, '%3F').replace(/#/g, '%23');
    return url;
  }
  let resolved = windows ? path.win32.resolve(filepath) : path.posix.resolve(filepath);
  if (windows) resolved = resolved.replace(/\\/g, '/');
  else if (filepath.endsWith('/') && !resolved.endsWith('/')) resolved += '/';
  const url = new WHATWG.URL('file://');
  url.pathname = encodePathChars(resolved, windows).replace(/\?/g, '%3F').replace(/#/g, '%23');
  return url;
}

function urlToHttpOptions(url) {
  const options = {
    protocol: url.protocol,
    hostname: typeof url.hostname === 'string' && url.hostname.startsWith('[') ? url.hostname.slice(1, -1) : url.hostname,
    hash: url.hash,
    search: url.search,
    pathname: url.pathname,
    path: `${url.pathname || ''}${url.search || ''}`,
    href: url.href,
  };
  if (url.port !== '') options.port = Number(url.port);
  if (url.username || url.password) options.auth = `${decodeURIComponent(url.username)}:${decodeURIComponent(url.password)}`;
  return options;
}

// ---- legacy API ----------------------------------------------------------------------------

function Url() {
  this.protocol = null;
  this.slashes = null;
  this.auth = null;
  this.host = null;
  this.port = null;
  this.hostname = null;
  this.hash = null;
  this.search = null;
  this.query = null;
  this.pathname = null;
  this.path = null;
  this.href = null;
}

const slashedProtocol = new Set(['http', 'http:', 'https', 'https:', 'ftp', 'ftp:', 'gopher', 'gopher:', 'file', 'file:', 'ws', 'ws:', 'wss', 'wss:']);
const protocolRe = /^[a-z0-9.+-]+:/i;
const hostEnd = /[/?#\\]/;

function parseQuery(search) {
  const result = { __proto__: null };
  for (const [key, value] of new WHATWG.URLSearchParams(search)) {
    if (key in result) {
      if (Array.isArray(result[key])) result[key].push(value); else result[key] = [result[key], value];
    } else result[key] = value;
  }
  return result;
}

Url.prototype.parse = function parse(input, parseQueryString, slashesDenoteHost) {
  errors.validateString(input, 'url');
  let rest = input.trim().replace(/\\/g, (m, offset, whole) => (whole.slice(0, offset).search(/[?#]/) === -1 ? '/' : m));
  const hashAt = rest.indexOf('#');
  if (hashAt !== -1) { this.hash = rest.slice(hashAt); rest = rest.slice(0, hashAt); }
  const queryAt = rest.indexOf('?');
  if (queryAt !== -1) { this.search = rest.slice(queryAt); this.query = rest.slice(queryAt + 1); rest = rest.slice(0, queryAt); }
  else if (parseQueryString) { this.search = ''; this.query = { __proto__: null }; }
  if (parseQueryString && this.query !== null && typeof this.query === 'string') this.query = parseQuery(this.query);

  const proto = protocolRe.exec(rest);
  let lowerProto;
  if (proto) {
    lowerProto = proto[0].toLowerCase();
    this.protocol = lowerProto;
    rest = rest.slice(proto[0].length);
  }
  let slashes = false;
  if (slashesDenoteHost || proto || /^\/\/[^@/]+@[^@/]+/.test(rest)) {
    slashes = rest.startsWith('//');
    if (slashes && !(lowerProto === 'javascript:')) { rest = rest.slice(2); this.slashes = true; }
  }
  if (slashes) {
    let end = rest.search(hostEnd);
    if (end === -1) end = rest.length;
    let host = rest.slice(0, end);
    rest = rest.slice(end);
    const at = host.lastIndexOf('@');
    if (at !== -1) { this.auth = decodeURIComponent(host.slice(0, at)); host = host.slice(at + 1); }
    const portMatch = /:([0-9]*)$/.exec(host);
    if (portMatch) {
      if (portMatch[1] !== '') this.port = portMatch[1];
      host = host.slice(0, host.length - portMatch[0].length);
    }
    this.hostname = host.startsWith('[') && host.endsWith(']') ? host.slice(1, -1) : host.toLowerCase();
    this.host = (this.hostname.includes(':') ? `[${this.hostname}]` : this.hostname) + (this.port ? `:${this.port}` : '');
    if (slashedProtocol.has(lowerProto) && rest === '') rest = '/';
  }
  if (rest !== '') this.pathname = rest;
  else if (slashedProtocol.has(lowerProto) && this.hostname) this.pathname = '/';
  if (this.pathname !== null || this.search !== null) this.path = (this.pathname || '') + (this.search || '');
  this.href = this.format();
  return this;
};

Url.prototype.format = function format() {
  let auth = this.auth || '';
  if (auth) auth = `${encodeURIComponent(auth).replace(/%3A/i, ':')}@`;
  let protocol = this.protocol || '';
  let pathname = this.pathname || '';
  let hash = this.hash || '';
  let host = '';
  let query = '';
  if (this.host) host = auth + this.host;
  else if (this.hostname) {
    host = auth + (this.hostname.includes(':') && !this.hostname.startsWith('[') ? `[${this.hostname}]` : this.hostname);
    if (this.port) host += `:${this.port}`;
  }
  if (this.query !== null && typeof this.query === 'object') query = new WHATWG.URLSearchParams(Object.entries(this.query).flatMap(([k, v]) => (Array.isArray(v) ? v.map((x) => [k, x]) : [[k, v]]))).toString();
  let search = this.search || (query && `?${query}`) || '';
  if (protocol && !protocol.endsWith(':')) protocol += ':';
  pathname = pathname.replace(/[?#]/g, encodeURIComponent);
  search = search.replace('#', '%23');
  if (this.slashes || slashedProtocol.has(protocol)) {
    if (this.slashes || host) {
      if (pathname && pathname[0] !== '/') pathname = `/${pathname}`;
      host = `//${host}`;
    } else if (protocol.length >= 4 && protocol.startsWith('file')) host = '//';
  }
  if (hash && hash[0] !== '#') hash = `#${hash}`;
  if (search && search[0] !== '?') search = `?${search}`;
  return protocol + host + pathname + search + hash;
};

Url.prototype.resolve = function resolve(relative) {
  return resolveUrl(this.href || this.format(), relative);
};

function urlParse(input, parseQueryString, slashesDenoteHost) {
  if (input instanceof Url) return input;
  const url = new Url();
  url.parse(input, parseQueryString, slashesDenoteHost);
  return url;
}

function urlFormat(value, options) {
  if (typeof value === 'string') value = urlParse(value);
  else if (value === null || typeof value !== 'object') throw errors.invalidArgType('urlObject', ['Object', 'string'], value);
  else if (isURL(value)) {
    const opts = { fragment: true, unicode: false, search: true, auth: true, ...options };
    let out = `${value.protocol}${value.host || value.protocol === 'file:' ? '//' : ''}`;
    if (opts.auth && (value.username || value.password)) out += `${value.username}${value.password ? `:${value.password}` : ''}@`;
    out += value.host + value.pathname;
    if (opts.search) out += value.search;
    if (opts.fragment) out += value.hash;
    return out;
  }
  return Url.prototype.format.call(value);
}

function resolveUrl(from, to) {
  const base = new WHATWG.URL(from, 'resolve://');
  const resolved = new WHATWG.URL(to, base);
  if (resolved.protocol === 'resolve:') return `${resolved.pathname}${resolved.search}${resolved.hash}`;
  return resolved.toString();
}

module.exports = {
  Url,
  parse: urlParse,
  resolve: resolveUrl,
  format: urlFormat,
  URL: WHATWG.URL,
  URLSearchParams: WHATWG.URLSearchParams,
  fileURLToPath,
  pathToFileURL,
  urlToHttpOptions,
};
