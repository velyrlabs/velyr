'use strict';
// Temporal.PlainMonthDay (see prelude.js). The slot string is the native
// `isoMonth,isoDay,referenceYear[u-ca=id]` state.
(() => {
  const T = globalThis.__temporal_shared;
  const {
    op, slots, getters, wrap, slotOf, strOf, fieldsOf, optionsOf, optionText,
    overflowOf, Temporal, calendarId, slotCalendar, define, defineMethods, tag,
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
  const ISO = 'iso8601';
  // Calendars with eras also read `era` and `eraYear` (alphabetical order: after `day`).
  const ERA_FREE_CALENDARS = [ISO, 'chinese', 'dangi'];
  const withEras = (names, calendar) => (ERA_FREE_CALENDARS.includes(calendar)
    ? names : names.flatMap((name) => (name === 'day' ? [name, 'era', 'eraYear'] : [name])));
  const BAG_NAMES = ['day', 'month', 'monthCode', 'year'];
  // The calendar of a property bag (an id or any ISO string), read before any other field.
  // A Temporal object (PlainDate, ...) keeps its calendar in its slot.
  const bagCalendar = (bag) => (slotOf(bag) !== undefined ? slotCalendar(slotOf(bag).str) : bag.calendar === undefined ? ISO : calendarId(bag.calendar, true));
  // Alphabetical, one Get and conversion each.
  const readFields = (bag, names) => {
    const out = {};
    for (const name of names) {
      const value = bag[name];
      if (value === undefined) continue;
      if (name === 'monthCode') out.monthCode = monthCodeField(value);
      else if (name === 'era') out.era = `${value}`;
      else out[name] = integerField(value, name, name === 'day' || name === 'month' ? 1 : undefined);
    }
    return out;
  };
  const calendarFieldArgs = (given) => [given.year, given.month, given.monthCode, given.day, given.era, given.eraYear];

  class PlainMonthDay {
    constructor(isoMonth, isoDay, calendar = 'iso8601', referenceISOYear = 1972) {
      const month = integerField(isoMonth, 'isoMonth', 1);
      const day = integerField(isoDay, 'isoDay', 1);
      const id = calendarId(calendar);
      const year = integerField(referenceISOYear, 'referenceISOYear');
      slots.set(this, { cls: 'PlainMonthDay', str: op('PlainMonthDay', 'create', month, day, year, 'reject', id) });
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
      const calendar = bagCalendar(item);
      const given = readFields(item, withEras(BAG_NAMES, calendar));
      if (given.day === undefined) throw new TypeError('day is required');
      if (given.monthCode === undefined && given.month === undefined) throw new TypeError('month or monthCode is required');
      const str = op('PlainMonthDay', 'fromFields', calendar, ...calendarFieldArgs(given), overflowOf(options));
      return wrapMonthDay(str);
    }

    with(bag, options) {
      const base = strOf(this, 'PlainMonthDay');
      if (notObject(bag) || slotOf(bag) !== undefined) throw new TypeError('with() needs a plain object');
      if (bag.calendar !== undefined || bag.timeZone !== undefined) {
        throw new TypeError('calendar and timeZone cannot be changed here');
      }
      const given = readFields(bag, withEras(BAG_NAMES, slotCalendar(base)));
      if (Object.keys(given).length === 0) throw new TypeError('with() needs at least one recognized property');
      const str = op('PlainMonthDay', 'with', base, ...calendarFieldArgs(given), overflowOf(options));
      return wrapMonthDay(str);
    }

    toPlainDate(item) {
      const str = strOf(this, 'PlainMonthDay');
      if (notObject(item)) throw new TypeError('toPlainDate() needs an object with a year');
      const names = ERA_FREE_CALENDARS.includes(slotCalendar(str)) ? ['year'] : ['era', 'eraYear', 'year'];
      const given = readFields(item, names);
      if (given.year === undefined && (given.era === undefined || given.eraYear === undefined)) throw new TypeError('year is required');
      const text = op('PlainMonthDay', 'toDate', str, ...calendarFieldArgs(given));
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
    calendarId() { return slotCalendar(strOf(this, 'PlainMonthDay')); },
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
