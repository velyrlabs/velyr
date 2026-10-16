'use strict';
// node:string_decoder. Holds the incomplete tail of multi-byte input (utf8, utf16le, base64) until
// the next write; the rest is decoded by the Rust ops.
const errors = require('internal/errors');
const ops = require('internal/ops');
const { getEncoding } = require('internal/encodings');

const toBytes = (buf) => {
  if (!ArrayBuffer.isView(buf)) throw errors.invalidArgType('buf', ['Buffer', 'TypedArray', 'DataView'], buf);
  return new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
};

function join(a, b) {
  const out = new Uint8Array(a.length + b.length);
  out.set(a);
  out.set(b, a.length);
  return out;
}

// Returns how many trailing bytes of `bytes` must wait for more input, and the size of the
// unit they belong to.
function incompleteTail(encoding, bytes) {
  const length = bytes.length;
  if (encoding === 'base64' || encoding === 'base64url') return { held: length % 3, total: 3 };
  if (encoding === 'utf16le') {
    let held = length % 2;
    const even = length - held;
    // A high surrogate at the end waits for its low half.
    if (even >= 2 && bytes[even - 1] >= 0xd8 && bytes[even - 1] <= 0xdb) return { held: held + 2, total: 4 };
    return { held, total: 2 };
  }
  for (let back = 1; back <= Math.min(3, length); back++) {
    const byte = bytes[length - back];
    if ((byte & 0xc0) === 0x80) continue;
    const total = byte >= 0xf0 && byte <= 0xf4 ? 4 : byte >= 0xe0 && byte < 0xf0 ? 3 : byte >= 0xc2 && byte < 0xe0 ? 2 : 1;
    return total > back ? { held: back, total } : { held: 0, total: 1 };
  }
  return { held: 0, total: 1 };
}

class StringDecoder {
  constructor(encoding) {
    this.encoding = getEncoding(encoding);
    this.lastNeed = 0;
    this.lastTotal = 0;
    this._pending = new Uint8Array(0);
  }

  write(buf) {
    if (typeof buf === 'string') return buf;
    const bytes = toBytes(buf);
    if (this.encoding !== 'utf8' && this.encoding !== 'utf16le' && this.encoding !== 'base64' && this.encoding !== 'base64url') {
      return ops.decode(bytes, this.encoding);
    }
    const all = this._pending.length === 0 ? bytes : join(this._pending, bytes);
    const { held, total } = incompleteTail(this.encoding, all);
    this._pending = all.slice(all.length - held);
    this.lastTotal = held === 0 ? 0 : total;
    this.lastNeed = held === 0 ? 0 : total - held;
    return ops.decode(all.subarray(0, all.length - held), this.encoding);
  }

  end(buf) {
    let text = buf === undefined ? '' : this.write(buf);
    if (this._pending.length > 0) {
      text += this.encoding === 'utf8' ? '�' : ops.decode(this._pending, this.encoding);
    }
    this._pending = new Uint8Array(0);
    this.lastNeed = 0;
    this.lastTotal = 0;
    return text;
  }
}

module.exports = { StringDecoder };
