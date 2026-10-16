'use strict';
// node:dns/promises. Written for this runtime.
//
// lookup() asks the resolver io-net installed (getaddrinfo on the blocking pool by default), so it
// follows /etc/hosts and the system resolver and needs no permission; it grants none either:
// connecting to the answer is still authorized by net-connect. resolve*() and reverse() send real
// DNS queries through hickory-resolver (src/dns_query.rs): to the system servers with no
// permission, or to the servers given to setServers(), which need net-connect for each of them.
//
// Known limits: getServers() lists only what setServers() set (the system servers are not
// exposed); `hints` is ignored (getaddrinfo uses the system configuration); `lookup()` reports
// every resolver failure as ENOTFOUND (no EAI_AGAIN split); nothing is cached between queries;
// dns.setLocalAddress, lookupService, `cancel()`'s effect and the legacy error constants are absent.
const errors = require('internal/errors');
const ops = require('internal/ops');
const { isIP, parseIp } = require('internal/net/ip');

const ORDERS = ['verbatim', 'ipv4first', 'ipv6first'];
let defaultOrder = 'verbatim';

function notFound(hostname) {
  const error = errors.make(Error, 'ENOTFOUND', `getaddrinfo ENOTFOUND ${hostname}`);
  error.errno = -3008;
  error.syscall = 'getaddrinfo';
  error.hostname = hostname;
  return error;
}

const FAMILIES = { undefined: 0, 0: 0, 4: 4, 6: 6, IPv4: 4, IPv6: 6 };

function familyOf(value) {
  const family = FAMILIES[value];
  if (family === undefined) throw errors.invalidArgValue('family', value, 'must be one of: 0, 4, 6');
  return family;
}

function sortOrder(options) {
  if (options.order !== undefined) {
    if (!ORDERS.includes(options.order)) throw errors.invalidArgValue('order', options.order, `must be one of: ${ORDERS.join(', ')}`);
    return options.order;
  }
  if (options.verbatim !== undefined) return options.verbatim ? 'verbatim' : 'ipv4first';
  return defaultOrder;
}

/** Resolves to `[{ address, family }]` (never empty). */
async function resolveAddresses(hostname, family, order) {
  const literal = isIP(hostname);
  if (literal !== 0) {
    if (family !== 0 && family !== literal) throw notFound(hostname);
    return [{ address: hostname, family: literal }];
  }
  let text;
  try {
    text = await ops.dnsLookup(hostname);
  } catch {
    throw notFound(hostname);
  }
  const seen = new Set();
  const found = [];
  for (const address of text === '' ? [] : text.split('\n')) {
    const kind = isIP(address);
    if (kind === 0 || (family !== 0 && kind !== family) || seen.has(address)) continue;
    seen.add(address);
    found.push({ address, family: kind });
  }
  if (found.length === 0) throw notFound(hostname);
  if (order !== 'verbatim') {
    const first = order === 'ipv4first' ? 4 : 6;
    found.sort((a, b) => (b.family === first) - (a.family === first));
  }
  return found;
}

async function lookup(hostname, options) {
  if (typeof options === 'number' || typeof options === 'string') options = { family: options };
  options = options ?? {};
  if (typeof options !== 'object') throw errors.invalidArgType('options', ['number', 'object'], options);
  if (typeof hostname !== 'string') throw errors.invalidArgType('hostname', 'string', hostname);
  const family = familyOf(options.family);
  const order = sortOrder(options);
  if (hostname === '') return options.all ? [] : { address: null, family: family === 6 ? 6 : 4 };
  const found = await resolveAddresses(hostname, family, order);
  return options.all ? found : found[0];
}

// ---- DNS queries ---------------------------------------------------------------------------------
const PUBLIC = { A: 'resolve4', AAAA: 'resolve6', ANY: 'resolveAny', CAA: 'resolveCaa', CNAME: 'resolveCname', MX: 'resolveMx',
  NAPTR: 'resolveNaptr', NS: 'resolveNs', PTR: 'resolvePtr', SOA: 'resolveSoa', SRV: 'resolveSrv', TXT: 'resolveTxt' };
const strip = ({ type, ttl, ...fields }) => fields;
// How each record type is shaped for the caller; `options` carries `{ ttl: true }`.
const SHAPES = {
  A: (record, options) => (options?.ttl ? { address: record.address, ttl: record.ttl } : record.address),
  CAA: ({ critical, tag, value }) => ({ critical, [tag]: value }),
  CNAME: (record) => record.value,
  MX: strip,
  NAPTR: strip,
  NS: (record) => record.value,
  PTR: (record) => record.value,
  SRV: strip,
  TXT: (record) => record.entries,
};
SHAPES.AAAA = SHAPES.A;

