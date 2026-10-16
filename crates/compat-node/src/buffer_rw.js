'use strict';
// Numeric reads and writes of Buffer. Every method validates like Node (ERR_INVALID_ARG_TYPE,
// ERR_OUT_OF_RANGE, ERR_BUFFER_OUT_OF_BOUNDS). shortcut: one generic byte loop serves every
// width, no per-width fast paths; upgrade only if profiling shows read/write hot.
const errors = require('internal/errors');

const methods = {};
const scratch = new DataView(new ArrayBuffer(8));

function boundsError(value, length, name = 'offset') {
  if (!Number.isInteger(value)) throw errors.outOfRange(name, 'an integer', value);
  if (length < 0) throw errors.make(RangeError, 'ERR_BUFFER_OUT_OF_BOUNDS', 'Attempt to access memory outside buffer bounds');
  throw errors.outOfRange(name, `>= 0 and <= ${length}`, value);
}

function checkOffset(buf, offset, width) {
  if (offset === undefined) offset = 0;
  if (typeof offset !== 'number') throw errors.invalidArgType('offset', 'number', offset);
  if (!Number.isInteger(offset) || offset < 0 || offset > buf.length - width) boundsError(offset, buf.length - width);
  return offset;
}

function checkWidth(byteLength) {
  if (typeof byteLength !== 'number') throw errors.invalidArgType('byteLength', 'number', byteLength);
  if (!Number.isInteger(byteLength) || byteLength < 1 || byteLength > 6) throw errors.outOfRange('byteLength', '>= 1 and <= 6', byteLength);
}

function readUnsigned(buf, offset, width, littleEndian) {
  let value = 0;
  for (let i = 0; i < width; i++) value = value * 256 + buf[offset + (littleEndian ? width - 1 - i : i)];
  return value;
}

function writeInteger(buf, value, offset, width, littleEndian, min, max) {
  value = +value;
  offset = checkOffset(buf, offset, width);
  if (value > max || value < min) throw errors.outOfRange('value', `>= ${min} and <= ${max}`, value);
  let rest = Math.trunc(value < 0 ? value + 2 ** (8 * width) : value);
  for (let i = 0; i < width; i++) {
    buf[offset + (littleEndian ? i : width - 1 - i)] = rest % 256;
    rest = Math.floor(rest / 256);
  }
  return offset + width;
}

function define(name, fn, alias) {
  methods[name] = fn;
  if (alias) methods[name.replace('UInt', 'Uint')] = fn;
}

function defineInteger(signed, width, suffix) {
  const littleEndian = suffix === 'LE';
  const bits = width * 8;
  const [min, max] = signed ? [-(2 ** (bits - 1)), 2 ** (bits - 1) - 1] : [0, 2 ** bits - 1];
  const name = `${signed ? '' : 'U'}Int${bits}${suffix}`;
  define(`read${name}`, function (offset) {
    offset = checkOffset(this, offset, width);
    const value = readUnsigned(this, offset, width, littleEndian);
    return signed && value > max ? value - 2 ** bits : value;
  }, !signed);
  define(`write${name}`, function (value, offset) {
    return writeInteger(this, value, offset, width, littleEndian, min, max);
  }, !signed);
}

for (const signed of [false, true]) {
  defineInteger(signed, 1, '');
  for (const width of [2, 4]) for (const suffix of ['LE', 'BE']) defineInteger(signed, width, suffix);
}

for (const signed of [false, true]) {
  for (const suffix of ['LE', 'BE']) {
    const littleEndian = suffix === 'LE';
    const name = `${signed ? '' : 'U'}Int${suffix}`;
    define(`read${name}`, function (offset, byteLength) {
      checkWidth(byteLength);
      offset = checkOffset(this, offset, byteLength);
      const value = readUnsigned(this, offset, byteLength, littleEndian);
      return signed && value >= 2 ** (8 * byteLength - 1) ? value - 2 ** (8 * byteLength) : value;
    }, !signed);
    define(`write${name}`, function (value, offset, byteLength) {
      checkWidth(byteLength);
      const bits = 8 * byteLength;
      const [min, max] = signed ? [-(2 ** (bits - 1)), 2 ** (bits - 1) - 1] : [0, 2 ** bits - 1];
      return writeInteger(this, value, offset, byteLength, littleEndian, min, max);
    }, !signed);
  }
}

for (const signed of [false, true]) {
  for (const suffix of ['LE', 'BE']) {
    const littleEndian = suffix === 'LE';
    const name = `Big${signed ? '' : 'U'}Int64${suffix}`;
    define(`read${name}`, function (offset) {
      offset = checkOffset(this, offset, 8);
      const low = readUnsigned(this, offset + (littleEndian ? 0 : 4), 4, littleEndian);
      const high = readUnsigned(this, offset + (littleEndian ? 4 : 0), 4, littleEndian);
      const value = (BigInt(high) << 32n) | BigInt(low);
      return signed ? BigInt.asIntN(64, value) : value;
    }, !signed);
    define(`write${name}`, function (value, offset) {
      if (typeof value !== 'bigint') throw errors.invalidArgType('value', 'bigint', value);
      offset = checkOffset(this, offset, 8);
      const [min, max, range] = signed
        ? [-(2n ** 63n), 2n ** 63n - 1n, '>= -(2n ** 63n) and < 2n ** 63n']
        : [0n, 2n ** 64n - 1n, '>= 0n and < 2n ** 64n'];
      if (value < min || value > max) throw errors.outOfRange('value', range, value);
      const bits = BigInt.asUintN(64, value);
      for (let i = 0; i < 8; i++) this[offset + (littleEndian ? i : 7 - i)] = Number((bits >> BigInt(8 * i)) & 0xffn);
      return offset + 8;
    }, !signed);
  }
}

for (const [name, width] of [['Float', 4], ['Double', 8]]) {
  for (const suffix of ['LE', 'BE']) {
    const littleEndian = suffix === 'LE';
    methods[`read${name}${suffix}`] = function (offset) {
      offset = checkOffset(this, offset, width);
      for (let i = 0; i < width; i++) scratch.setUint8(i, this[offset + i]);
      return width === 4 ? scratch.getFloat32(0, littleEndian) : scratch.getFloat64(0, littleEndian);
    };
    methods[`write${name}${suffix}`] = function (value, offset) {
      value = +value;
      offset = checkOffset(this, offset, width);
      if (width === 4) scratch.setFloat32(0, value, littleEndian); else scratch.setFloat64(0, value, littleEndian);
      for (let i = 0; i < width; i++) this[offset + i] = scratch.getUint8(i);
      return offset + width;
    };
  }
}

module.exports = { methods };
