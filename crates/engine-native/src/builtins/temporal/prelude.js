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
  const op = (cls, method, ...args) =>
    rawOp(cls, method, ...args.map((arg) => (arg === undefined ? '' : String(arg))));

  const define = (target, methods) => {
    for (const name of Reflect.ownKeys(methods)) {
      defineProperty(target, name, { value: methods[name], writable: true, configurable: true });
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
    if (typeof options !== 'object' || options === null) throw new TypeError('options must be an object');
    return options;
  };
  const optionText = (options, name) => {
    const value = options[name];
    return value === undefined ? undefined : String(value);
  };
  const overflowOf = (options) => {
    const overflow = optionText(optionsOf(options), 'overflow');
    if (overflow !== undefined && overflow !== 'constrain' && overflow !== 'reject') {
      throw new RangeError(`${overflow} is not a valid value for overflow`);
    }
    return overflow;
  };
  const diffArgs = (options) => {
    const o = optionsOf(options);
    return ['largestUnit', 'smallestUnit', 'roundingMode', 'roundingIncrement'].map((n) => optionText(o, n));
  };
  // `roundTo` is a unit string or an options object.
  const roundArgs = (roundTo) => {
    if (roundTo === undefined) throw new TypeError('roundTo is required');
    const options = typeof roundTo === 'string' ? { smallestUnit: roundTo } : optionsOf(roundTo);
    if (options.smallestUnit === undefined && options.largestUnit === undefined) {
      throw new RangeError('smallestUnit or largestUnit is required');
    }
    return diffArgs(options);
  };
  // fractionalSecondDigits, smallestUnit, roundingMode.
  const stringArgs = (options) => {
    const o = optionsOf(options);
    let digits = o.fractionalSecondDigits;
    if (digits !== undefined && digits !== 'auto') digits = toInt(digits, 'fractionalSecondDigits', 0, 9);
    return [digits, optionText(o, 'smallestUnit'), optionText(o, 'roundingMode')];
  };
  const calendarSuffix = (str, options) => {
    const name = optionText(optionsOf(options), 'calendarName');
    if (name === 'always') return `${str}[u-ca=iso8601]`;
    if (name === 'critical') return `${str}[!u-ca=iso8601]`;
    return str;
  };

  // Reads `names` from a property bag; undefined properties are skipped.
  const readBag = (bag, names) => {
    const out = {};
    for (const name of names) {
      if (bag[name] !== undefined) out[name] = bag[name];
    }
    return out;
  };
  const monthFromCode = (code) => {
    const match = /^M(\d\d)$/.exec(String(code));
    if (match === null || match[1] === '00') throw new RangeError(`Invalid month code ${code}`);
    return Number(match[1]);
  };

  const Temporal = global.Temporal;
  tag(Temporal, 'Temporal');

  // Methods shared by every class: arithmetic, comparison and serialization.
  const installCommon = (Class, cls, fromStr, equals) => {
    const proto = Class.prototype;
    tag(proto, `Temporal.${cls}`);
    const same = (a, b) => op(cls, 'compare', a, b) === '0';
    define(proto, {
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
        return this.toString();
      },
      toLocaleString() {
        return this.toString();
      },
      valueOf() {
        throw new TypeError(`Temporal.${cls} cannot be converted to a primitive; use compare()`);
      },
    });
    define(Class, {
      compare(a, b) {
        return Number(op(cls, 'compare', strOf(Class.from(a), cls), strOf(Class.from(b), cls)));
      },
    });
  };

  // ---- Duration ----
  const DURATION_UNITS = [
    'years', 'months', 'weeks', 'days', 'hours', 'minutes', 'seconds',
    'milliseconds', 'microseconds', 'nanoseconds',
  ];
  const wrapDuration = (str) => wrap(Temporal.Duration.prototype, 'Duration', str);
  // Integers can exceed 2 ** 53 for the sub-second units, so go through BigInt.
  const durationInt = (value, name) => BigInt(toInt(value, name)).toString();
  const durationFromBag = (bag) => {
    const given = readBag(bag, DURATION_UNITS);
    if (Object.keys(given).length === 0) throw new TypeError('Duration needs at least one unit');
    return op('Duration', 'create', ...DURATION_UNITS.map((unit) => durationInt(given[unit] ?? 0, unit)));
  };
  const durationStr = (value) => {
    if (typeof value === 'string') return op('Duration', 'parse', value);
    const slot = slotOf(value);
    if (slot !== undefined && slot.cls === 'Duration') return slot.str;
    if (typeof value === 'object' && value !== null) return durationFromBag(value);
    throw new TypeError('Invalid duration');
  };
  const relativeToArg = (value) => {
    if (value === undefined || typeof value === 'string') return value;
    const slot = slotOf(value);
    if (slot !== undefined) return slot.str;
    if (typeof value !== 'object' || value === null) throw new TypeError('Invalid relativeTo');
    const zoned = value.timeZone !== undefined;
    return strOf((zoned ? Temporal.ZonedDateTime : Temporal.PlainDate).from(value), zoned ? 'ZonedDateTime' : 'PlainDate');
  };

  class Duration {
    constructor(years, months, weeks, days, hours, minutes, seconds, milliseconds, microseconds, nanoseconds) {
      const values = [years, months, weeks, days, hours, minutes, seconds, milliseconds, microseconds, nanoseconds];
      const str = op('Duration', 'create', ...values.map((v, i) => durationInt(v ?? 0, DURATION_UNITS[i])));
      slots.set(this, { cls: 'Duration', str });
    }

    static from(item) {
      return wrapDuration(durationStr(item));
    }

    static compare(one, two, options) {
      const relativeTo = relativeToArg(optionsOf(options).relativeTo);
      return Number(op('Duration', 'compare', durationStr(one), durationStr(two), relativeTo));
    }

    get sign() {
      const str = strOf(this, 'Duration');
      return this.blank ? 0 : str.startsWith('-') ? -1 : 1;
    }

    get blank() {
      return fieldsOf(this, 'Duration').every((field) => field === '0');
    }

    negated() {
      return wrapDuration(op('Duration', 'negated', strOf(this, 'Duration')));
    }

    abs() {
      return wrapDuration(op('Duration', 'abs', strOf(this, 'Duration')));
    }

    add(other) {
      return wrapDuration(op('Duration', 'add', strOf(this, 'Duration'), durationStr(other)));
    }

    subtract(other) {
      return wrapDuration(op('Duration', 'subtract', strOf(this, 'Duration'), durationStr(other)));
    }

    with(bag) {
      const current = fieldsOf(this, 'Duration');
      const given = readBag(optionsOf(bag), DURATION_UNITS);
      if (Object.keys(given).length === 0) throw new TypeError('Duration.with needs at least one unit');
      const values = DURATION_UNITS.map((unit, i) => durationInt(given[unit] ?? current[i], unit));
      return wrapDuration(op('Duration', 'create', ...values));
    }

    round(roundTo) {
      if (roundTo === undefined) throw new TypeError('roundTo is required');
      const options = typeof roundTo === 'string' ? { smallestUnit: roundTo } : optionsOf(roundTo);
      const args = roundArgs(options);
      return wrapDuration(op('Duration', 'round', strOf(this, 'Duration'), ...args, relativeToArg(options.relativeTo)));
    }

    total(totalOf) {
      if (totalOf === undefined) throw new TypeError('totalOf is required');
      const options = typeof totalOf === 'string' ? { unit: totalOf } : optionsOf(totalOf);
      if (options.unit === undefined) throw new RangeError('unit is required');
      return Number(op('Duration', 'total', strOf(this, 'Duration'), options.unit, relativeToArg(options.relativeTo)));
    }

    toString(options) {
      return op('Duration', 'toString', strOf(this, 'Duration'), ...stringArgs(options));
    }

    toJSON() {
      return this.toString();
    }

    toLocaleString() {
      return this.toString();
    }

    valueOf() {
      throw new TypeError('Temporal.Duration cannot be converted to a primitive; use compare()');
    }
  }
  DURATION_UNITS.forEach((unit, i) => {
    defineProperty(Duration.prototype, unit, { get() { return Number(fieldsOf(this, 'Duration')[i]); }, configurable: true });
  });
  tag(Duration.prototype, 'Temporal.Duration');
  define(Temporal, { Duration });

  defineProperty(global, '__temporal_shared', {
    value: {
      op, slots, define, getters, tag, wrap, slotOf, strOf, fieldsOf, toInt, toIntOr, optionsOf,
      optionText, overflowOf, diffArgs, roundArgs, stringArgs, calendarSuffix, readBag,
      monthFromCode, Temporal, installCommon, durationStr, wrapDuration,
    },
    configurable: true,
  });
})();
