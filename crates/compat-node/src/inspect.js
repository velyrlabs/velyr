'use strict';
// util.inspect, written for this runtime to produce the same layout as Node (compact=3 grouping,
// 80-column break, circular refs, typed arrays, ...). Known limits: Proxy and Promise state cannot
// be observed from JS (a promise prints <state unavailable>), numericSeparator and getters are
// not supported, string width is its length (no East Asian width).
const types = require('internal/types');
const { colors, styles, groupArrayElements, widthOf } = require('internal/inspect_layout');

const kObjectType = 0;
const kArrayType = 1;
const kArrayExtrasType = 2;

const hasOwn = (object, key) => Object.prototype.hasOwnProperty.call(object, key);
const isEnumerable = (object, key) => Object.prototype.propertyIsEnumerable.call(object, key);

const defaultOptions = {
  showHidden: false, depth: 2, colors: false, customInspect: true, showProxy: false,
  maxArrayLength: 100, maxStringLength: 10000, breakLength: 80, compact: 3, sorted: false,
  getters: false, numericSeparator: false,
};

const stylizeNoColor = (text) => text;
function stylizeWithColor(text, styleType) {
  const color = colors[styles[styleType]];
  return color ? `\u001b[${color[0]}m${text}\u001b[${color[1]}m` : text;
}

function inspect(value, options) {
  const ctx = { ...defaultOptions, budget: {}, indentationLvl: 0, seen: [], currentDepth: 0, stylize: stylizeNoColor };
  if (arguments.length > 1) {
    if (typeof options === 'boolean') {
      ctx.showHidden = options;
      if (arguments.length > 2 && arguments[2] !== undefined) ctx.depth = arguments[2];
      if (arguments.length > 3 && arguments[3] !== undefined) ctx.colors = arguments[3];
    } else if (options) {
      for (const key of Object.keys(options)) if (hasOwn(defaultOptions, key) || key === 'stylize') ctx[key] = options[key];
    }
  }
  if (ctx.colors) ctx.stylize = stylizeWithColor;
  if (ctx.maxArrayLength === null) ctx.maxArrayLength = Infinity;
  if (ctx.maxStringLength === null) ctx.maxStringLength = Infinity;
  if (ctx.depth === null) ctx.depth = Infinity;
  return formatValue(ctx, value, 0);
}

inspect.custom = Symbol.for('nodejs.util.inspect.custom');
inspect.colors = colors;
inspect.styles = styles;
Object.defineProperty(inspect, 'defaultOptions', {
  get: () => defaultOptions,
  set(options) {
    if (options === null || typeof options !== 'object') throw require('internal/errors').invalidArgType('options', 'Object', options);
    Object.assign(defaultOptions, options);
  },
});

