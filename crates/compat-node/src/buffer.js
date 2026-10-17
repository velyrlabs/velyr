'use strict';
// node:buffer. Buffer is a Uint8Array subclass (as in Node, via FastBuffer); transcoding between
// bytes and strings is done by Rust ops. Limits: allocUnsafe and from(string) pooling is only for
// allocUnsafe; indexOf is a naive scan; no transcode/isUtf8/isAscii/SlowBuffer; numeric range
// errors do not use Node's digit separators.
const errors = require('internal/errors');
const ops = require('internal/ops');
const types = require('internal/types');
const { normalizeEncoding, getEncoding } = require('internal/encodings');
const { methods: numeric } = require('internal/buffer_rw');

const kMaxLength = 2 ** 32;
const kStringMaxLength = 2 ** 29 - 24;
let inspectMaxBytes = 50;
const kBufferOrUint8 = ['Buffer', 'Uint8Array'];

class FastBuffer extends Uint8Array {
  // eslint-disable-next-line no-useless-constructor
  constructor(bufferOrLength, byteOffset, length) {
    super(bufferOrLength, byteOffset, length);
  }
}

function Buffer(arg, encodingOrOffset, length) {
  if (typeof arg === 'number') {
    if (typeof encodingOrOffset === 'string') throw errors.invalidArgType('string', 'string', arg);
    return Buffer.alloc(arg); // zero-filled, unlike Node's deprecated Buffer(n): safer, same shape
  }
  return Buffer.from(arg, encodingOrOffset, length);
}
Buffer.prototype = FastBuffer.prototype;
Object.defineProperty(FastBuffer.prototype, 'constructor', { value: Buffer, writable: true, configurable: true });
Object.setPrototypeOf(Buffer, Uint8Array);
Buffer.poolSize = 65536;

const isUint8 = (value) => value instanceof Uint8Array;
const bytesOf = (text, encoding) => new Uint8Array(ops.encode(text, encoding));

function validateSize(size) {
  if (typeof size !== 'number') throw errors.invalidArgType('size', 'number', size);
  if (!(size >= 0 && size <= kMaxLength)) throw errors.outOfRange('size', `>= 0 && <= ${kMaxLength}`, size);
}

function validateOffset(value, name, min = 0, max = kMaxLength) {
  if (typeof value !== 'number') throw errors.invalidArgType(name, 'number', value);
  if (!Number.isInteger(value)) throw errors.outOfRange(name, 'an integer', value);
  if (value < min || value > max) throw errors.outOfRange(name, `>= ${min} && <= ${max}`, value);
}

let pool;
let poolOffset = 0;
function allocUnsafe(size) {
  validateSize(size);
  if (size === 0) return new FastBuffer();
  if (size >= Buffer.poolSize >>> 1) return new FastBuffer(size);
  if (pool === undefined || size > pool.byteLength - poolOffset) {
    pool = new ArrayBuffer(Buffer.poolSize);
    poolOffset = 0;
  }
  const buffer = new FastBuffer(pool, poolOffset, size);
  poolOffset = (poolOffset + size + 7) & ~7;
  return buffer;
}

Buffer.allocUnsafe = allocUnsafe;
Buffer.allocUnsafeSlow = (size) => {
  validateSize(size);
  return new FastBuffer(size);
};
Buffer.alloc = (size, fill, encoding) => {
  validateSize(size);
  const buffer = new FastBuffer(size);
  return size > 0 && fill !== undefined && fill !== 0 ? buffer.fill(fill, 0, size, encoding) : buffer;
};

function outsideBuffer(what) {
  return errors.make(RangeError, 'ERR_BUFFER_OUT_OF_BOUNDS', `"${what}" is outside of buffer bounds`);
}

function fromArrayBuffer(arrayBuffer, byteOffset, length) {
  byteOffset = byteOffset === undefined ? 0 : +byteOffset || 0;
  const maxLength = arrayBuffer.byteLength - byteOffset;
  if (maxLength < 0) throw outsideBuffer('offset');
  if (length === undefined) length = maxLength;
  else if ((length = +length) > 0) {
    if (length > maxLength) throw outsideBuffer('length');
  } else length = 0;
  return new FastBuffer(arrayBuffer, byteOffset, length);
}

function fromObject(object) {
  if (object.length !== undefined || types.isAnyArrayBuffer(object.buffer)) {
    return typeof object.length === 'number' ? new FastBuffer(object) : new FastBuffer();
  }
  if (object.type === 'Buffer' && Array.isArray(object.data)) return new FastBuffer(object.data);
  return undefined;
}

