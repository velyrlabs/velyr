'use strict';
// Internal: Node-style errors. Each carries the Node `code`; `name` stays the base class name.
// Kernel errors (VELYR_*) never pass through here, so their identity is untouched.

function make(Base, code, message) {
  const error = new Base(message);
  Object.defineProperty(error, 'code', { value: code, enumerable: true, writable: true, configurable: true });
  return error;
}

function inspect(value, options) {
  return require('util').inspect(value, options);
}

function describe(actual) {
  if (actual === undefined || actual === null) return ` Received ${actual}`;
  if (typeof actual === 'function') return ` Received function ${actual.name}`;
  if (typeof actual === 'object') {
    const name = actual.constructor && actual.constructor.name;
    return name ? ` Received an instance of ${name}` : ` Received ${inspect(actual, { depth: -1 })}`;
  }
  let shown = inspect(actual, { colors: false });
  if (shown.length > 28) shown = `${shown.slice(0, 25)}...`;
  return ` Received type ${typeof actual} (${shown})`;
}

function joinTypes(list) {
  if (list.length === 1) return list[0];
  if (list.length === 2) return `${list[0]} or ${list[1]}`;
  return `${list.slice(0, -1).join(', ')}, or ${list[list.length - 1]}`;
}

function article(types) {
  const list = Array.isArray(types) ? types : [types];
  const primitives = [];
  const instances = [];
  for (const type of list) {
    if (/^[a-z]/.test(type)) primitives.push(type);
    else if (type === 'Function' || type === 'Object') primitives.push(type.toLowerCase());
    else instances.push(type);
  }
  const parts = [];
  if (primitives.length) {
    parts.push(`of type ${joinTypes(primitives)}`);
  }
  if (instances.length) {
    parts.push(`an instance of ${joinTypes(instances)}`);
  }
  return parts.join(' or ');
}

exports.invalidArgType = (name, expected, actual) => {
  const subject = name.includes('.') ? `property "${name}"` : `"${name}" argument`;
  return make(TypeError, 'ERR_INVALID_ARG_TYPE', `The ${subject} must be ${article(expected)}.${describe(actual)}`);
};
exports.invalidArgValue = (name, value, reason = 'is invalid') =>
  make(TypeError, 'ERR_INVALID_ARG_VALUE', `The argument '${name}' ${reason}. Received ${inspect(value)}`);
exports.outOfRange = (name, range, value) =>
  make(RangeError, 'ERR_OUT_OF_RANGE', `The value of "${name}" is out of range. It must be ${range}. Received ${inspect(value)}`);
exports.missingArgs = (...names) =>
  make(TypeError, 'ERR_MISSING_ARGS', `The ${names.map((n) => `"${n}"`).join(' and ')} argument${names.length > 1 ? 's' : ''} must be specified`);
exports.make = make;
exports.describe = describe;

exports.validateString = (value, name) => {
  if (typeof value !== 'string') throw exports.invalidArgType(name, 'string', value);
};
exports.validateFunction = (value, name) => {
  if (typeof value !== 'function') throw exports.invalidArgType(name, 'Function', value);
};
exports.validateObject = (value, name) => {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw exports.invalidArgType(name, 'Object', value);
};
