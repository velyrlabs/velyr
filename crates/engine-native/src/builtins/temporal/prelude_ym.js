'use strict';
// Temporal.PlainYearMonth, iso8601 only (see prelude.js). The slot string is the native
// `isoYear,isoMonth,referenceDay` state.
(() => {
  const T = globalThis.__temporal_shared;
  const {
    op, slots, getters, wrap, slotOf, strOf, fieldsOf, optionsOf, optionText,
    overflowOf, monthFromCode, Temporal, installCommon, calendarId, define,
  } = T;
  const { defineProperty } = Object;

  const wrapYearMonth = (str) => wrap(PlainYearMonth.prototype, 'PlainYearMonth', str);
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
        op('PlainYearMonth', 'parse', value);
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
    if (bag.month !== undefined) out.month = integerField(bag.month, 'month', 1);
    if (bag.monthCode !== undefined) out.monthCode = monthCodeField(bag.monthCode);
    if (bag.year !== undefined) out.year = integerField(bag.year, 'year');
    return out;
  };
  const resolveMonth = (given) => {
    if (given.monthCode === undefined) return given.month;
    const fromCode = monthFromCode(given.monthCode);
    if (fromCode > 12) throw new RangeError(`monthCode ${given.monthCode} is not valid for iso8601`);
    if (given.month !== undefined && given.month !== fromCode) throw new RangeError('month and monthCode disagree');
    return fromCode;
  };
  const createFrom = (year, month, overflow) => op('PlainYearMonth', 'create', year, month, 1, overflow);

  class PlainYearMonth {
    constructor(isoYear, isoMonth, calendar = 'iso8601', referenceISODay = 1) {
      const year = integerField(isoYear, 'isoYear');
      const month = integerField(isoMonth, 'isoMonth', 1);
      if (calendarId(calendar) !== 'iso8601') throw new RangeError('Only the iso8601 calendar is supported');
      const day = integerField(referenceISODay, 'referenceISODay', 1);
      slots.set(this, { cls: 'PlainYearMonth', str: op('PlainYearMonth', 'create', year, month, day, 'reject') });
    }

    static from(item, options) {
      if (typeof item === 'string') {
        const str = op('PlainYearMonth', 'parse', item);
        overflowOf(options);
        return wrapYearMonth(str);
      }
      if (notObject(item)) throw new TypeError('Invalid PlainYearMonth');
      const slot = slotOf(item);
      if (slot !== undefined && slot.cls === 'PlainYearMonth') {
        overflowOf(options);
        return wrapYearMonth(slot.str);
      }
      const given = readFields(item, true);
      const month = resolveMonth(given);
      if (given.year === undefined || month === undefined) throw new TypeError('year and month are required');
      return wrapYearMonth(createFrom(given.year, month, overflowOf(options)));
    }

    with(bag, options) {
      const base = fieldsOf(this, 'PlainYearMonth');
      if (notObject(bag) || slotOf(bag) !== undefined) throw new TypeError('with() needs a plain object');
      if (bag.calendar !== undefined || bag.timeZone !== undefined) {
        throw new TypeError('calendar and timeZone cannot be changed here');
      }
      const given = readFields(bag, false);
      if (Object.keys(given).length === 0) throw new TypeError('with() needs at least one recognized property');
      const overflow = overflowOf(options);
      const month = resolveMonth(given) ?? Number(base[1]);
      return wrapYearMonth(createFrom(given.year ?? Number(base[0]), month, overflow));
    }

    toPlainDate(item) {
      const str = strOf(this, 'PlainYearMonth');
      if (notObject(item)) throw new TypeError('toPlainDate() needs an object with a day');
      const { day } = item;
      if (day === undefined) throw new TypeError('day is required');
      const text = op('PlainYearMonth', 'toDate', str, integerField(day, 'day', 1));
      return wrap(Temporal.PlainDate.prototype, 'PlainDate', text);
    }

    toString(options) {
      const str = strOf(this, 'PlainYearMonth');
      const name = optionText(optionsOf(options), 'calendarName');
      if (name !== undefined && !['auto', 'always', 'never', 'critical'].includes(name)) {
        throw new RangeError(`${name} is not a valid calendarName`);
      }
      return op('PlainYearMonth', 'toString', str, name);
    }
  }
  defineProperty(PlainYearMonth, 'length', { value: 2, configurable: true });

  // `fields` csv: year, month, monthCode, daysInYear, daysInMonth, monthsInYear, inLeapYear.
  const FIELD_GETTERS = { year: 0, month: 1, daysInYear: 3, daysInMonth: 4, monthsInYear: 5 };
  const table = {
    calendarId() { fieldsOf(this, 'PlainYearMonth'); return 'iso8601'; },
    era() { fieldsOf(this, 'PlainYearMonth'); return undefined; },
    eraYear() { fieldsOf(this, 'PlainYearMonth'); return undefined; },
    monthCode() { return fieldsOf(this, 'PlainYearMonth')[2]; },
    inLeapYear() { return fieldsOf(this, 'PlainYearMonth')[6] === '1'; },
  };
  for (const [name, index] of Object.entries(FIELD_GETTERS)) {
    table[name] = function () { return Number(fieldsOf(this, 'PlainYearMonth')[index]); };
  }
  getters(PlainYearMonth.prototype, table);
  installCommon(PlainYearMonth, 'PlainYearMonth', wrapYearMonth);
  for (const method of ['with', 'toPlainDate', 'toString']) {
    const length = method === 'toString' ? 0 : 1;
    defineProperty(PlainYearMonth.prototype[method], 'length', { value: length, configurable: true });
  }
  defineProperty(PlainYearMonth.from, 'length', { value: 1, configurable: true });
  define(Temporal, { PlainYearMonth });
})();