// Pool-backed like Node: encode straight into the pool slice; the copy path covers hosts that cannot borrow.
function fromString(value, encoding) {
  const length = ops.byteLength(value, encoding);
  if (length === 0) return new FastBuffer();
  const buffer = allocUnsafe(length);
  if (ops.bufferWrite(buffer, value, encoding, 0, length) === length) return buffer;
  return new FastBuffer(ops.encode(value, encoding));
}

Buffer.from = function from(value, encodingOrOffset, length) {
  if (typeof value === 'string') return fromString(value, getEncoding(encodingOrOffset));
  if (typeof value === 'object' && value !== null) {
    if (types.isAnyArrayBuffer(value)) return fromArrayBuffer(value, encodingOrOffset, length);
    const valueOf = value.valueOf && value.valueOf();
    if (valueOf != null && valueOf !== value && (typeof valueOf === 'string' || typeof valueOf === 'object')) {
      return from(valueOf, encodingOrOffset, length);
    }
    const fromShape = fromObject(value);
    if (fromShape) return fromShape;
    if (typeof value[Symbol.toPrimitive] === 'function') {
      const primitive = value[Symbol.toPrimitive]('string');
      if (typeof primitive === 'string') return from(primitive, encodingOrOffset);
    }
  }
  throw errors.make(TypeError, 'ERR_INVALID_ARG_TYPE',
    `The first argument must be of type string or an instance of Buffer, ArrayBuffer, or Array or an Array-like Object.${errors.describe(value)}`);
};

Buffer.isBuffer = (value) => value instanceof Buffer;
Buffer.isEncoding = (encoding) => typeof encoding === 'string' && encoding.length !== 0 && normalizeEncoding(encoding) !== undefined;

Buffer.byteLength = (value, encoding) => {
  if (typeof value === 'string') return ops.byteLength(value, normalizeEncoding(encoding) ?? 'utf8');
  if (ArrayBuffer.isView(value) || types.isAnyArrayBuffer(value)) return value.byteLength;
  throw errors.invalidArgType('string', ['string', 'Buffer', 'ArrayBuffer'], value);
};

function compareBytes(a, b) {
  const length = Math.min(a.length, b.length);
  for (let i = 0; i < length; i++) if (a[i] !== b[i]) return a[i] < b[i] ? -1 : 1;
  return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
}

Buffer.compare = (a, b) => {
  if (!isUint8(a)) throw errors.invalidArgType('buf1', kBufferOrUint8, a);
  if (!isUint8(b)) throw errors.invalidArgType('buf2', kBufferOrUint8, b);
  return a === b ? 0 : compareBytes(a, b);
};

Buffer.concat = (list, length) => {
  if (!Array.isArray(list)) throw errors.invalidArgType('list', 'Array', list);
  if (list.length === 0) return new FastBuffer();
  if (length === undefined) {
    length = 0;
    for (let i = 0; i < list.length; i++) {
      if (!isUint8(list[i])) throw errors.invalidArgType(`list[${i}]`, kBufferOrUint8, list[i]);
      length += list[i].length;
    }
  } else validateOffset(length, 'length');
  const result = allocUnsafe(length);
  let position = 0;
  for (let i = 0; i < list.length && position < length; i++) {
    let item = list[i];
    if (!isUint8(item)) throw errors.invalidArgType(`list[${i}]`, kBufferOrUint8, item);
    if (position + item.length > length) item = item.subarray(0, length - position);
    result.set(item, position);
    position += item.length;
  }
  if (position < length) result.fill(0, position, length);
  return result;
};

const proto = Buffer.prototype;
Object.assign(proto, numeric);

proto.toString = function toString(encoding, start, end) {
  const length = this.length;
  if (arguments.length === 0) return ops.decode(this, 'utf8');
  const name = getEncoding(encoding);
  if (start === undefined || start <= 0) start = 0;
  else if (start >= length) return '';
  else start = Math.trunc(start);
  if (end === undefined || end > length) end = length;
  else end = Math.trunc(end);
  if (end <= start) return '';
  return ops.decode(this, name, start, end);
};
proto.toLocaleString = proto.toString;

// Node's undocumented `<encoding>Slice(start, end)` (undici's body reader calls `utf8Slice`).
for (const encoding of ['utf8', 'latin1', 'ascii', 'hex', 'base64', 'base64url', 'ucs2']) {
  Object.defineProperty(proto, `${encoding}Slice`, {
    value: function (start, end) { return this.toString(encoding, start, end); },
    writable: true, configurable: true,
  });
}