const strEscapeRe = /[\x00-\x1f\x27\x5c\x7f-\x9f]|[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/g;
const controlNames = { 8: '\\b', 9: '\\t', 10: '\\n', 12: '\\f', 13: '\\r', 39: "\\'", 92: '\\\\' };
function escapeChar(char) {
  const code = char.charCodeAt(0);
  if (controlNames[code]) return controlNames[code];
  if (code >= 0xd800 && code <= 0xdfff) return `\\u${code.toString(16).toUpperCase()}`;
  return `\\x${code.toString(16).toUpperCase().padStart(2, '0')}`;
}

function strEscape(text) {
  let quote = "'";
  if (text.includes("'")) {
    if (!text.includes('"')) quote = '"';
    else if (!text.includes('`') && !text.includes('${')) quote = '`';
  }
  const escaped = text.replace(strEscapeRe, (char) => (char === "'" && quote !== "'" ? char : escapeChar(char)));
  return `${quote}${escaped}${quote}`;
}

function formatNumber(stylize, number) {
  return stylize(Object.is(number, -0) ? '-0' : `${number}`, 'number');
}

function formatPrimitive(stylize, value, ctx) {
  switch (typeof value) {
    case 'string': {
      let trailer = '';
      if (value.length > ctx.maxStringLength) {
        const remaining = value.length - ctx.maxStringLength;
        value = value.slice(0, ctx.maxStringLength);
        trailer = `... ${remaining} more character${remaining > 1 ? 's' : ''}`;
      }
      if (ctx.compact !== true && value.length > 16 && value.length > ctx.breakLength - ctx.indentationLvl - 4) {
        const lines = value.split(/(?<=\n)/).map((line) => stylize(strEscape(line), 'string'));
        return lines.join(` +\n${' '.repeat(ctx.indentationLvl + 2)}`) + trailer;
      }
      return stylize(strEscape(value), 'string') + trailer;
    }
    case 'number': return formatNumber(stylize, value);
    case 'bigint': return stylize(`${value}n`, 'bigint');
    case 'boolean': return stylize(`${value}`, 'boolean');
    case 'undefined': return stylize('undefined', 'undefined');
    default: return stylize(Symbol.prototype.toString.call(value), 'symbol');
  }
}

function formatValue(ctx, value, recurseTimes, typedArray) {
  if (typeof value !== 'object' && typeof value !== 'function') return formatPrimitive(ctx.stylize, value, ctx);
  if (value === null) return ctx.stylize('null', 'null');
  if (ctx.customInspect) {
    const custom = value[inspect.custom];
    if (typeof custom === 'function' && custom !== inspect && !(value.constructor && value.constructor.prototype === value)) {
      const depth = ctx.depth - recurseTimes;
      const userOptions = { ...ctx, depth, stylize: ctx.stylize };
      delete userOptions.budget; delete userOptions.seen; delete userOptions.indentationLvl; delete userOptions.currentDepth;
      const result = custom.call(value, depth, userOptions, inspect);
      if (result !== value) {
        if (typeof result !== 'string') return formatValue(ctx, result, recurseTimes);
        return result.replace(/\n/g, `\n${' '.repeat(ctx.indentationLvl)}`);
      }
    }
  }
  if (ctx.seen.includes(value)) {
    if (ctx.circular === undefined) ctx.circular = new Map();
    let index = ctx.circular.get(value);
    if (index === undefined) {
      index = ctx.circular.size + 1;
      ctx.circular.set(value, index);
    }
    return ctx.stylize(`[Circular *${index}]`, 'special');
  }
  return formatRaw(ctx, value, recurseTimes, typedArray);
}

function constructorName(object) {
  let first;
  for (let current = object; current !== null && current !== undefined; ) {
    const descriptor = Object.getOwnPropertyDescriptor(current, 'constructor');
    if (descriptor && typeof descriptor.value === 'function' && descriptor.value.name !== '') {
      let instance = false;
      try { instance = object instanceof descriptor.value; } catch { /* keep looking */ }
      if (instance) return String(descriptor.value.name);
    }
    current = Object.getPrototypeOf(current);
    if (first === undefined) first = current;
  }
  return first === null ? null : `${Object.prototype.toString.call(object).slice(8, -1)} <Complex prototype>`;
}

function getPrefix(constructor, tag, fallback, size = '') {
  if (constructor === null) {
    return tag !== '' && fallback !== tag
      ? `[${fallback}${size}: null prototype] [${tag}] `
      : `[${fallback}${size}: null prototype] `;
  }
  return tag !== '' && constructor !== tag ? `${constructor}${size} [${tag}] ` : `${constructor}${size} `;
}

const isIndex = (key) => typeof key === 'string' && /^(0|[1-9][0-9]*)$/.test(key) && Number(key) < 2 ** 32 - 1;

function ownKeys(value, showHidden) {
  const names = showHidden ? Object.getOwnPropertyNames(value) : Object.keys(value);
  const symbols = Object.getOwnPropertySymbols(value);
  return names.concat(showHidden ? symbols : symbols.filter((symbol) => isEnumerable(value, symbol)));
}

const classRe = /^class[\s{]/;

function functionBase(value, constructor, tag) {
  const source = Function.prototype.toString.call(value);
  if (classRe.test(source)) {
    let base = `class ${(hasOwn(value, 'name') && value.name) || '(anonymous)'}`;
    if (constructor !== 'Function' && constructor !== null) base += ` [${constructor}]`;
    if (tag !== '' && constructor !== tag) base += ` [${tag}]`;
    if (constructor !== null) {
      const parent = Object.getPrototypeOf(value).name;
      if (parent) base += ` extends ${parent}`;
    } else {
      base += ' extends [null prototype]';
    }
    return `[${base}]`;
  }
  let type = 'Function';
  if (types.isGeneratorFunction(value)) type = `Generator${type}`;
  if (types.isAsyncFunction(value)) type = `Async${type}`;
  let base = `[${type}`;
  if (constructor === null) base += ' (null prototype)';
  base += value.name === '' ? ' (anonymous)' : `: ${value.name}`;
  base += ']';
  if (constructor !== type && constructor !== null) base += ` ${constructor}`;
  if (tag !== '' && constructor !== tag) base += ` [${tag}]`;
  return base;
}

function boxedBase(value, ctx, keys, constructor, tag) {
  const [type, valueOf] = types.isNumberObject(value) ? ['Number', Number.prototype.valueOf]
    : types.isStringObject(value) ? ['String', String.prototype.valueOf]
    : types.isBooleanObject(value) ? ['Boolean', Boolean.prototype.valueOf]
    : types.isBigIntObject?.(value) ? ['BigInt', BigInt.prototype.valueOf]
    : ['Symbol', Symbol.prototype.valueOf];
  if (type === 'String') keys.splice(0, value.length);
  let base = `[${type}`;
  if (type !== constructor) base += constructor === null ? ' (null prototype)' : ` (${constructor})`;
  base += `: ${formatPrimitive(stylizeNoColor, valueOf.call(value), ctx)}]`;
  if (tag !== '' && tag !== constructor) base += ` [${tag}]`;
  return keys.length !== 0 || ctx.stylize === stylizeNoColor ? base : ctx.stylize(base, type.toLowerCase());
}

function errorBase(error, constructor, tag, ctx, keys) {
  const stack = error.stack ? String(error.stack) : Error.prototype.toString.call(error);
  if (!ctx.showHidden) {
    for (const name of ['name', 'message', 'stack']) {
      const at = keys.indexOf(name);
      if (at !== -1 && (typeof error[name] !== 'string' || stack.includes(error[name]))) keys.splice(at, 1);
    }
  }
  if ('cause' in error && !keys.includes('cause')) keys.push('cause');
  if (Array.isArray(error.errors) && !keys.includes('errors')) keys.push('errors');
  let text = stack;
  let from = error.message ? stack.indexOf(error.message) : -1;
  if (from !== -1) from += error.message.length;
  if (text.indexOf('\n    at', from === -1 ? 0 : from) === -1) text = `[${text}]`;
  if (ctx.indentationLvl !== 0) text = text.replace(/\n/g, `\n${' '.repeat(ctx.indentationLvl)}`);
  return text;
}

function formatRaw(ctx, value, recurseTimes, typedArray) {
  const constructor = constructorName(value);
  let tag = value[Symbol.toStringTag];
  if (typeof tag !== 'string' || (tag !== '' && (ctx.showHidden ? hasOwn : isEnumerable)(value, Symbol.toStringTag))) tag = '';
  let base = '';
  let formatter = () => [];
  let braces;
  let keys;
  let extrasType = kObjectType;
  const filterIndexes = (all) => ownKeys(value, all).filter((key) => !isIndex(key));
  const typedKind = types.isTypedArray(value) ? Object.getOwnPropertyDescriptor(Object.getPrototypeOf(Uint8Array.prototype), Symbol.toStringTag).get.call(value) : undefined;

  if (Array.isArray(value)) {
    const prefix = constructor !== 'Array' || tag !== '' ? getPrefix(constructor, tag, 'Array', `(${value.length})`) : '';
    keys = filterIndexes(ctx.showHidden);
    braces = [`${prefix}[`, ']'];
    if (value.length === 0 && keys.length === 0) return `${braces[0]}]`;
    extrasType = kArrayExtrasType;
    formatter = formatArray;
  } else if (types.isSet(value)) {
    const size = Object.getOwnPropertyDescriptor(Set.prototype, 'size').get.call(value);
    const prefix = getPrefix(constructor, tag, 'Set', `(${size})`);
    keys = ownKeys(value, ctx.showHidden);
    if (size === 0 && keys.length === 0) return `${prefix}{}`;
    braces = [`${prefix}{`, '}'];
    formatter = formatSet;
  } else if (types.isMap(value)) {
    const size = Object.getOwnPropertyDescriptor(Map.prototype, 'size').get.call(value);
    const prefix = getPrefix(constructor, tag, 'Map', `(${size})`);
    keys = ownKeys(value, ctx.showHidden);
    if (size === 0 && keys.length === 0) return `${prefix}{}`;
    braces = [`${prefix}{`, '}'];
    formatter = formatMap;
  } else if (typedKind !== undefined) {
    keys = filterIndexes(ctx.showHidden);
    const prefix = getPrefix(constructor, tag, typedKind, `(${value.length})`);
    braces = [`${prefix}[`, ']'];
    if (value.length === 0 && keys.length === 0 && !ctx.showHidden) return `${braces[0]}]`;
    formatter = formatTypedArray;
    extrasType = kArrayExtrasType;
  } else {
    keys = ownKeys(value, ctx.showHidden);
    braces = ['{', '}'];
    if (typeof value === 'function') {
      base = functionBase(value, constructor, tag);
      if (keys.length === 0) return ctx.stylize(base, 'special');
    } else if (types.isArgumentsObject(value)) {
      braces[0] = '[Arguments] [';
      braces[1] = ']';
      keys = filterIndexes(ctx.showHidden);
      extrasType = kArrayExtrasType;
      formatter = formatArray;
    } else if (constructor === 'Object') {
      if (tag !== '') braces[0] = `${getPrefix(constructor, tag, 'Object')}{`;
      if (keys.length === 0) return `${braces[0]}}`;
    } else if (types.isRegExp(value)) {
      base = RegExp.prototype.toString.call(value);
      const prefix = getPrefix(constructor, tag, 'RegExp');
      if (prefix !== 'RegExp ') base = `${prefix}${base}`;
      if (keys.length === 0 || recurseTimes > ctx.depth) return ctx.stylize(base, 'regexp');
    } else if (types.isDate(value)) {
      base = Number.isNaN(Date.prototype.getTime.call(value)) ? Date.prototype.toString.call(value) : Date.prototype.toISOString.call(value);
      const prefix = getPrefix(constructor, tag, 'Date');
      if (prefix !== 'Date ') base = `${prefix}${base}`;
      if (keys.length === 0) return ctx.stylize(base, 'date');
    } else if (types.isNativeError(value)) {
      base = errorBase(value, constructor, tag, ctx, keys);
      if (keys.length === 0) return base;
    } else if (types.isAnyArrayBuffer(value)) {
      const prefix = getPrefix(constructor, tag, types.isArrayBuffer(value) ? 'ArrayBuffer' : 'SharedArrayBuffer');
      if (typedArray === undefined) formatter = formatArrayBuffer;
      else if (keys.length === 0) return `${prefix}{ byteLength: ${formatNumber(ctx.stylize, value.byteLength)} }`;
      braces[0] = `${prefix}{`;
      keys.unshift('byteLength');
    } else if (types.isDataView(value)) {
      braces[0] = `${getPrefix(constructor, tag, 'DataView')}{`;
      keys.unshift('byteLength', 'byteOffset', 'buffer');
    } else if (types.isPromise(value)) {
      braces[0] = `${getPrefix(constructor, tag, 'Promise')}{`;
      formatter = () => [ctx.stylize('<state unavailable>', 'special')];
    } else if (types.isWeakSet(value) || types.isWeakMap(value)) {
      braces[0] = `${getPrefix(constructor, tag, types.isWeakSet(value) ? 'WeakSet' : 'WeakMap')}{`;
      formatter = () => [ctx.stylize('<items unknown>', 'special')];
    } else if (types.isBoxedPrimitive(value)) {
      base = boxedBase(value, ctx, keys, constructor, tag);
      if (keys.length === 0) return base;
    } else {
      if (keys.length === 0) return `${getPrefix(constructor, tag, 'Object')}{}`;
      braces[0] = `${getPrefix(constructor, tag, 'Object')}{`;
    }
  }

  if (recurseTimes > ctx.depth) {
    const name = getPrefix(constructor, tag, 'Object').slice(0, -1);
    return ctx.stylize(constructor !== null ? `[${name}]` : name, 'special');
  }
  recurseTimes += 1;
  ctx.seen.push(value);
  ctx.currentDepth = recurseTimes;
  const output = formatter(ctx, value, recurseTimes);
  for (const key of keys) output.push(formatProperty(ctx, value, recurseTimes, key, extrasType));
  if (ctx.circular !== undefined) {
    const index = ctx.circular.get(value);
    if (index !== undefined) {
      const reference = ctx.stylize(`<ref *${index}>`, 'special');
      if (ctx.compact !== true) base = base === '' ? reference : `${reference} ${base}`;
      else braces[0] = `${reference} ${braces[0]}`;
    }
  }
  ctx.seen.pop();
  if (ctx.sorted) {
    const compare = ctx.sorted === true ? undefined : ctx.sorted;
    if (extrasType === kObjectType) output.sort(compare);
    else if (keys.length > 1) output.splice(output.length - keys.length, keys.length, ...output.slice(output.length - keys.length).sort(compare));
  }
  return reduceToSingleString(ctx, output, base, braces, extrasType, recurseTimes, value);
}

const remainingText = (count, unit = 'item') => `... ${count} more ${unit}${count > 1 ? 's' : ''}`;

function formatArray(ctx, value, recurseTimes) {
  const length = Math.min(Math.max(0, ctx.maxArrayLength), value.length);
  const output = [];
  let index = 0;
  const keys = Object.keys(value);
  for (const key of keys) {
    if (output.length >= length || !isIndex(key)) break;
    const at = Number(key);
    if (at > index) {
      const holes = at - index;
      output.push(ctx.stylize(`<${holes} empty item${holes > 1 ? 's' : ''}>`, 'undefined'));
      index = at;
      if (output.length === length) break;
    }
    output.push(formatProperty(ctx, value, recurseTimes, key, kArrayType));
    index++;
  }
  const remaining = value.length - index;
  if (output.length !== length) {
    if (remaining > 0) output.push(ctx.stylize(`<${remaining} empty item${remaining > 1 ? 's' : ''}>`, 'undefined'));
  } else if (remaining > 0) {
    output.push(remainingText(remaining));
  }
  return output;
}

function formatTypedArray(ctx, value) {
  const length = Math.min(Math.max(0, ctx.maxArrayLength), value.length);
  const output = [];
  for (let i = 0; i < length; i++) {
    output.push(typeof value[i] === 'bigint' ? ctx.stylize(`${value[i]}n`, 'bigint') : formatNumber(ctx.stylize, value[i]));
  }
  if (value.length > length) output.push(remainingText(value.length - length));
  return output;
}

function formatSet(ctx, value, recurseTimes) {
  const output = [];
  ctx.indentationLvl += 2;
  for (const item of Set.prototype.values.call(value)) output.push(formatValue(ctx, item, recurseTimes));
  ctx.indentationLvl -= 2;
  return output;
}

function formatMap(ctx, value, recurseTimes) {
  const output = [];
  ctx.indentationLvl += 2;
  for (const [key, item] of Map.prototype.entries.call(value)) {
    output.push(`${formatValue(ctx, key, recurseTimes)} => ${formatValue(ctx, item, recurseTimes)}`);
  }
  ctx.indentationLvl -= 2;
  return output;
}

function formatArrayBuffer(ctx, value) {
  const bytes = new Uint8Array(value);
  const shown = Math.min(ctx.maxArrayLength, bytes.length);
  let hex = Array.from(bytes.subarray(0, shown), (byte) => byte.toString(16).padStart(2, '0')).join(' ');
  if (bytes.length > shown) hex += ` ${remainingText(bytes.length - shown, 'byte')}`;
  return [`${ctx.stylize('[Uint8Contents]', 'special')}: <${hex}>`];
}

const keyRe = /^[a-zA-Z_][a-zA-Z_0-9]*$/;

function formatProperty(ctx, value, recurseTimes, key, type) {
  let extra = ' ';
  const descriptor = Object.getOwnPropertyDescriptor(value, key) || { value: value[key], enumerable: true };
  let text;
  if (descriptor.value !== undefined) {
    const diff = ctx.compact !== true || type !== kObjectType ? 2 : 3;
    ctx.indentationLvl += diff;
    text = formatValue(ctx, descriptor.value, recurseTimes);
    if (diff === 3 && ctx.breakLength < text.length) extra = `\n${' '.repeat(ctx.indentationLvl)}`;
    ctx.indentationLvl -= diff;
  } else if (descriptor.get !== undefined) {
    text = ctx.stylize(descriptor.set !== undefined ? '[Getter/Setter]' : '[Getter]', 'special');
  } else if (descriptor.set !== undefined) {
    text = ctx.stylize('[Setter]', 'special');
  } else {
    text = ctx.stylize('undefined', 'undefined');
  }
  if (type === kArrayType) return text;
  let name;
  if (typeof key === 'symbol') name = `[${ctx.stylize(Symbol.prototype.toString.call(key), 'symbol')}]`;
  else if (key === '__proto__') name = "['__proto__']";
  else if (descriptor.enumerable === false) name = `[${key.replace(strEscapeRe, escapeChar)}]`;
  else if (keyRe.test(key)) name = ctx.stylize(key, 'name');
  else name = ctx.stylize(strEscape(key), 'string');
  return `${name}:${extra}${text}`;
}


function isBelowBreakLength(ctx, output, start, base) {
  let total = output.length + start;
  if (total + output.length > ctx.breakLength) return false;
  for (const entry of output) {
    total += ctx.colors ? widthOf(entry, true) : entry.length;
    if (total > ctx.breakLength) return false;
  }
  return base === '' || !base.includes('\n');
}

function reduceToSingleString(ctx, output, base, braces, extrasType, recurseTimes, value) {
  const prefix = base ? `${base} ` : '';
  if (ctx.compact !== true) {
    if (typeof ctx.compact === 'number' && ctx.compact >= 1) {
      const entries = output.length;
      if (extrasType === kArrayExtrasType && entries > 6) output = groupArrayElements(ctx, output, value);
      if (ctx.currentDepth - recurseTimes < ctx.compact && entries === output.length) {
        const start = output.length + ctx.indentationLvl + braces[0].length + base.length + 10;
        if (isBelowBreakLength(ctx, output, start, base)) {
          const joined = output.join(', ');
          if (!joined.includes('\n')) return `${prefix}${braces[0]} ${joined} ${braces[1]}`;
        }
      }
    }
    const indentation = `\n${' '.repeat(ctx.indentationLvl)}`;
    return `${prefix}${braces[0]}${indentation}  ${output.join(`,${indentation}  `)}${indentation}${braces[1]}`;
  }
  if (isBelowBreakLength(ctx, output, 0, base)) {
    return `${braces[0]}${base ? ` ${base}` : ''} ${output.join(', ')} ${braces[1]}`;
  }
  const indentation = `\n${' '.repeat(ctx.indentationLvl)}`;
  const lead = base === '' && braces[0].length === 1 ? ' ' : `${base ? ` ${base}` : ''}${indentation}  `;
  return `${braces[0]}${lead}${output.join(`,${indentation}  `)} ${braces[1]}`;
}

module.exports = { inspect, stylizeNoColor, formatNumber, strEscape, defaultOptions };
