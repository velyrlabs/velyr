'use strict';
// Deep equality for assert and util.isDeepStrictEqual (loose and strict modes, cycle-safe).
const types = require('internal/types');

const hasOwn = (o, k) => Object.prototype.hasOwnProperty.call(o, k);
const tagOf = (v) => Object.prototype.toString.call(v);
const enumerableSymbols = (o) => Object.getOwnPropertySymbols(o).filter((s) => Object.prototype.propertyIsEnumerable.call(o, s));

function isBoxedSame(a, b) {
  if (types.isNumberObject(a)) return types.isNumberObject(b) && Object.is(Number.prototype.valueOf.call(a), Number.prototype.valueOf.call(b));
  if (types.isStringObject(a)) return types.isStringObject(b) && String.prototype.valueOf.call(a) === String.prototype.valueOf.call(b);
  if (types.isBooleanObject(a)) return types.isBooleanObject(b) && Boolean.prototype.valueOf.call(a) === Boolean.prototype.valueOf.call(b);
  if (types.isBigIntObject(a)) return types.isBigIntObject(b) && BigInt.prototype.valueOf.call(a) === BigInt.prototype.valueOf.call(b);
  return types.isSymbolObject(b) && Symbol.prototype.valueOf.call(a) === Symbol.prototype.valueOf.call(b);
}

function deepEqual(a, b, strict, memo) {
  if (a === b) return a !== 0 || !strict || Object.is(a, b);
  if (strict) {
    if (typeof a !== 'object') return typeof a === 'number' && Number.isNaN(a) && Number.isNaN(b);
    if (typeof b !== 'object' || a === null || b === null) return false;
    if (Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) return false;
  } else {
    if (a === null || typeof a !== 'object') {
      if (b === null || typeof b !== 'object') return a == b || (Number.isNaN(a) && Number.isNaN(b)); // eslint-disable-line eqeqeq
      return false;
    }
    if (b === null || typeof b !== 'object') return false;
  }
  const tag = tagOf(a);
  if (tag !== tagOf(b)) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a) && a.length !== b.length) return false;
  if (types.isDate(a)) {
    if (!types.isDate(b) || Date.prototype.getTime.call(a) !== Date.prototype.getTime.call(b)) return false;
  } else if (types.isRegExp(a)) {
    if (!types.isRegExp(b) || a.source !== b.source || a.flags !== b.flags || a.lastIndex !== b.lastIndex) return false;
  } else if (types.isNativeError(a) || a instanceof Error) {
    if ((!types.isNativeError(b) && !(b instanceof Error)) || a.message !== b.message || a.name !== b.name) return false;
    if (strict && !deepEqual(a.cause, b.cause, strict, memo)) return false;
  } else if (types.isBoxedPrimitive(a)) {
    if (!isBoxedSame(a, b)) return false;
  } else if (types.isTypedArray(a)) {
    if (!types.isTypedArray(b) || a.length !== b.length || (strict && tag !== tagOf(b))) return false;
    for (let i = 0; i < a.length; i++) {
      if (strict ? !Object.is(a[i], b[i]) : a[i] != b[i] && !(Number.isNaN(a[i]) && Number.isNaN(b[i]))) return false; // eslint-disable-line eqeqeq
    }
  } else if (types.isAnyArrayBuffer(a)) {
    if (!types.isAnyArrayBuffer(b) || a.byteLength !== b.byteLength) return false;
    const x = new Uint8Array(a);
    const y = new Uint8Array(b);
    for (let i = 0; i < x.length; i++) if (x[i] !== y[i]) return false;
  }
  memo = memo || { a: new Map(), b: new Map() };
  const seenA = memo.a.get(a);
  if (seenA !== undefined && seenA === memo.b.get(b)) return true;
  memo.a.set(a, memo.a.size);
  memo.b.set(b, memo.b.size);
  try {
    return compareKeys(a, b, strict, memo);
  } finally {
    memo.a.delete(a);
    memo.b.delete(b);
  }
}

function compareKeys(a, b, strict, memo) {
  const skipIndexes = types.isTypedArray(a) || types.isStringObject(a);
  const keysA = Object.keys(a).filter((k) => !(skipIndexes && /^\d+$/.test(k)));
  const keysB = Object.keys(b).filter((k) => !(skipIndexes && /^\d+$/.test(k)));
  if (keysA.length !== keysB.length) return false;
  for (const key of keysA) if (!hasOwn(b, key) || !Object.prototype.propertyIsEnumerable.call(b, key)) return false;
  if (strict) {
    const symbolsA = enumerableSymbols(a);
    if (symbolsA.length !== enumerableSymbols(b).length) return false;
    for (const symbol of symbolsA) if (!Object.prototype.propertyIsEnumerable.call(b, symbol)) return false;
    keysA.push(...symbolsA);
  }
  if (types.isSet(a)) {
    if (!types.isSet(b) || a.size !== b.size || !setsEqual(a, b, strict, memo)) return false;
  } else if (types.isMap(a)) {
    if (!types.isMap(b) || a.size !== b.size || !mapsEqual(a, b, strict, memo)) return false;
  } else if (types.isSet(b) || types.isMap(b)) {
    return false;
  }
  for (const key of keysA) if (!deepEqual(a[key], b[key], strict, memo)) return false;
  return true;
}

const isObjectLike = (v) => typeof v === 'object' && v !== null;

function setsEqual(a, b, strict, memo) {
  const pending = [];
  for (const item of b) if (isObjectLike(item) || !strict) pending.push(item);
  for (const item of a) {
    if (!isObjectLike(item) && strict) {
      if (!b.has(item)) return false;
      continue;
    }
    const at = pending.findIndex((other) => deepEqual(item, other, strict, memo));
    if (at === -1) return false;
    pending.splice(at, 1);
  }
  return true;
}

function mapsEqual(a, b, strict, memo) {
  const pending = [...b];
  for (const [key, value] of a) {
    if (!isObjectLike(key) && strict && b.has(key)) {
      if (!deepEqual(value, b.get(key), strict, memo)) return false;
      const at = pending.findIndex(([k]) => k === key);
      if (at !== -1) pending.splice(at, 1);
      continue;
    }
    const at = pending.findIndex(([k, v]) => deepEqual(key, k, strict, memo) && deepEqual(value, v, strict, memo));
    if (at === -1) return false;
    pending.splice(at, 1);
  }
  return true;
}

module.exports = {
  isDeepEqual: (a, b) => deepEqual(a, b, false),
  isDeepStrictEqual: (a, b) => deepEqual(a, b, true),
};
