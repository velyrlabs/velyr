'use strict';
// Internal: IP literal parsing shared by node:net and node:dns, and net.BlockList. Written for this
// runtime. Addresses are compared as 128-bit integers; an IPv4 address is held in its IPv4-mapped
// form (::ffff:a.b.c.d), so `::ffff:10.0.0.5` and `10.0.0.5` are the same address, as in Node.
//
// Known limits: BlockList takes address strings only (no SocketAddress), and has no toJSON/fromJSON.
const errors = require('internal/errors');

const V4 = /^(?:0|[1-9]\d{0,2})(?:\.(?:0|[1-9]\d{0,2})){3}$/;
const GROUP = /^[0-9a-fA-F]{1,4}$/;
const MAPPED = 0xffffn << 32n;

function parseV4(text) {
  if (!V4.test(text)) return null;
  let value = 0n;
  for (const part of text.split('.')) {
    const number = Number(part);
    if (number > 255) return null;
    value = (value << 8n) | BigInt(number);
  }
  return value;
}

function parseV6(input) {
  let text = input;
  const zone = text.indexOf('%');
  if (zone !== -1) {
    if (zone === text.length - 1) return null;
    text = text.slice(0, zone);
  }
  if (!/^[0-9a-fA-F:.]+$/.test(text)) return null;
  if (text.includes('.')) {
    const cut = text.lastIndexOf(':');
    const v4 = cut === -1 ? null : parseV4(text.slice(cut + 1));
    if (v4 === null) return null;
    text = `${text.slice(0, cut + 1)}${(v4 >> 16n).toString(16)}:${(v4 & 0xffffn).toString(16)}`;
  }
  const halves = text.split('::');
  if (halves.length > 2) return null;
  const groups = (part) => (part === '' ? [] : part.split(':'));
  const head = groups(halves[0]);
  const rest = halves.length === 2 ? groups(halves[1]) : [];
  if (halves.length === 1 ? head.length !== 8 : head.length + rest.length > 7) return null;
  const all = [...head, ...new Array(halves.length === 2 ? 8 - head.length - rest.length : 0).fill('0'), ...rest];
  if (!all.every((group) => GROUP.test(group))) return null;
  return all.reduce((value, group) => (value << 16n) | BigInt(`0x${group}`), 0n);
}

/** `{ family: 4 | 6, value }` (value is the 128-bit form) or null when `text` is not an IP literal. */
function parseIp(text) {
  if (typeof text !== 'string') return null;
  const v4 = parseV4(text);
  if (v4 !== null) return { family: 4, value: MAPPED | v4 };
  const v6 = parseV6(text);
  return v6 === null ? null : { family: 6, value: v6 };
}

const isIP = (text) => parseIp(text)?.family ?? 0;
const isIPv4 = (text) => isIP(text) === 4;
const isIPv6 = (text) => isIP(text) === 6;

function familyOf(type, name) {
  if (type === undefined) return 4;
  if (typeof type !== 'string') throw errors.invalidArgType(name, 'string', type);
  const lower = type.toLowerCase();
  if (lower !== 'ipv4' && lower !== 'ipv6') throw errors.invalidArgValue(name, type, "must be one of: 'ipv4', 'ipv6'");
  return lower === 'ipv4' ? 4 : 6;
}

function addressOf(text, family, name) {
  if (typeof text !== 'string') throw errors.invalidArgType(name, 'string', text);
  const parsed = parseIp(text);
  if (parsed === null || parsed.family !== family) throw errors.invalidArgValue(name, text, `is not an IPv${family} address`);
  return parsed.value;
}

class BlockList {
  #rules = [];

  static isBlockList(value) {
    return value instanceof BlockList;
  }

  get rules() {
    return this.#rules.map((rule) => rule.text);
  }

  addAddress(address, type) {
    const family = familyOf(type, 'family');
    const value = addressOf(address, family, 'address');
    this.#rules.push({ from: value, to: value, text: `Address: IPv${family} ${address}` });
  }

  addRange(start, end, type) {
    const family = familyOf(type, 'family');
    const from = addressOf(start, family, 'start');
    const to = addressOf(end, family, 'end');
    if (from > to) throw errors.invalidArgValue('start', start, 'must not be greater than end');
    this.#rules.push({ from, to, text: `Range: IPv${family} ${start}-${end}` });
  }

  addSubnet(network, prefix, type) {
    const family = familyOf(type, 'family');
    const base = addressOf(network, family, 'network');
    const max = family === 4 ? 32 : 128;
    if (!Number.isInteger(prefix)) throw errors.invalidArgType('prefix', 'number', prefix);
    if (prefix < 0 || prefix > max) throw errors.outOfRange('prefix', `>= 0 && <= ${max}`, prefix);
    const shift = 128n - BigInt(prefix + (family === 4 ? 96 : 0));
    const from = (base >> shift) << shift;
    this.#rules.push({ from, to: from | ((1n << shift) - 1n), text: `Subnet: IPv${family} ${network}/${prefix}` });
  }

  check(address, type) {
    const family = familyOf(type, 'family');
    const parsed = parseIp(address);
    if (parsed === null || parsed.family !== family) return false;
    return this.#rules.some((rule) => parsed.value >= rule.from && parsed.value <= rule.to);
  }
}

module.exports = { BlockList, isIP, isIPv4, isIPv6, parseIp };
