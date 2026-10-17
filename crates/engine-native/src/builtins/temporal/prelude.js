'use strict';
// Temporal over the `__temporal_op` native op (see mod.rs). Each object keeps one canonical
// ISO string in `slots`. This file holds the shared helpers and `Duration`;
// `prelude_plain.js` and `prelude_zoned.js` run next and pick the helpers up from
// `__temporal_shared` (the last one deletes it).
(() => {
  const global = globalThis;
  const rawOp = global.__temporal_op;
  delete global.__temporal_op;
  const { defineProperty } = Object;
  const slots = new WeakMap();
  const opText = (arg) => {
    if (typeof arg === 'symbol') throw new TypeError('Cannot convert a Symbol to a string');
    return arg === undefined ? '' : `${arg}`;
  };
  const op = (cls, method, ...args) => rawOp(cls, method, ...args.map(opText));
  // Temporal objects with a calendar slot (all iso8601) are read without touching their properties.
  const CALENDAR_SLOT_CLASSES = ['PlainDate', 'PlainDateTime', 'PlainMonthDay', 'PlainYearMonth', 'ZonedDateTime'];
  const calendarId = (value) => {
    if (typeof value === 'object' && value !== null && CALENDAR_SLOT_CLASSES.includes(slots.get(value)?.cls)) return 'iso8601';
    if (typeof value !== 'string') throw new TypeError('calendar must be a string');
    return op('Calendar', 'id', value);
  };
  const timeZoneId = (value) => {
    if (typeof value !== 'string') throw new TypeError('timeZone must be a string');
    return op('TimeZone', 'id', value);
  };

  const define = (target, methods) => {
    for (const name of Reflect.ownKeys(methods)) {
      defineProperty(target, name, { value: methods[name], writable: true, configurable: true });
    }
  };
  // Like `define`, with explicit spec `length`s (default parameters hide them); `methods` must
  // use method syntax so the functions are not constructors.
  const defineMethods = (target, methods, lengths = {}) => {
    define(target, methods);
    for (const name of Object.keys(lengths)) {
      defineProperty(methods[name], 'length', { value: lengths[name], configurable: true });
    }
  };
  const getters = (target, table) => {
    for (const name of Reflect.ownKeys(table)) {
      defineProperty(target, name, { get: table[name], configurable: true });
    }
  };
  const tag = (target, name) =>
    defineProperty(target, Symbol.toStringTag, { value: name, configurable: true });

  const wrap = (proto, cls, str) => {
    const object = Object.create(proto);
    slots.set(object, { cls, str });
    return object;
  };
  const slotOf = (value) => (typeof value === 'object' && value !== null ? slots.get(value) : undefined);
  // The canonical string of `value` when it is a `cls`.
  const strOf = (value, cls) => {
    const slot = slotOf(value);
    if (slot === undefined || slot.cls !== cls) throw new TypeError(`Receiver is not a Temporal.${cls}`);
    return slot.str;
  };
  const fieldsOf = (value, cls) => op(cls, 'fields', strOf(value, cls)).split(',');

  // ToIntegerWithTruncation, with optional bounds.
  const toInt = (value, name, min = -Infinity, max = Infinity) => {
    const n = Number(value);
    if (!Number.isFinite(n)) throw new RangeError(`${name} must be a finite number`);
    const integer = Math.trunc(n) + 0;
    if (integer < min || integer > max) throw new RangeError(`${name} is out of range`);
    return integer;
  };
  const toIntOr = (value, fallback, name, min, max) =>
    value === undefined ? fallback : toInt(value, name, min, max);

  const optionsOf = (options) => {
    if (options === undefined) return {};
    if ((typeof options !== 'object' && typeof options !== 'function') || options === null) {
      throw new TypeError('options must be an object');
    }
    return options;
  };
  // ToString of an option value; a Symbol throws TypeError.
  const optionText = (options, name) => {
    const value = options[name];
    return value === undefined ? undefined : `${value}`;
  };
  const overflowOf = (options) => {
    const overflow = optionText(optionsOf(options), 'overflow');
    if (overflow !== undefined && overflow !== 'constrain' && overflow !== 'reject') {
      throw new RangeError(`${overflow} is not a valid value for overflow`);
    }
    return overflow;
  };
  // Reads in spec order, one Get each; returns [largestUnit, smallestUnit, roundingMode,
  // roundingIncrement], the order of the native op. `withLargest` false skips largestUnit.
  const diffArgs = (options, withLargest = true) => {
    const o = optionsOf(options);
    const largest = withLargest ? optionText(o, 'largestUnit') : undefined;
    const increment = roundingIncrementText(o);
    const mode = optionText(o, 'roundingMode');
    return [largest, optionText(o, 'smallestUnit'), mode, increment];
  };
  // `roundTo` is a unit string or an options object. Duration.round needs `withLargest`;
  // the other classes pass false (their round does not read largestUnit).
  const roundArgs = (roundTo, withLargest = true) => {
    if (roundTo === undefined) throw new TypeError('roundTo is required');
    const options = typeof roundTo === 'string' ? { smallestUnit: roundTo } : optionsOf(roundTo);
    const args = diffArgs(options, withLargest);
    if (args[1] === undefined && args[0] === undefined) {
      throw new RangeError(withLargest ? 'smallestUnit or largestUnit is required' : 'smallestUnit is required');
    }
    return args;
  };
  // fractionalSecondDigits, smallestUnit, roundingMode; read in spec order.
  const stringArgs = (options) => {
    const o = optionsOf(options);
    let digits = o.fractionalSecondDigits;
    if (digits !== undefined && digits !== 'auto') digits = toInt(digits, 'fractionalSecondDigits', 0, 9);
    const mode = optionText(o, 'roundingMode');
    return [digits, optionText(o, 'smallestUnit'), mode];
  };
  const calendarSuffix = (str, options) => {
    const name = optionText(optionsOf(options), 'calendarName');
    if (name === 'always') return `${str}[u-ca=iso8601]`;
    if (name === 'critical') return `${str}[!u-ca=iso8601]`;
    return str;
  };

  // Reads `names` from a property bag in the given order (callers pass spec order), one Get
  // each; undefined properties are skipped.
  const readBag = (bag, names) => {
    const out = {};
    for (const name of names) {
      const value = bag[name];
      if (value !== undefined) out[name] = value;
    }
    return out;
  };
  const monthFromCode = (code) => {
    const text = `${code}`;
    const digits = text.slice(1);
    const valid = text.length === 3 && text[0] === 'M' && [...digits].every((c) => c >= '0' && c <= '9');
    if (!valid || digits === '00') throw new RangeError(`Invalid month code ${text}`);
    return Number(digits);
  };

  const Temporal = global.Temporal;
  tag(Temporal, 'Temporal');

  // Methods shared by every class: arithmetic, comparison and serialization.
  const installCommon = (Class, cls, fromStr, equals) => {
    const proto = Class.prototype;
    tag(proto, `Temporal.${cls}`);
    const same = (a, b) => op(cls, 'compare', a, b) === '0';
    const intrinsicToString = proto.toString;
    defineMethods(proto, {
      add(duration, options) {
        return fromStr(op(cls, 'add', strOf(this, cls), durationStr(duration), overflowOf(options)));
      },
      subtract(duration, options) {
        return fromStr(op(cls, 'subtract', strOf(this, cls), durationStr(duration), overflowOf(options)));
      },
      until(other, options) {
        const str = op(cls, 'until', strOf(this, cls), strOf(Class.from(other), cls), ...diffArgs(options));
        return wrapDuration(str);
      },
      since(other, options) {
        const str = op(cls, 'since', strOf(this, cls), strOf(Class.from(other), cls), ...diffArgs(options));
        return wrapDuration(str);
      },
      equals(other) {
        const a = strOf(this, cls);
        const b = strOf(Class.from(other), cls);
        return equals ? op(cls, 'equals', a, b) === 'true' : same(a, b);
      },
      toJSON() {
        strOf(this, cls);
        return intrinsicToString.call(this);
      },
      toLocaleString() {
        strOf(this, cls);
        return intrinsicToString.call(this);
      },
      valueOf() {
        throw new TypeError(`Temporal.${cls} cannot be converted to a primitive; use compare()`);
      },
    }, { add: 1, subtract: 1, until: 1, since: 1, equals: 1, toJSON: 0, toLocaleString: 0, valueOf: 0 });
    defineMethods(Class, {
      compare(a, b) {
        return Number(op(cls, 'compare', strOf(Class.from(a), cls), strOf(Class.from(b), cls)));
      },
    }, { compare: 2 });
  };

  // ---- Duration ----
  // A Duration slot keeps `fields` (ten comma-separated integers, exact above 999 sub-second
  // units); `str` is the ISO form other classes feed to their native ops, built on demand.
  const DURATION_UNITS = [
    'years', 'months', 'weeks', 'days', 'hours', 'minutes', 'seconds',
    'milliseconds', 'microseconds', 'nanoseconds',
  ];
  const DURATION_BAG_ORDER = [...DURATION_UNITS].sort();
  const RELATIVE_TO_FIELDS = [
    'calendar', 'day', 'hour', 'microsecond', 'millisecond', 'minute', 'month', 'monthCode',
    'nanosecond', 'offset', 'second', 'timeZone', 'year',
  ];
  // Fields hold float64-representable integers (the spec stores Numbers), so results round.
  const roundToDoubles = (fields) =>
    fields.split(',').map((field) => BigInt(Number(field)).toString()).join(',');
  const wrapDuration = (result) => {
    const fields = roundToDoubles(/^[+-]?P/i.test(result) ? op('Duration', 'parseExact', result) : result);
    const object = Object.create(Temporal.Duration.prototype);
    slots.set(object, { cls: 'Duration', fields, get str() { return op('Duration', 'iso', fields); } });
    return object;
  };
  const durationFields = (value) => {
    const slot = slotOf(value);
    if (slot === undefined || slot.cls !== 'Duration') throw new TypeError('Receiver is not a Temporal.Duration');
    return slot.fields;
  };
  // ToIntegerIfIntegral as an exact decimal string; `+` rejects Symbol and BigInt.
  const durationInt = (value, name) => {
    const n = +value;
    if (n !== n) return '0';
    if (n - n !== 0 || Math.trunc(n) !== n) throw new RangeError(`${name} must be an integer`);
    return BigInt(n).toString();
  };
  const durationFromParts = (parts) => op('Duration', 'createExact', ...parts);
  // ToTemporalPartialDurationRecord: alphabetical reads, each converted right away.
  const partialDuration = (bag) => {
    const given = {};
    for (const name of DURATION_BAG_ORDER) {
      const value = bag[name];
      if (value !== undefined) given[name] = durationInt(value, name);
    }
    if (Object.keys(given).length === 0) throw new TypeError('Duration needs at least one unit');
    return given;
  };
  const toDurationFields = (value) => {
    if (typeof value === 'string') return op('Duration', 'parseExact', value);
    if (typeof value !== 'object' || value === null) throw new TypeError('Invalid duration');
    const slot = slotOf(value);
    if (slot !== undefined && slot.cls === 'Duration') return slot.fields;
    const given = partialDuration(value);
    return durationFromParts(DURATION_UNITS.map((unit) => given[unit] ?? '0'));
  };
  const durationStr = (value) => op('Duration', 'iso', toDurationFields(value));
  const relativeToField = (name, value) => {
    if (name === 'timeZone' || name === 'calendar') {
      if (typeof value !== 'string') throw new TypeError(`${name} must be a string`);
      return value;
    }
    if (name === 'monthCode' || name === 'offset') {
      const text = typeof value === 'object' && value !== null ? `${value}` : value;
      if (typeof text !== 'string') throw new TypeError(`${name} must be a string`);
      return text;
    }
    const n = +value;
    if (n !== n || n - n !== 0) throw new RangeError(`${name} must be a finite number`);
    return Math.trunc(n) + 0;
  };
  // GetTemporalRelativeToOption: the text handed to the native op, or undefined.
  const relativeToArg = (value) => {
    if (value === undefined || typeof value === 'string') return value;
    if ((typeof value !== 'object' && typeof value !== 'function') || value === null) {
      throw new TypeError('Invalid relativeTo');
    }
    const slot = slotOf(value);
    if (slot !== undefined && ['ZonedDateTime', 'PlainDate', 'PlainDateTime'].includes(slot.cls)) return slot.str;
    const bag = {};
    for (const name of RELATIVE_TO_FIELDS) {
      const field = value[name];
      if (field !== undefined) bag[name] = relativeToField(name, field);
    }
    if (bag.timeZone === undefined) return strOf(Temporal.PlainDate.from(bag), 'PlainDate');
    return strOf(Temporal.ZonedDateTime.from(bag, { offset: 'reject' }), 'ZonedDateTime');
  };
  // `{ [key]: shorthand }` without Object.prototype lookups, or the options object itself.
  const shorthandOptions = (value, key, name) => {
    if (value === undefined) throw new TypeError(`${name} is required`);
    if (typeof value !== 'string') return optionsOf(value);
    const options = Object.create(null);
    options[key] = value;
    return options;
  };
  const roundingIncrementText = (options) => {
    const value = options.roundingIncrement;
    if (value === undefined) return undefined;
    const n = +value;
    if (n !== n || n - n !== 0) throw new RangeError('roundingIncrement must be a finite number');
    const increment = Math.trunc(n);
    if (increment < 1 || increment > 1e9) throw new RangeError('roundingIncrement is out of range');
    return `${increment}`;
  };
  // fractionalSecondDigits, smallestUnit, roundingMode in the order of the native op.
  const durationStringArgs = (options) => {
    const o = optionsOf(options);
    let digits = o.fractionalSecondDigits;
    if (typeof digits === 'number') {
      if (digits !== digits || digits - digits !== 0) throw new RangeError('fractionalSecondDigits must be finite');
      digits = Math.floor(digits);
      if (digits < 0 || digits > 9) throw new RangeError('fractionalSecondDigits is out of range');
    } else if (digits !== undefined) {
      if (`${digits}` !== 'auto') throw new RangeError('fractionalSecondDigits must be auto or a number');
      digits = 'auto';
    }
    const mode = optionText(o, 'roundingMode');
    return [digits, optionText(o, 'smallestUnit'), mode];
  };
  const durationText = (duration, options) =>
    op('Duration', 'toString', durationFields(duration), ...durationStringArgs(options));

  class Duration {
    constructor(...values) {
      const parts = DURATION_UNITS.map((unit, i) => (values[i] === undefined ? '0' : durationInt(values[i], unit)));
      const fields = durationFromParts(parts);
      slots.set(this, { cls: 'Duration', fields, get str() { return op('Duration', 'iso', fields); } });
    }

    static from(item) {
      return wrapDuration(toDurationFields(item));
    }

    static compare(one, two, options) {
      const first = toDurationFields(one);
      const second = toDurationFields(two);
      const relativeTo = relativeToArg(optionsOf(options).relativeTo);
      return Number(op('Duration', 'compare', first, second, relativeTo));
    }

    get sign() {
      const first = durationFields(this).split(',').find((field) => field !== '0');
      return first === undefined ? 0 : first.startsWith('-') ? -1 : 1;
    }

    get blank() {
      return durationFields(this).split(',').every((field) => field === '0');
    }

    negated() {
      return wrapDuration(op('Duration', 'negated', durationFields(this)));
    }

    abs() {
      return wrapDuration(op('Duration', 'abs', durationFields(this)));
    }

    add(other) {
      const fields = durationFields(this);
      return wrapDuration(op('Duration', 'add', fields, toDurationFields(other)));
    }

    subtract(other) {
      const fields = durationFields(this);
      return wrapDuration(op('Duration', 'subtract', fields, toDurationFields(other)));
    }

    with(bag) {
      const current = durationFields(this).split(',');
      const given = partialDuration(optionsOf(bag));
      return wrapDuration(durationFromParts(DURATION_UNITS.map((unit, i) => given[unit] ?? current[i])));
    }

    round(roundTo) {
      const fields = durationFields(this);
      const options = shorthandOptions(roundTo, 'smallestUnit', 'roundTo');
      const largest = optionText(options, 'largestUnit');
      const relativeTo = relativeToArg(options.relativeTo);
      const increment = roundingIncrementText(options);
      const mode = optionText(options, 'roundingMode');
      const smallest = optionText(options, 'smallestUnit');
      if (smallest === undefined && largest === undefined) {
        throw new RangeError('smallestUnit or largestUnit is required');
      }
      return wrapDuration(op('Duration', 'round', fields, largest, smallest, mode, increment, relativeTo));
    }

    total(totalOf) {
      const fields = durationFields(this);
      const options = shorthandOptions(totalOf, 'unit', 'totalOf');
      const relativeTo = relativeToArg(options.relativeTo);
      const unit = optionText(options, 'unit');
      if (unit === undefined) throw new RangeError('unit is required');
      return Number(op('Duration', 'total', fields, unit, relativeTo));
    }

    toString(options) {
      return durationText(this, options);
    }

    toJSON() {
      return durationText(this, undefined);
    }

    toLocaleString() {
      return durationText(this, undefined);
    }

    valueOf() {
      throw new TypeError('Temporal.Duration cannot be converted to a primitive; use compare()');
    }
  }
  DURATION_UNITS.forEach((unit, i) => {
    defineProperty(Duration.prototype, unit, { get() { return Number(durationFields(this).split(',')[i]); }, configurable: true });
  });
  defineProperty(Duration.compare, 'length', { value: 2, configurable: true });
  defineProperty(Duration.prototype.toString, 'length', { value: 0, configurable: true });
  tag(Duration.prototype, 'Temporal.Duration');
  define(Temporal, { Duration });

  defineProperty(global, '__temporal_shared', {
    value: {
      op, slots, define, defineMethods, calendarId, timeZoneId, getters, tag, wrap, slotOf, strOf, fieldsOf, toInt, toIntOr, optionsOf,
      optionText, overflowOf, diffArgs, roundArgs, stringArgs, calendarSuffix, readBag,
      monthFromCode, Temporal, installCommon, durationStr, wrapDuration,
    },
    configurable: true,
  });
})();
