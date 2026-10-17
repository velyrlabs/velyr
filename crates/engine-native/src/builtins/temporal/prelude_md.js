'use strict';
// Temporal.PlainMonthDay, iso8601 only (see prelude.js). The slot string is the native
// `isoMonth,isoDay,referenceYear` state.
(() => {
  const T = globalThis.__temporal_shared;
  const {
    op, slots, getters, wrap, slotOf, strOf, fieldsOf, optionsOf, optionText,
    overflowOf, Temporal, calendarId, define, defineMethods, tag,
  } = T;
  const { defineProperty } = Object;

  const wrapMonthDay = (str) => wrap(PlainMonthDay.prototype, 'PlainMonthDay', str);
  const notObject = (item) => typeof item !== 'object' || item === null;

  // PrepareCalendarFields value conversions.
  const integerField = (value, name, min) => {
    const n = +value; // unary plus throws for BigInt and Symbol
    if (!Number.isFinite(n)) throw new RangeError(`${name} must be a finite number`);
    const integer = Math.trunc(n) + 0;
    if (min !== undefined && integer < min) throw new RangeError(`${name} is out of range`);
    return integer;
  };
  const primitiveText = (value) => {
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
  const monthCodeField = (value) => {
    const code = primitiveText(value);
    if (typeof code !== 'string') throw new TypeError('monthCode must be a string');
    if (!/^M[0-9]{2}L?$/.test(code)) throw new RangeError(`Invalid month code ${code}`);
    return code;
  };
  // A calendar property: an iso8601 id, or any string that parses as an ISO date.
  const checkCalendar = (value) => {
    if (typeof value === 'string') {
      try {
        op('PlainMonthDay', 'parse', value);
        return;
      } catch (err) {
        void err; // not an ISO string: fall through to the calendar id check
      }
    }
    if (calendarId(value) !== 'iso8601') throw new RangeError('Only the iso8601 calendar is supported');
  };
  // Alphabetical, one Get and conversion each.
  const readFields = (bag, withCalendar) => {
    const out = {};
    if (withCalendar && bag.calendar !== undefined) checkCalendar(bag.calendar);
    if (bag.day !== undefined) out.day = integerField(bag.day, 'day', 1);
    if (bag.month !== undefined) out.month = integerField(bag.month, 'month', 1);
    if (bag.monthCode !== undefined) out.monthCode = monthCodeField(bag.monthCode);
    if (bag.year !== undefined) out.year = integerField(bag.year, 'year');
    return out;
  };

  class PlainMonthDay {
    constructor(isoMonth, isoDay, calendar = 'iso8601', referenceISOYear = 1972) {
      const month = integerField(isoMonth, 'isoMonth', 1);
      const day = integerField(isoDay, 'isoDay', 1);
      if (calendarId(calendar) !== 'iso8601') throw new RangeError('Only the iso8601 calendar is supported');
      const year = integerField(referenceISOYear, 'referenceISOYear');
      slots.set(this, { cls: 'PlainMonthDay', str: op('PlainMonthDay', 'create', month, day, year, 'reject') });
    }

    static from(item, options) {
      if (typeof item === 'string') {
        const str = op('PlainMonthDay', 'parse', item);
        overflowOf(options);
        return wrapMonthDay(str);
      }
      if (notObject(item)) throw new TypeError('Invalid PlainMonthDay');
      const slot = slotOf(item);
      if (slot !== undefined && slot.cls === 'PlainMonthDay') {
        overflowOf(options);
        return wrapMonthDay(slot.str);
      }
      const given = readFields(item, true);
      if (given.day === undefined) throw new TypeError('day is required');
      if (given.monthCode === undefined && given.month === undefined) throw new TypeError('month or monthCode is required');
      const str = op('PlainMonthDay', 'fromFields', given.year, given.month, given.monthCode, given.day, overflowOf(options));
      return wrapMonthDay(str);
    }

    with(bag, options) {
      const base = strOf(this, 'PlainMonthDay');
      if (notObject(bag) || slotOf(bag) !== undefined) throw new TypeError('with() needs a plain object');
      if (bag.calendar !== undefined || bag.timeZone !== undefined) {
        throw new TypeError('calendar and timeZone cannot be changed here');
      }
      const given = readFields(bag, false);
      if (Object.keys(given).length === 0) throw new TypeError('with() needs at least one recognized property');
      const str = op('PlainMonthDay', 'with', base, given.year, given.month, given.monthCode, given.day, overflowOf(options));
      return wrapMonthDay(str);
    }

    toPlainDate(item) {
      const str = strOf(this, 'PlainMonthDay');
      if (notObject(item)) throw new TypeError('toPlainDate() needs an object with a year');
      const { year } = item;
      if (year === undefined) throw new TypeError('year is required');
      const text = op('PlainMonthDay', 'toDate', str, integerField(year, 'year'));
      return wrap(Temporal.PlainDate.prototype, 'PlainDate', text);
    }

    toString(options) {
      const str = strOf(this, 'PlainMonthDay');
      const name = optionText(optionsOf(options), 'calendarName');
      if (name !== undefined && !['auto', 'always', 'never', 'critical'].includes(name)) {
        throw new RangeError(`${name} is not a valid calendarName`);
      }
      return op('PlainMonthDay', 'toString', str, name);
    }
  }
  defineProperty(PlainMonthDay, 'length', { value: 2, configurable: true });

  getters(PlainMonthDay.prototype, {
    calendarId() { strOf(this, 'PlainMonthDay'); return 'iso8601'; },
    monthCode() { return fieldsOf(this, 'PlainMonthDay')[0]; },
    day() { return Number(fieldsOf(this, 'PlainMonthDay')[1]); },
  });
  tag(PlainMonthDay.prototype, 'Temporal.PlainMonthDay');
  const intrinsicToString = PlainMonthDay.prototype.toString;
  defineMethods(PlainMonthDay.prototype, {
    equals(other) {
      const a = strOf(this, 'PlainMonthDay');
      return op('PlainMonthDay', 'equals', a, strOf(PlainMonthDay.from(other), 'PlainMonthDay')) === 'true';
    },
    toJSON() { strOf(this, 'PlainMonthDay'); return intrinsicToString.call(this); },
    toLocaleString() { strOf(this, 'PlainMonthDay'); return intrinsicToString.call(this); },
    valueOf() { throw new TypeError('Temporal.PlainMonthDay cannot be converted to a primitive'); },
  }, { equals: 1, toJSON: 0, toLocaleString: 0, valueOf: 0 });
  for (const method of ['with', 'toPlainDate']) {
    defineProperty(PlainMonthDay.prototype[method], 'length', { value: 1, configurable: true });
  }
  defineProperty(PlainMonthDay.prototype.toString, 'length', { value: 0, configurable: true });
  defineProperty(PlainMonthDay.from, 'length', { value: 1, configurable: true });
  define(Temporal, { PlainMonthDay });
})();
