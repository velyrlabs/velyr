'use strict';
// Instant, ZonedDateTime and Now (see prelude.js); prelude_intl.js removes the shared
// helpers from the global.
(() => {
  const T = globalThis.__temporal_shared;
  const {
    op, slots, defineMethods, wrap, slotOf, strOf, fieldsOf, optionsOf, optionText,
    monthFromCode, Temporal, installCommon, calendarId, calendarSuffix, wrapDuration,
  } = T;
  const { defineProperty } = Object;

  const NS_PER_MS = 1000000n;
  const TIME_NAMES = ['hour', 'minute', 'second', 'millisecond', 'microsecond', 'nanosecond'];
  // Spec (alphabetical) read order of a ZonedDateTime property bag.
  const ZONED_READ_ORDER = ['day', 'hour', 'microsecond', 'millisecond', 'minute', 'month', 'monthCode', 'nanosecond', 'offset', 'second', 'timeZone', 'year'];
  const WITH_READ_ORDER = ZONED_READ_ORDER.filter((name) => name !== 'timeZone');
  const notObject = (item) => typeof item !== 'object' || item === null;
  const setLengths = (lengths) => {
    for (const [fn, length] of lengths) defineProperty(fn, 'length', { value: length, configurable: true });
  };

  // Local copies of the plain-type helpers (prelude_plain.js keeps its own private).
  // ToPrimitive with hint string, so that an object whose toString returns a number is rejected.
  const toPrimitiveString = (value) => {
    if (Object(value) !== value) return value;
    const exotic = value[Symbol.toPrimitive];
    if (exotic !== undefined && exotic !== null) return exotic.call(value, 'string');
    for (const method of ['toString', 'valueOf']) {
      const fn = value[method];
      if (typeof fn === 'function') {
        const result = fn.call(value);
        if (Object(result) !== result) return result;
      }
    }
    throw new TypeError('Cannot convert object to a primitive');
  };
  const numericField = (value, name, min) => {
    const n = +value; // unary plus throws for BigInt and Symbol
    if (!Number.isFinite(n)) throw new RangeError(`${name} must be a finite number`);
    const integer = Math.trunc(n) + 0;
    if (min !== undefined && integer < min) throw new RangeError(`${name} is out of range`);
    return integer;
  };
  const stringField = (value, name) => {
    const primitive = toPrimitiveString(value);
    if (typeof primitive !== 'string') throw new TypeError(`${name} must be a string`);
    return primitive;
  };
  const MONTH_CODE_SYNTAX = /^M[0-9]{2}L?$/;
  const OFFSET_SYNTAX = /^[+-]\d\d(?::\d\d(?::\d\d(?:[.,]\d{1,9})?)?|\d\d(?:\d\d(?:[.,]\d{1,9})?)?)?$/;
  // shortcut: year-month and month-day strings are matched by shape only; the other ISO
  // strings go through the native parsers.
  const YEAR_MONTH_OR_MONTH_DAY = /^(?:[0-9]{4}-[0-9]{2}|[0-9]{2}-[0-9]{2})(?:\[u-ca=iso8601\])?$/;
  const parsesAsIso = (text) =>
    YEAR_MONTH_OR_MONTH_DAY.test(text) || ['PlainDateTime', 'PlainTime', 'PlainDate'].some((cls) => {
      try {
        op(cls, 'parse', text);
        return true;
      } catch (error) {
        return false;
      }
    });
  // Temporal objects carry iso8601; the constructor takes calendar ids only (no ISO strings).
  const calendarField = (value, allowText = true) => {
    const slot = slotOf(value);
    if (slot !== undefined && slot.cls !== 'Duration') return;
    if (allowText && typeof value === 'string' && parsesAsIso(value)) return;
    if (calendarId(value) !== 'iso8601') throw new RangeError('Only the iso8601 calendar is supported');
  };
  // A time zone identifier, offset or ISO string with a time zone, in canonical form.
  const zoneOf = (value) => {
    if (typeof value === 'string') return op('TimeZone', 'id', value);
    const slot = slotOf(value);
    if (slot !== undefined && slot.cls === 'ZonedDateTime') return fieldsOf(value, 'ZonedDateTime')[21];
    throw new TypeError('Invalid time zone');
  };
  const MINIMUM = { day: 1, month: 1 };
  // One Get and conversion per name, in the order given.
  const readFields = (bag, names) => {
    const out = {};
    for (const name of names) {
      const value = bag[name];
      if (value === undefined) continue;
      if (name === 'monthCode') {
        out[name] = stringField(value, name);
        if (!MONTH_CODE_SYNTAX.test(out[name])) throw new RangeError(`Invalid month code ${out[name]}`);
      } else if (name === 'offset') {
        out[name] = stringField(value, name);
        if (!OFFSET_SYNTAX.test(out[name])) throw new RangeError(`Invalid offset ${out[name]}`);
      } else if (name === 'timeZone') {
        out[name] = zoneOf(value);
      } else {
        out[name] = numericField(value, name, MINIMUM[name] ?? (TIME_NAMES.includes(name) ? 0 : undefined));
      }
    }
    return out;
  };
  // The ISO local date-time of `given`, filling gaps from the `fields` csv `base`.
  const localDateTime = (given, base, overflow) => {
    const parts = [given.year, given.month, given.day];
    if (base) parts.forEach((part, i) => { if (part === undefined) parts[i] = Number(base[i]); });
    if (parts[0] === undefined || parts[2] === undefined || (parts[1] === undefined && given.monthCode === undefined)) {
      throw new TypeError('year, month and day are required');
    }
    if (given.monthCode !== undefined) {
      const fromCode = monthFromCode(given.monthCode);
      if (fromCode > 12) throw new RangeError(`monthCode ${given.monthCode} is not valid for iso8601`);
      if (given.month !== undefined && given.month !== fromCode) throw new RangeError('month and monthCode disagree');
      parts[1] = fromCode;
    }
    const time = TIME_NAMES.map((name, i) => given[name] ?? (base ? Number(base[12 + i]) : 0));
    return op('PlainDateTime', 'create', ...parts, ...time, overflow);
  };

  const enumOption = (o, name, allowed) => {
    const value = optionText(o, name);
    if (value !== undefined && !allowed.includes(value)) throw new RangeError(`${value} is not a valid value for ${name}`);
    return value;
  };
  // disambiguation, offset and overflow, read and validated in spec order.
  const readChoices = (options) => {
    const o = optionsOf(options);
    return {
      disambiguation: enumOption(o, 'disambiguation', ['compatible', 'earlier', 'later', 'reject']),
      offset: enumOption(o, 'offset', ['use', 'prefer', 'ignore', 'reject']),
      overflow: enumOption(o, 'overflow', ['constrain', 'reject']),
    };
  };
  const digitsOf = (o) => {
    const value = o.fractionalSecondDigits;
    if (value === undefined) return undefined;
    if (typeof value !== 'number') {
      if (`${value}` !== 'auto') throw new RangeError('fractionalSecondDigits must be auto or 0-9');
      return 'auto';
    }
    const digits = Math.floor(value);
    if (!Number.isFinite(value) || digits < 0 || digits > 9) throw new RangeError('fractionalSecondDigits must be 0-9');
    return digits;
  };
  // GetRoundingIncrementOption: ToNumber, then 1..1e9 after truncation.
  const incrementOf = (o) => {
    const value = o.roundingIncrement;
    if (value === undefined) return undefined;
    const number = +value;
    const increment = Math.trunc(number);
    if (!Number.isFinite(number) || increment < 1 || increment > 1e9) throw new RangeError('invalid roundingIncrement');
    return String(increment);
  };
  // [largestUnit, smallestUnit, roundingMode, roundingIncrement] for the native op, in spec order.
  const unitArgs = (o, withLargest) => {
    const largest = withLargest ? optionText(o, 'largestUnit') : undefined;
    const increment = incrementOf(o);
    const mode = optionText(o, 'roundingMode');
    return [largest, optionText(o, 'smallestUnit'), mode, increment];
  };
  const roundArgs = (roundTo) => {
    if (roundTo === undefined) throw new TypeError('roundTo is required');
    const options = typeof roundTo === 'string' ? { __proto__: null, smallestUnit: roundTo } : optionsOf(roundTo);
    const args = unitArgs(options, false);
    if (args[1] === undefined) throw new RangeError('smallestUnit is required');
    return args;
  };
  const differenceMethods = (Class, cls) => {
    const difference = (self, method, other, options) => {
      const from = strOf(self, cls);
      return wrapDuration(op(cls, method, from, strOf(Class.from(other), cls), ...unitArgs(optionsOf(options), true)));
    };
    defineMethods(Class.prototype, {
      until(other, options) { return difference(this, 'until', other, options); },
      since(other, options) { return difference(this, 'since', other, options); },
    }, { until: 1, since: 1 });
  };
  const DURATION_UNITS = ['years', 'months', 'weeks', 'days', 'hours', 'minutes', 'seconds', 'milliseconds', 'microseconds', 'nanoseconds'];
  const DURATION_READ_ORDER = [...DURATION_UNITS].sort();
  // ToTemporalDuration with the properties read alphabetically and integral values required.
  const durationArg = (item) => {
    if (typeof item === 'string') return op('Duration', 'parse', item);
    const slot = slotOf(item);
    if (slot !== undefined && slot.cls === 'Duration') return slot.str;
    if (notObject(item)) throw new TypeError('Invalid duration');
    const given = {};
    for (const unit of DURATION_READ_ORDER) {
      const value = item[unit];
      if (value === undefined) continue;
      const number = +value;
      if (!Number.isInteger(number)) throw new RangeError(`${unit} must be an integer`);
      given[unit] = BigInt(number).toString();
    }
    if (Object.keys(given).length === 0) throw new TypeError('Duration needs at least one unit');
    return op('Duration', 'create', ...DURATION_UNITS.map((unit) => given[unit] ?? '0'));
  };
  // add and subtract; only the ZonedDateTime ones read `overflow`.
  const arithmeticMethods = (Class, cls, wrapper) => {
    const overflowOption = (options) => (cls === 'Instant' ? undefined : enumOption(optionsOf(options), 'overflow', ['constrain', 'reject']));
    const arithmetic = (self, method, duration, options) => {
      const str = strOf(self, cls);
      const amount = durationArg(duration);
      return wrapper(op(cls, method, str, amount, overflowOption(options)));
    };
    defineMethods(Class.prototype, {
      add(duration, options) { return arithmetic(this, 'add', duration, options); },
      subtract(duration, options) { return arithmetic(this, 'subtract', duration, options); },
    }, { add: 1, subtract: 1 });
  };
  // toJSON and toLocaleString brand-check, then use the intrinsic toString.
  const serializers = (Class, cls) => {
    const intrinsic = Class.prototype.toString;
    defineMethods(Class.prototype, {
      toJSON() { strOf(this, cls); return intrinsic.call(this); },
      toLocaleString() { strOf(this, cls); return intrinsic.call(this); },
    }, { toJSON: 0, toLocaleString: 0 });
  };
  // ToBigInt: booleans and strings convert, numbers and the rest are a TypeError.
  const toBigInt = (value) => {
    const primitive = toPrimitiveString(value);
    if (typeof primitive === 'boolean' || typeof primitive === 'string') return BigInt(primitive);
    if (typeof primitive !== 'bigint') throw new TypeError('epoch nanoseconds must be a BigInt');
    return primitive;
  };

  // ---- Instant ----
  const wrapInstant = (str) => wrap(Instant.prototype, 'Instant', str);
  const instantText = (item) => {
    const slot = slotOf(item);
    if (slot !== undefined && slot.cls === 'Instant') return slot.str;
    if (slot !== undefined && slot.cls === 'ZonedDateTime') return op('ZonedDateTime', 'toInstant', slot.str);
    return op('Instant', 'parse', stringField(item, 'Instant'));
  };

  class Instant {
    constructor(epochNanoseconds) {
      slots.set(this, { cls: 'Instant', str: op('Instant', 'create', toBigInt(epochNanoseconds)) });
    }

    static from(item) {
      return wrapInstant(instantText(item));
    }

    static fromEpochMilliseconds(epochMilliseconds) {
      const ms = +epochMilliseconds; // throws for BigInt and Symbol
      if (!Number.isInteger(ms)) throw new RangeError('epochMilliseconds must be an integer');
      return wrapInstant(op('Instant', 'create', BigInt(ms) * NS_PER_MS));
    }

    static fromEpochNanoseconds(epochNanoseconds) {
      return new Instant(epochNanoseconds);
    }

    get epochMilliseconds() {
      return Number(op('Instant', 'epochMilliseconds', strOf(this, 'Instant')));
    }

    get epochNanoseconds() {
      return BigInt(op('Instant', 'epochNanoseconds', strOf(this, 'Instant')));
    }

    round(roundTo) {
      return wrapInstant(op('Instant', 'round', strOf(this, 'Instant'), ...roundArgs(roundTo)));
    }

    toZonedDateTimeISO(timeZone) {
      const str = op('Instant', 'toZoned', strOf(this, 'Instant'), zoneOf(timeZone));
      return wrap(ZonedDateTime.prototype, 'ZonedDateTime', str);
    }

    toString(options) {
      const str = strOf(this, 'Instant');
      const o = optionsOf(options);
      const digits = digitsOf(o);
      const mode = optionText(o, 'roundingMode');
      const unit = optionText(o, 'smallestUnit');
      const zone = o.timeZone;
      const timeZone = zone === undefined ? undefined : zoneOf(zone);
      return op('Instant', 'toString', str, timeZone, digits, unit, mode);
    }
  }
  installCommon(Instant, 'Instant', wrapInstant);
  differenceMethods(Instant, 'Instant');
  arithmeticMethods(Instant, 'Instant', wrapInstant);
  serializers(Instant, 'Instant');
  setLengths([[Instant.prototype.toString, 0]]);

  // ---- ZonedDateTime ----
  const wrapZoned = (str) => wrap(ZonedDateTime.prototype, 'ZonedDateTime', str);
  const zonedFrom = (local, offset, zone, disambiguation, offsetOption) =>
    wrapZoned(op('ZonedDateTime', 'parse', `${local}${offset ?? ''}[${zone}]`, disambiguation, offsetOption));
  // A string is parsed before the options are read; the options are read before the rest of
  // the validation of a bag (options-read-before-algorithmic-validation).
  const zonedFromText = (text, options) => {
    if (options === undefined) return wrapZoned(op('ZonedDateTime', 'parse', text));
    op('ZonedDateTime', 'parse', text, undefined, 'ignore');
    const choices = readChoices(options);
    return wrapZoned(op('ZonedDateTime', 'parse', text, choices.disambiguation, choices.offset));
  };

  class ZonedDateTime {
    constructor(epochNanoseconds, timeZone, calendar = undefined) {
      const nanoseconds = toBigInt(epochNanoseconds);
      if (typeof timeZone !== 'string') throw new TypeError('timeZone must be a string');
      const zone = op('ZonedDateTime', 'zoneId', timeZone);
      if (calendar !== undefined) calendarField(calendar, false);
      slots.set(this, { cls: 'ZonedDateTime', str: op('ZonedDateTime', 'create', nanoseconds, zone) });
    }

    static from(item, options) {
      if (typeof item === 'string') return zonedFromText(item, options);
      if (notObject(item)) throw new TypeError('Invalid ZonedDateTime');
      const slot = slotOf(item);
      if (slot !== undefined) {
        if (slot.cls !== 'ZonedDateTime') throw new TypeError('Invalid ZonedDateTime');
        readChoices(options);
        return wrapZoned(slot.str);
      }
      const calendar = item.calendar;
      if (calendar !== undefined) calendarField(calendar);
      const given = readFields(item, ZONED_READ_ORDER);
      if (given.timeZone === undefined) throw new TypeError('timeZone is required');
      const choices = readChoices(options);
      const local = localDateTime(given, undefined, choices.overflow);
      return zonedFrom(local, given.offset, given.timeZone, choices.disambiguation, choices.offset);
    }

    get timeZoneId() { return fieldsOf(this, 'ZonedDateTime')[21]; }
    get offset() { return fieldsOf(this, 'ZonedDateTime')[18]; }
    get offsetNanoseconds() { return Number(fieldsOf(this, 'ZonedDateTime')[19]); }
    get hoursInDay() { return Number(fieldsOf(this, 'ZonedDateTime')[22]); }
    get epochNanoseconds() { return BigInt(fieldsOf(this, 'ZonedDateTime')[20]); }
    get epochMilliseconds() { return Number(this.epochNanoseconds / NS_PER_MS - (this.epochNanoseconds % NS_PER_MS < 0n ? 1n : 0n)); }

    with(bag, options) {
      const current = fieldsOf(this, 'ZonedDateTime');
      if (notObject(bag) || slotOf(bag) !== undefined) throw new TypeError('with() needs a plain object');
      if (bag.calendar !== undefined || bag.timeZone !== undefined) throw new TypeError('calendar and timeZone cannot be changed here');
      const given = readFields(bag, WITH_READ_ORDER);
      if (Object.keys(given).length === 0) throw new TypeError('with() needs at least one recognized property');
      const choices = readChoices(options);
      const local = localDateTime(given, current, choices.overflow);
      return zonedFrom(local, given.offset ?? current[18], current[21], choices.disambiguation, choices.offset ?? 'prefer');
    }

    withPlainTime(plainTime) {
      const str = strOf(this, 'ZonedDateTime');
      const time = plainTime === undefined ? undefined : Temporal.PlainTime.from(plainTime).toString();
      return wrapZoned(op('ZonedDateTime', 'withPlainTime', str, time));
    }

    withCalendar(calendar) {
      const str = strOf(this, 'ZonedDateTime');
      calendarField(calendar);
      return wrapZoned(str);
    }

    withTimeZone(timeZone) {
      return wrapZoned(op('ZonedDateTime', 'withTimeZone', strOf(this, 'ZonedDateTime'), zoneOf(timeZone)));
    }

    getTimeZoneTransition(directionParam) {
      const str = strOf(this, 'ZonedDateTime');
      if (directionParam === undefined) throw new TypeError('direction is required');
      const direction = typeof directionParam === 'string' ? directionParam : `${optionsOf(directionParam).direction}`;
      const found = op('ZonedDateTime', 'transition', str, direction);
      return found === '' ? null : wrapZoned(found);
    }

    startOfDay() {
      return wrapZoned(op('ZonedDateTime', 'startOfDay', strOf(this, 'ZonedDateTime')));
    }

    round(roundTo) {
      return wrapZoned(op('ZonedDateTime', 'round', strOf(this, 'ZonedDateTime'), ...roundArgs(roundTo)));
    }

    toInstant() {
      return wrapInstant(op('ZonedDateTime', 'toInstant', strOf(this, 'ZonedDateTime')));
    }

    toPlainDate() { strOf(this, 'ZonedDateTime'); return Temporal.PlainDate.from(this); }
    toPlainTime() { strOf(this, 'ZonedDateTime'); return Temporal.PlainTime.from(this); }
    toPlainDateTime() { strOf(this, 'ZonedDateTime'); return Temporal.PlainDateTime.from(this); }

    toString(options) {
      const str = strOf(this, 'ZonedDateTime');
      const o = optionsOf(options);
      const calendarName = enumOption(o, 'calendarName', ['auto', 'always', 'never', 'critical']);
      const digits = digitsOf(o);
      const offset = enumOption(o, 'offset', ['auto', 'never']);
      const mode = optionText(o, 'roundingMode');
      const unit = optionText(o, 'smallestUnit');
      const zoneName = enumOption(o, 'timeZoneName', ['auto', 'never', 'critical']);
      const text = op('ZonedDateTime', 'toString', str, offset, zoneName, 'auto', digits, unit, mode);
      return calendarSuffix(text, { calendarName });
    }
  }
  T.installDateGetters(ZonedDateTime.prototype, 'ZonedDateTime');
  T.installTimeGetters(ZonedDateTime.prototype, 'ZonedDateTime', 12);
  installCommon(ZonedDateTime, 'ZonedDateTime', wrapZoned, true);
  differenceMethods(ZonedDateTime, 'ZonedDateTime');
  arithmeticMethods(ZonedDateTime, 'ZonedDateTime', wrapZoned);
  serializers(ZonedDateTime, 'ZonedDateTime');
  setLengths([
    [ZonedDateTime.from, 1], [ZonedDateTime.prototype.with, 1], [ZonedDateTime.prototype.withPlainTime, 0],
    [ZonedDateTime.prototype.toString, 0],
  ]);

  // ---- Now ----
  const Now = {};
  T.tag(Now, 'Temporal.Now');
  const nowInstant = () => wrapInstant(op('Instant', 'create', op('Now', 'epochNanoseconds')));
  const zonedNow = (timeZone) =>
    nowInstant().toZonedDateTimeISO(timeZone === undefined ? op('Now', 'timeZoneId') : timeZone);
  defineMethods(Now, {
    instant() { return nowInstant(); },
    timeZoneId() { return op('Now', 'timeZoneId'); },
    zonedDateTimeISO(timeZone) { return zonedNow(timeZone); },
    plainDateTimeISO(timeZone) { return zonedNow(timeZone).toPlainDateTime(); },
    plainDateISO(timeZone) { return zonedNow(timeZone).toPlainDate(); },
    plainTimeISO(timeZone) { return zonedNow(timeZone).toPlainTime(); },
  }, { instant: 0, timeZoneId: 0, zonedDateTimeISO: 0, plainDateTimeISO: 0, plainDateISO: 0, plainTimeISO: 0 });

  T.define(Temporal, { Instant, ZonedDateTime, Now });
})();
