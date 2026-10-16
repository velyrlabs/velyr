'use strict';
// Internal: Buffer encoding names. Aliases map to the canonical names the Rust ops understand.
const errors = require('internal/errors');

const aliases = Object.assign(Object.create(null), {
  utf8: 'utf8', 'utf-8': 'utf8', ucs2: 'utf16le', 'ucs-2': 'utf16le', utf16le: 'utf16le', 'utf-16le': 'utf16le',
  latin1: 'latin1', binary: 'latin1', ascii: 'ascii', base64: 'base64', base64url: 'base64url', hex: 'hex',
});

// `undefined` for an unknown name; nullish and '' mean utf8, as in Node.
function normalizeEncoding(encoding) {
  if (encoding === undefined || encoding === null || encoding === '') return 'utf8';
  return typeof encoding === 'string' ? aliases[encoding.toLowerCase()] : undefined;
}

function getEncoding(encoding) {
  const name = normalizeEncoding(encoding);
  if (name === undefined) throw errors.make(TypeError, 'ERR_UNKNOWN_ENCODING', `Unknown encoding: ${encoding}`);
  return name;
}

module.exports = { normalizeEncoding, getEncoding };