proto.toJSON = function toJSON() {
  return { type: 'Buffer', data: Array.from(this) };
};

proto.equals = function equals(other) {
  if (!isUint8(other)) throw errors.invalidArgType('otherBuffer', kBufferOrUint8, other);
  return this === other || (this.length === other.length && compareBytes(this, other) === 0);
};

proto.compare = function compare(target, targetStart, targetEnd, sourceStart, sourceEnd) {
  if (!isUint8(target)) throw errors.invalidArgType('target', kBufferOrUint8, target);
  if (targetStart === undefined) targetStart = 0; else validateOffset(targetStart, 'targetStart');
  if (targetEnd === undefined) targetEnd = target.length; else validateOffset(targetEnd, 'targetEnd', 0, target.length);
  if (sourceStart === undefined) sourceStart = 0; else validateOffset(sourceStart, 'sourceStart');
  if (sourceEnd === undefined) sourceEnd = this.length; else validateOffset(sourceEnd, 'sourceEnd', 0, this.length);
  if (sourceStart >= sourceEnd) return targetStart >= targetEnd ? 0 : -1;
  if (targetStart >= targetEnd) return 1;
  return compareBytes(this.subarray(sourceStart, sourceEnd), target.subarray(targetStart, targetEnd));
};

proto.copy = function copy(target, targetStart, sourceStart, sourceEnd) {
  if (!isUint8(target)) throw errors.invalidArgType('target', kBufferOrUint8, target);
  targetStart = targetStart === undefined ? 0 : Math.trunc(+targetStart) || 0;
  sourceStart = sourceStart === undefined ? 0 : Math.trunc(+sourceStart) || 0;
  sourceEnd = sourceEnd === undefined ? this.length : Math.trunc(+sourceEnd) || 0;
  if (targetStart < 0) throw errors.outOfRange('targetStart', '>= 0', targetStart);
  if (sourceStart < 0) throw errors.outOfRange('sourceStart', '>= 0', sourceStart);
  if (sourceStart > this.length) throw errors.outOfRange('sourceStart', `<= ${this.length}`, sourceStart);
  sourceEnd = Math.min(sourceEnd, this.length);
  if (targetStart >= target.length || sourceStart >= sourceEnd) return 0;
  const count = Math.min(sourceEnd - sourceStart, target.length - targetStart);
  Uint8Array.prototype.set.call(target, Uint8Array.prototype.subarray.call(this, sourceStart, sourceStart + count), targetStart);
  return count;
};

function clampIndex(value, length, fallback) {
  if (value === undefined) return fallback;
  const index = Math.trunc(+value) || 0;
  return index < 0 ? Math.max(length + index, 0) : Math.min(index, length);
}

proto.subarray = function subarray(start, end) {
  const from = clampIndex(start, this.length, 0);
  const to = Math.max(clampIndex(end, this.length, this.length), from);
  return new FastBuffer(this.buffer, this.byteOffset + from, to - from);
};
proto.slice = proto.subarray;

proto.fill = function fill(value, offset, end, encoding) {
  if (typeof offset === 'string') { encoding = offset; offset = 0; end = this.length; }
  else if (typeof end === 'string') { encoding = end; end = this.length; }
  if (offset === undefined) offset = 0; else validateOffset(offset, 'offset', 0, this.length);
  if (end === undefined) end = this.length; else validateOffset(end, 'end', 0, this.length);
  if (offset >= end) return this;
  let pattern;
  if (typeof value === 'string') {
    if (value.length === 0) value = 0;
    else {
      pattern = bytesOf(value, getEncoding(encoding));
      if (pattern.length === 0) throw errors.make(TypeError, 'ERR_INVALID_ARG_VALUE', `The argument 'value' is invalid. Received '${value}'`);
    }
  } else if (isUint8(value)) {
    pattern = value;
    if (pattern.length === 0) throw errors.make(TypeError, 'ERR_INVALID_ARG_VALUE', "The argument 'value' is invalid. Received <Buffer >");
  }
  if (pattern === undefined) return Uint8Array.prototype.fill.call(this, (+value || 0) & 255, offset, end);
  for (let i = offset, j = 0; i < end; i++, j = (j + 1) % pattern.length) this[i] = pattern[j];
  return this;
};

