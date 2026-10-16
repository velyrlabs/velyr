'use strict';
// util.types. Brand checks call a prototype method or getter that throws on foreign values, so
// they cannot be fooled by Symbol.toStringTag. Limits: isProxy is always false (a Proxy cannot be
// told apart from its target in plain JS) and isPromise/isGeneratorObject/isModuleNamespaceObject
// rely on the toString tag plus the prototype chain.

const tag = (value) => Object.prototype.toString.call(value);
const getter = (proto, key) => Object.getOwnPropertyDescriptor(proto, key).get;
const brand = (call) => (value) => {
  try {
    call(value);
    return true;
  } catch {
    return false;
  }
};

const typedArrayTag = getter(Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag);
const typedArrayKind = (value) => (ArrayBuffer.isView(value) ? typedArrayTag.call(value) : undefined);
const isTypedArray = (value) => typedArrayKind(value) !== undefined;
const kind = (name) => (value) => typedArrayKind(value) === name;

const isMap = brand((v) => getter(Map.prototype, 'size').call(v));
const isSet = brand((v) => getter(Set.prototype, 'size').call(v));
const isWeakMap = brand((v) => WeakMap.prototype.has.call(v, {}));
const isWeakSet = brand((v) => WeakSet.prototype.has.call(v, {}));
const isArrayBuffer = brand((v) => getter(ArrayBuffer.prototype, 'byteLength').call(v));
const isSharedArrayBuffer = typeof SharedArrayBuffer === 'function'
  ? brand((v) => getter(SharedArrayBuffer.prototype, 'byteLength').call(v))
  : () => false;
const isBoxed = (valueOf, primitive) => (value) =>
  typeof value === 'object' && value !== null && brand((v) => valueOf.call(v))(value) && typeof valueOf.call(value) === primitive;

const isBooleanObject = isBoxed(Boolean.prototype.valueOf, 'boolean');
const isNumberObject = isBoxed(Number.prototype.valueOf, 'number');
const isStringObject = isBoxed(String.prototype.valueOf, 'string');
const isSymbolObject = isBoxed(Symbol.prototype.valueOf, 'symbol');
const isBigIntObject = isBoxed(BigInt.prototype.valueOf, 'bigint');

const GeneratorFunction = Object.getPrototypeOf(function* () {}).constructor;
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;

module.exports = {
  isAnyArrayBuffer: (v) => isArrayBuffer(v) || isSharedArrayBuffer(v),
  isArgumentsObject: (v) => typeof v === 'object' && v !== null && tag(v) === '[object Arguments]',
  isArrayBuffer,
  isAsyncFunction: (v) => typeof v === 'function' && v instanceof AsyncFunction,
  isBigIntObject,
  isBigInt64Array: kind('BigInt64Array'),
  isBigUint64Array: kind('BigUint64Array'),
  isBooleanObject,
  isBoxedPrimitive: (v) => isNumberObject(v) || isStringObject(v) || isBooleanObject(v) || isBigIntObject(v) || isSymbolObject(v),
  isDataView: brand((v) => getter(DataView.prototype, 'byteLength').call(v)),
  isDate: brand((v) => Date.prototype.getTime.call(v)),
  isExternal: () => false,
  isFloat32Array: kind('Float32Array'),
  isFloat64Array: kind('Float64Array'),
  isGeneratorFunction: (v) => typeof v === 'function' && v instanceof GeneratorFunction,
  isGeneratorObject: (v) => typeof v === 'object' && v !== null && tag(v) === '[object Generator]',
  isInt16Array: kind('Int16Array'),
  isInt32Array: kind('Int32Array'),
  isInt8Array: kind('Int8Array'),
  isMap,
  isMapIterator: (v) => typeof v === 'object' && v !== null && tag(v) === '[object Map Iterator]',
  isModuleNamespaceObject: (v) => typeof v === 'object' && v !== null && tag(v) === '[object Module]',
  isNativeError: (v) => typeof v === 'object' && v !== null && (v instanceof Error || tag(v) === '[object Error]'),
  isNumberObject,
  isPromise: (v) => v instanceof Promise && tag(v) === '[object Promise]',
  isProxy: () => false,
  isRegExp: (v) => typeof v === 'object' && v !== null && brand((x) => getter(RegExp.prototype, 'source').call(x))(v),
  isSet,
  isSetIterator: (v) => typeof v === 'object' && v !== null && tag(v) === '[object Set Iterator]',
  isSharedArrayBuffer,
  isStringObject,
  isSymbolObject,
  isTypedArray,
  isUint16Array: kind('Uint16Array'),
  isUint32Array: kind('Uint32Array'),
  isUint8Array: kind('Uint8Array'),
  isUint8ClampedArray: kind('Uint8ClampedArray'),
  isWeakMap,
  isWeakSet,
};
