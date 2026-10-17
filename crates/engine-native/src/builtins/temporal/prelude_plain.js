'use strict';
// PlainDate, PlainTime and PlainDateTime (see prelude.js).
(() => {
  const T = globalThis.__temporal_shared;
  const {
    op, slots, define, getters, wrap, slotOf, strOf, fieldsOf, optionsOf, optionText,
    overflowOf, monthFromCode, Temporal, installCommon, calendarId, defineMethods, durationStr,
    wrapDuration, withCalendarName, slotCalendar,
  } = T;
  const { defineProperty } = Object;

  const DATE_NAMES = ['day', 'month', 'monthCode', 'year'];
  // Native op order; `TIME_READ_ORDER` is the alphabetical order the spec reads bags in.
  const TIME_NAMES = ['hour', 'minute', 'second', 'millisecond', 'microsecond', 'nanosecond'];
  const TIME_READ_ORDER = ['hour', 'microsecond', 'millisecond', 'minute', 'nanosecond', 'second'];
  const DATE_TIME_READ_ORDER = [
    'day', 'hour', 'microsecond', 'millisecond', 'minute', 'month', 'monthCode', 'nanosecond', 'second', 'year',
  ];
  const DATE_GETTERS = {
    year: 0, month: 1, day: 2, dayOfWeek: 3, dayOfYear: 4, weekOfYear: 5, yearOfWeek: 6,
    daysInMonth: 7, daysInYear: 8, monthsInYear: 9,
  };
  const DAYS_IN_WEEK = 7;

  // Getters over the `fields` csv: 12 date fields, then 6 time fields.
  T.installDateGetters = (proto, cls) => {
    const table = {
      calendarId() { fieldsOf(this, cls); return slotCalendar(slotOf(this).str); },
      era() { fieldsOf(this, cls); return undefined; },
      eraYear() { fieldsOf(this, cls); return undefined; },
    };
    for (const [name, index] of Object.entries(DATE_GETTERS)) {
      table[name] = function () { return Number(fieldsOf(this, cls)[index]); };
    }
    table.daysInWeek = function () { fieldsOf(this, cls); return DAYS_IN_WEEK; };
    table.inLeapYear = function () { return fieldsOf(this, cls)[10] === '1'; };
    table.monthCode = function () { return fieldsOf(this, cls)[11]; };
    getters(proto, table);
  };
  T.installTimeGetters = (proto, cls, base) => {
    const table = {};
    TIME_NAMES.forEach((name, i) => {
      table[name] = function () { return Number(fieldsOf(this, cls)[base + i]); };
    });
    getters(proto, table);
  };
  T.timeZoneIdOf = (value) => {
    if (typeof value === 'string') return value;
    const slot = slotOf(value);
    if (slot !== undefined && slot.cls === 'ZonedDateTime') return fieldsOf(value, 'ZonedDateTime')[21];
    throw new TypeError('Invalid time zone');
  };

  // PrepareCalendarFields: one Get and conversion per field, in the order given (alphabetical).
  const numericField = (value, name, min) => {
    const n = +value; // unary plus throws for BigInt and Symbol
    if (!Number.isFinite(n)) throw new RangeError(`${name} must be a finite number`);
    const integer = Math.trunc(n) + 0;
    if (min !== undefined && integer < min) throw new RangeError(`${name} is out of range`);
    return integer;
  };
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
  const MONTH_CODE_SYNTAX = /^M[0-9]{2}L?$/;
  const textField = (value, name) => {
    const primitive = toPrimitiveString(value);
    if (typeof primitive !== 'string') throw new TypeError(`${name} must be a string`);
    if (name === 'monthCode' && !MONTH_CODE_SYNTAX.test(primitive)) throw new RangeError(`Invalid month code ${primitive}`);
    return primitive;
  };
  // The constructors take calendar ids only; everywhere else an ISO string is accepted too.
  const calendarOf = (value, allowText = true) => calendarId(value, allowText);
  // shortcut: bag fields are read as ISO fields, so a non-iso `calendar` is refused until N46.
  const bagCalendar = (value) => {
    if (calendarOf(value) !== 'iso8601') throw new RangeError('Property bags with a non-iso8601 calendar are not supported yet');
  };
  const MINIMUM = { day: 1, month: 1 };
  const readFields = (bag, names) => {
    const out = {};
    for (const name of names) {
      const value = bag[name];
      if (value === undefined) continue;
      if (name === 'calendar') bagCalendar(value);
      else out[name] = name === 'monthCode' ? textField(value, name) : numericField(value, name, MINIMUM[name] ?? (TIME_NAMES.includes(name) ? 0 : undefined));
    }
    return out;
  };
  // [year, month, day] from converted fields, filling gaps from the `base` fields.
  const dateParts = (given, base) => {
    const parts = [given.year, given.month, given.day];
    if (base) parts.forEach((part, i) => { if (part === undefined) parts[i] = Number(base[i]); });
    if (parts[0] === undefined || parts[2] === undefined || (parts[1] === undefined && given.monthCode === undefined)) {
      throw new TypeError('year, month and day are required');
    }
    return parts;
  };
  const resolveMonth = (given, parts) => {
    if (given.monthCode === undefined) return parts;
    const fromCode = monthFromCode(given.monthCode);
    if (fromCode > 12) throw new RangeError(`monthCode ${given.monthCode} is not valid for iso8601`);
    if (given.month !== undefined && given.month !== fromCode) throw new RangeError('month and monthCode disagree');
    parts[1] = fromCode;
    return parts;
  };
  const timeParts = (given, base) =>
    TIME_NAMES.map((name, i) => given[name] ?? (base ? Number(base[i]) : 0));
  const hasAny = (bag, names) => names.some((name) => bag[name] !== undefined);
  const needSome = (bag, names, what) => {
    if (!hasAny(bag, names)) throw new TypeError(`${what} needs at least one recognized property`);
  };
  const noZoneOrCalendar = (bag) => {
    if (bag.calendar !== undefined || bag.timeZone !== undefined) {
      throw new TypeError('calendar and timeZone cannot be changed here');
    }
  };
  const fromSlot = (item, accepted) => {
    const slot = slotOf(item);
    if (slot === undefined || !accepted.includes(slot.cls)) throw new TypeError('Invalid Temporal object');
    return slot;
  };
  const notObject = (item) => typeof item !== 'object' || item === null;
  const withObject = (bag) => {
    if (notObject(bag)) throw new TypeError('with() needs an object');
    noZoneOrCalendar(bag);
  };
  const CALENDAR_NAMES = ['auto', 'always', 'never', 'critical'];
  const calendarNameOf = (options) => {
    const name = optionText(optionsOf(options), 'calendarName');
    if (name !== undefined && !CALENDAR_NAMES.includes(name)) throw new RangeError(`${name} is not a valid calendarName`);
    return name;
  };
  // `str` with `time` between its date-time and its annotations.
  const withTime = (str, time) => str.replace(/^[^[T]*(?:T[^[]*)?/, (head) => `${head.split('T')[0]}T${time}`);
  const digitsOf = (value) => {
    if (typeof value !== 'number') {
      if (`${value}` !== 'auto') throw new RangeError('fractionalSecondDigits must be auto or 0-9');
      return 'auto';
    }
    const digits = Math.floor(value);
    if (!Number.isFinite(value) || digits < 0 || digits > 9) throw new RangeError('fractionalSecondDigits must be 0-9');
    return digits;
  };
  // fractionalSecondDigits, smallestUnit, roundingMode for the native op, read in spec order.
  const timeStringArgs = (options) => {
    const o = optionsOf(options);
    const rawDigits = o.fractionalSecondDigits;
    const digits = rawDigits === undefined ? undefined : digitsOf(rawDigits);
    const mode = optionText(o, 'roundingMode');
    return [digits, optionText(o, 'smallestUnit'), mode];
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
  // [largestUnit, smallestUnit, roundingMode, roundingIncrement] for the native op, read in spec order.
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
  const DISAMBIGUATIONS = ['compatible', 'earlier', 'later', 'reject'];
  const disambiguationOf = (options) => {
    const value = optionText(optionsOf(options), 'disambiguation');
    if (value !== undefined && !DISAMBIGUATIONS.includes(value)) throw new RangeError(`${value} is not a valid disambiguation`);
    return value;
  };
  // until/since with the increment coerced as a Number, replacing the shared string read.
  const difference = (self, Class, cls, method, other, options) => {
    const from = strOf(self, cls);
    return wrapDuration(op(cls, method, from, strOf(Class.from(other), cls), ...unitArgs(optionsOf(options), true)));
  };
  const differenceMethods = (Class, cls) => {
    defineMethods(Class.prototype, {
      until(other, options) {
        return difference(this, Class, cls, 'until', other, options);
      },
      since(other, options) {
        return difference(this, Class, cls, 'since', other, options);
      },
    }, { until: 1, since: 1 });
  };
  const setLengths = (lengths) => {
    for (const [fn, length] of lengths) defineProperty(fn, 'length', { value: length, configurable: true });
  };

  // ---- PlainDate ----
  const wrapDate = (str) => wrap(PlainDate.prototype, 'PlainDate', str);
  const timeText = (time) => (time === undefined ? '00:00:00' : strOf(Temporal.PlainTime.from(time), 'PlainTime'));
  const dateFromSlot = (item) =>
    op('PlainDate', 'parse', fromSlot(item, ['PlainDate', 'PlainDateTime', 'ZonedDateTime']).str);
  const createDate = (given, overflow, base) =>
    op('PlainDate', 'create', ...resolveMonth(given, dateParts(given, base)), overflow);

  class PlainDate {
    constructor(year, month, day, calendar = 'iso8601') {
      const parts = [numericField(year, 'year'), numericField(month, 'month', 1), numericField(day, 'day', 1)];
      const id = calendarOf(calendar, false);
      slots.set(this, { cls: 'PlainDate', str: op('PlainDate', 'create', ...parts, 'reject', id) });
    }

    static from(item, options) {
      if (typeof item === 'string') {
        const str = op('PlainDate', 'parse', item);
        overflowOf(options);
        return wrapDate(str);
      }
      if (notObject(item)) throw new TypeError('Invalid PlainDate');
      if (slotOf(item) !== undefined) {
        const str = dateFromSlot(item);
        overflowOf(options);
        return wrapDate(str);
      }
      const given = readFields(item, ['calendar', ...DATE_NAMES]);
      dateParts(given);
      return wrapDate(createDate(given, overflowOf(options)));
    }

    with(bag, options) {
      const base = fieldsOf(this, 'PlainDate');
      withObject(bag);
      const given = readFields(bag, DATE_NAMES);
      needSome(given, DATE_NAMES, 'with()');
      return wrapDate(createDate(given, overflowOf(options), base));
    }

    withCalendar(calendar) {
      return wrapDate(op('PlainDate', 'withCalendar', strOf(this, 'PlainDate'), calendarOf(calendar)));
    }

    toPlainDateTime(time) {
      return Temporal.PlainDateTime.from(withTime(strOf(this, 'PlainDate'), timeText(time)));
    }

    toPlainYearMonth() {
      strOf(this, 'PlainDate');
      return Temporal.PlainYearMonth.from(this);
    }

    toPlainMonthDay() {
      strOf(this, 'PlainDate');
      return Temporal.PlainMonthDay.from(this);
    }

    toZonedDateTime(item) {
      const bag = typeof item === 'object' && item !== null && slotOf(item) === undefined ? item : { timeZone: item };
      const time = bag.plainTime === undefined ? undefined : strOf(Temporal.PlainTime.from(bag.plainTime), 'PlainTime');
      const str = op('PlainDate', 'toZoned', strOf(this, 'PlainDate'), T.timeZoneIdOf(bag.timeZone), time);
      return wrap(Temporal.ZonedDateTime.prototype, 'ZonedDateTime', str);
    }

    toString(options) {
      const str = strOf(this, 'PlainDate');
      return withCalendarName(str, calendarNameOf(options));
    }
  }
  T.installDateGetters(PlainDate.prototype, 'PlainDate');
  installCommon(PlainDate, 'PlainDate', wrapDate);
  differenceMethods(PlainDate, 'PlainDate');
  setLengths([
    [PlainDate.from, 1], [PlainDate.prototype.with, 1], [PlainDate.prototype.withCalendar, 1],
    [PlainDate.prototype.toPlainDateTime, 0], [PlainDate.prototype.toZonedDateTime, 1],
    [PlainDate.prototype.toString, 0],
  ]);

  // ---- PlainTime ----
  const wrapTime = (str) => wrap(PlainTime.prototype, 'PlainTime', str);

  class PlainTime {
    constructor(hour = 0, minute = 0, second = 0, millisecond = 0, microsecond = 0, nanosecond = 0) {
      const values = [hour, minute, second, millisecond, microsecond, nanosecond];
      const parts = values.map((value, i) => numericField(value, TIME_NAMES[i], 0));
      slots.set(this, { cls: 'PlainTime', str: op('PlainTime', 'create', ...parts, 'reject') });
    }

    static from(item, options) {
      if (typeof item === 'string') {
        const str = op('PlainTime', 'parse', item);
        overflowOf(options);
        return wrapTime(str);
      }
      if (notObject(item)) throw new TypeError('Invalid PlainTime');
      if (slotOf(item) !== undefined) {
        const str = op('PlainTime', 'parse', fromSlot(item, ['PlainTime', 'PlainDateTime', 'ZonedDateTime']).str);
        overflowOf(options);
        return wrapTime(str);
      }
      const bag = readFields(item, TIME_READ_ORDER);
      needSome(bag, TIME_NAMES, 'PlainTime.from()');
      return wrapTime(op('PlainTime', 'create', ...timeParts(bag), overflowOf(options)));
    }

    with(bag, options) {
      const base = fieldsOf(this, 'PlainTime');
      withObject(bag);
      const given = readFields(bag, TIME_READ_ORDER);
      needSome(given, TIME_NAMES, 'with()');
      return wrapTime(op('PlainTime', 'create', ...timeParts(given, base), overflowOf(options)));
    }

    round(roundTo) {
      const str = strOf(this, 'PlainTime');
      return wrapTime(op('PlainTime', 'round', str, ...roundArgs(roundTo)));
    }

    toString(options) {
      return op('PlainTime', 'toString', strOf(this, 'PlainTime'), ...timeStringArgs(options));
    }
  }
  T.installTimeGetters(PlainTime.prototype, 'PlainTime', 0);
  installCommon(PlainTime, 'PlainTime', wrapTime);
  differenceMethods(PlainTime, 'PlainTime');
  defineMethods(PlainTime.prototype, {
    add(duration) {
      return wrapTime(op('PlainTime', 'add', strOf(this, 'PlainTime'), durationStr(duration)));
    },
    subtract(duration) {
      return wrapTime(op('PlainTime', 'subtract', strOf(this, 'PlainTime'), durationStr(duration)));
    },
  }, { add: 1, subtract: 1 });
  setLengths([
    [PlainTime.from, 1], [PlainTime.prototype.with, 1], [PlainTime.prototype.round, 1],
    [PlainTime.prototype.toString, 0],
  ]);

  // ---- PlainDateTime ----
  const wrapDateTime = (str) => wrap(PlainDateTime.prototype, 'PlainDateTime', str);
  const dateTimeFromSlot = (item) => {
    const slot = fromSlot(item, ['PlainDate', 'PlainDateTime', 'ZonedDateTime']);
    return op('PlainDateTime', 'parse', slot.cls === 'PlainDate' ? withTime(slot.str, '00:00:00') : slot.str);
  };

  class PlainDateTime {
    constructor(year, month, day, hour = 0, minute = 0, second = 0, millisecond = 0, microsecond = 0, nanosecond = 0, calendar = 'iso8601') {
      const date = [numericField(year, 'year'), numericField(month, 'month', 1), numericField(day, 'day', 1)];
      const values = [hour, minute, second, millisecond, microsecond, nanosecond];
      const time = values.map((value, i) => numericField(value, TIME_NAMES[i], 0));
      const id = calendarOf(calendar, false);
      slots.set(this, { cls: 'PlainDateTime', str: op('PlainDateTime', 'create', ...date, ...time, 'reject', id) });
    }

    static from(item, options) {
      if (typeof item === 'string') {
        const str = op('PlainDateTime', 'parse', item);
        overflowOf(options);
        return wrapDateTime(str);
      }
      if (notObject(item)) throw new TypeError('Invalid PlainDateTime');
      if (slotOf(item) !== undefined) {
        const str = dateTimeFromSlot(item);
        overflowOf(options);
        return wrapDateTime(str);
      }
      const given = readFields(item, ['calendar', ...DATE_TIME_READ_ORDER]);
      const date = dateParts(given);
      const overflow = overflowOf(options);
      return wrapDateTime(op('PlainDateTime', 'create', ...resolveMonth(given, date), ...timeParts(given), overflow));
    }

    with(bag, options) {
      const base = fieldsOf(this, 'PlainDateTime');
      withObject(bag);
      const given = readFields(bag, DATE_TIME_READ_ORDER);
      needSome(given, [...DATE_NAMES, ...TIME_NAMES], 'with()');
      const overflow = overflowOf(options);
      const date = resolveMonth(given, dateParts(given, base));
      return wrapDateTime(op('PlainDateTime', 'create', ...date, ...timeParts(given, base.slice(12)), overflow));
    }

    withPlainTime(plainTime) {
      return wrapDateTime(op('PlainDateTime', 'parse', withTime(strOf(this, 'PlainDateTime'), timeText(plainTime))));
    }

    withCalendar(calendar) {
      return wrapDateTime(op('PlainDateTime', 'withCalendar', strOf(this, 'PlainDateTime'), calendarOf(calendar)));
    }

    round(roundTo) {
      const str = strOf(this, 'PlainDateTime');
      return wrapDateTime(op('PlainDateTime', 'round', str, ...roundArgs(roundTo)));
    }

    toPlainDate() {
      strOf(this, 'PlainDateTime');
      return PlainDate.from(this);
    }

    toPlainTime() {
      strOf(this, 'PlainDateTime');
      return PlainTime.from(this);
    }

    toPlainYearMonth() {
      strOf(this, 'PlainDateTime');
      return Temporal.PlainYearMonth.from(this);
    }

    toPlainMonthDay() {
      strOf(this, 'PlainDateTime');
      return Temporal.PlainMonthDay.from(this);
    }

    toZonedDateTime(timeZone, options) {
      const self = strOf(this, 'PlainDateTime');
      const zone = T.timeZoneIdOf(timeZone);
      const str = op('PlainDateTime', 'toZoned', self, zone, disambiguationOf(options));
      return wrap(Temporal.ZonedDateTime.prototype, 'ZonedDateTime', str);
    }

    toString(options) {
      const str = strOf(this, 'PlainDateTime');
      const name = calendarNameOf(options);
      return withCalendarName(op('PlainDateTime', 'toString', str, ...timeStringArgs(options)), name);
    }
  }
  T.installDateGetters(PlainDateTime.prototype, 'PlainDateTime');
  T.installTimeGetters(PlainDateTime.prototype, 'PlainDateTime', 12);
  installCommon(PlainDateTime, 'PlainDateTime', wrapDateTime);
  differenceMethods(PlainDateTime, 'PlainDateTime');
  setLengths([
    [PlainDateTime.from, 1], [PlainDateTime.prototype.with, 1], [PlainDateTime.prototype.withPlainTime, 0],
    [PlainDateTime.prototype.withCalendar, 1], [PlainDateTime.prototype.round, 1],
    [PlainDateTime.prototype.toZonedDateTime, 1], [PlainDateTime.prototype.toString, 0],
  ]);

  for (const Class of [PlainDate, PlainTime, PlainDateTime]) {
    defineMethods(Class.prototype, {
      toJSON() {
        return Class.prototype.toString.call(this);
      },
      toLocaleString() {
        return Class.prototype.toString.call(this);
      },
    }, { toJSON: 0, toLocaleString: 0 });
  }

  define(Temporal, { PlainDate, PlainTime, PlainDateTime });
})();