proto.write = function write(string, offset, length, encoding) {
  if (typeof string !== 'string') throw errors.invalidArgType('argument', 'string', string);
  if (offset === undefined) { offset = 0; length = this.length; encoding = undefined; }
  else if (length === undefined && typeof offset === 'string') { encoding = offset; length = this.length; offset = 0; }
  else {
    validateOffset(offset, 'offset', 0, this.length);
    const remaining = this.length - offset;
    if (length === undefined) length = remaining;
    else if (typeof length === 'string') { encoding = length; length = remaining; }
    else { validateOffset(length, 'length', 0, this.length); length = Math.min(length, remaining); }
  }
  const name = getEncoding(encoding);
  const written = ops.bufferWrite(this, string, name, offset, length);
  if (written >= 0) return written;
  const bytes = bytesOf(string, name);
  let count = Math.min(bytes.length, length);
  if (count < bytes.length) {
    if (name === 'utf8') while (count > 0 && (bytes[count] & 0xc0) === 0x80) count--; // never split a character
    else if (name === 'utf16le') count &= ~1;
  }
  Uint8Array.prototype.set.call(this, bytes.subarray(0, count), offset);
  return count;
};

function search(buffer, value, byteOffset, encoding, forward) {
  if (typeof byteOffset === 'string') { encoding = byteOffset; byteOffset = undefined; }
  byteOffset = +byteOffset;
  if (Number.isNaN(byteOffset)) byteOffset = forward ? 0 : buffer.length;
  let needle;
  if (typeof value === 'number') needle = [value & 255];
  else if (typeof value === 'string') needle = bytesOf(value, getEncoding(encoding));
  else if (isUint8(value)) needle = value;
  else throw errors.invalidArgType('value', ['number', 'string', 'Buffer', 'Uint8Array'], value);
  const length = buffer.length;
  if (byteOffset < 0) byteOffset += length;
  if (forward) {
    const from = Math.max(Math.trunc(byteOffset), 0);
    if (needle.length === 0) return Math.min(from, length);
    for (let i = from; i <= length - needle.length; i++) if (matchesAt(buffer, needle, i)) return i;
    return -1;
  }
  if (byteOffset < 0) return -1;
  const from = Math.min(Math.trunc(byteOffset), length - needle.length);
  if (needle.length === 0) return Math.min(Math.trunc(byteOffset), length);
  for (let i = from; i >= 0; i--) if (matchesAt(buffer, needle, i)) return i;
  return -1;
}

function matchesAt(buffer, needle, at) {
  for (let j = 0; j < needle.length; j++) if (buffer[at + j] !== needle[j]) return false;
  return true;
}

proto.indexOf = function indexOf(value, byteOffset, encoding) { return search(this, value, byteOffset, encoding, true); };
proto.lastIndexOf = function lastIndexOf(value, byteOffset, encoding) { return search(this, value, byteOffset, encoding, false); };
proto.includes = function includes(value, byteOffset, encoding) { return this.indexOf(value, byteOffset, encoding) !== -1; };

for (const bits of [16, 32, 64]) {
  const width = bits / 8;
  proto[`swap${bits}`] = function swap() {
    if (this.length % width !== 0) throw errors.make(RangeError, 'ERR_INVALID_BUFFER_SIZE', `Buffer size must be a multiple of ${bits}-bits`);
    for (let i = 0; i < this.length; i += width) {
      for (let a = i, b = i + width - 1; a < b; a++, b--) {
        const held = this[a];
        this[a] = this[b];
        this[b] = held;
      }
    }
    return this;
  };
}

proto[Symbol.for('nodejs.util.inspect.custom')] = function inspectBuffer() {
  const shown = Math.min(inspectMaxBytes, this.length);
  let text = ops.decode(new Uint8Array(this.buffer, this.byteOffset, shown), 'hex').replace(/(.{2})/g, '$1 ').trim();
  const remaining = this.length - inspectMaxBytes;
  if (remaining > 0) text += ` ... ${remaining} more byte${remaining > 1 ? 's' : ''}`;
  return `<${this.constructor.name} ${text}>`;
};

const lazyGlobal = (name) => ({ enumerable: true, configurable: true, get: () => globalThis[name] });

module.exports = {
  Buffer,
  constants: { MAX_LENGTH: kMaxLength, MAX_STRING_LENGTH: kStringMaxLength },
  kMaxLength,
  kStringMaxLength,
};
Object.defineProperties(module.exports, {
  // Web globals are installed by the host, maybe after this module loads: resolve on access.
  Blob: lazyGlobal('Blob'),
  File: lazyGlobal('File'),
  atob: lazyGlobal('atob'),
  btoa: lazyGlobal('btoa'),
  INSPECT_MAX_BYTES: {
    enumerable: true,
    get: () => inspectMaxBytes,
    set(value) {
      validateOffset(value, 'INSPECT_MAX_BYTES', 0);
      inspectMaxBytes = value;
    },
  },
});