function queryError(error, rrtype, hostname) {
  if (error?.code === 'VELYR_PERMISSION_DENIED') {
    const denied = errors.make(Error, 'ERR_ACCESS_DENIED', error.message);
    Object.defineProperty(denied, 'cause', { value: error, writable: true, configurable: true });
    return denied;
  }
  const match = /^(E[A-Z]+)\|/.exec(String(error?.message));
  if (!match) return error;
  const syscall = `query${rrtype[0]}${rrtype.slice(1).toLowerCase()}`;
  const failure = errors.make(Error, match[1], `${syscall} ${match[1]} ${hostname}`);
  return Object.assign(failure, { errno: match[1], syscall, hostname });
}

// Node's notation: the port is left out when it is 53; IPv6 with a port is bracketed.
const formatServer = ({ host, port }) => (port === 53 ? host : isIP(host) === 6 ? `[${host}]:${port}` : `${host}:${port}`);

function parseServer(text) {
  let host = text;
  let port = 53;
  const bracket = /^\[([^\]]+)\](?::(\d+))?$/.exec(text);
  const plain = /^([^:]+):(\d+)$/.exec(text);
  if (bracket) [, host, port = 53] = bracket;
  else if (plain) [, host, port] = plain;
  port = Number(port);
  if (typeof text !== 'string' || !isIP(host) || !(port > 0 && port < 65536)) throw errors.invalidArgValue('servers', text, 'is not an IP address with an optional port');
  return { host, port };
}

// The PTR name of an IP literal: octets reversed under in-addr.arpa, nibbles under ip6.arpa.
function reverseName(ip) {
  const { family, value } = parseIp(ip);
  if (family === 4) return `${[...Array(4)].map((_, i) => Number((value >> BigInt(8 * i)) & 0xffn)).join('.')}.in-addr.arpa`;
  return `${[...value.toString(16).padStart(32, '0')].reverse().join('.')}.ip6.arpa`;
}

const SERVERS = Symbol('servers');

async function query(resolver, hostname, rrtype) {
  if (typeof hostname !== 'string') throw errors.invalidArgType('name', 'string', hostname);
  const servers = resolver[SERVERS].map(({ host, port }) => `${host} ${port}`).join('\n');
  try {
    return JSON.parse(await ops.dnsQuery(hostname, rrtype, servers));
  } catch (error) {
    throw queryError(error, rrtype, hostname);
  }
}

class Resolver {
  constructor() {
    Object.defineProperty(this, SERVERS, { value: [], writable: true });
  }

  getServers() {
    return this[SERVERS].map(formatServer);
  }

  setServers(servers) {
    if (!Array.isArray(servers)) throw errors.invalidArgType('servers', 'Array', servers);
    this[SERVERS] = servers.map(parseServer);
  }

  cancel() {}

  async resolve(hostname, rrtype = 'A') {
    if (typeof rrtype !== 'string') throw errors.invalidArgType('rrtype', 'string', rrtype);
    if (!Object.hasOwn(PUBLIC, rrtype)) throw errors.invalidArgValue('rrtype', rrtype, 'is not a supported record type');
    return this[PUBLIC[rrtype]](hostname);
  }

  async resolveSoa(hostname) {
    const [soa] = await query(this, hostname, 'SOA');
    return strip(soa);
  }

  async resolveAny(hostname) {
    return query(this, hostname, 'ANY');
  }

  async reverse(ip) {
    if (!isIP(ip)) throw errors.make(TypeError, 'ERR_INVALID_ARG_VALUE', `The argument 'ip' is invalid. Received '${ip}'`);
    return (await query(this, reverseName(ip), 'PTR')).map(SHAPES.PTR);
  }
}
for (const [rrtype, name] of Object.entries(PUBLIC)) {
  if (!Object.hasOwn(Resolver.prototype, name)) {
    const method = async function (hostname, options) {
      return (await query(this, hostname, rrtype)).map((record) => SHAPES[rrtype](record, options));
    };
    Object.defineProperty(Resolver.prototype, name, { value: method, writable: true, configurable: true });
  }
}

const shared = new Resolver();
const bound = (name) => (...args) => shared[name](...args);
const api = {
  Resolver,
  getDefaultResultOrder: () => defaultOrder,
  lookup,
  setDefaultResultOrder(order) {
    if (!ORDERS.includes(order)) throw errors.invalidArgValue('order', order, `must be one of: ${ORDERS.join(', ')}`);
    defaultOrder = order;
  },
};
for (const name of ['getServers', 'setServers', 'resolve', 'reverse', ...Object.values(PUBLIC)]) api[name] = bound(name);

module.exports = api;
