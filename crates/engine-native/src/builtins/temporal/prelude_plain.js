'use strict';
// PlainDate, PlainTime and PlainDateTime (see prelude.js).
(() => {
  const T = globalThis.__temporal_shared;
  const {
    op, slots, define, getters, wrap, slotOf, strOf, fieldsOf, toInt, toIntOr, optionsOf,
    overflowOf, readBag, monthFromCode, Temporal, installCommon, stringArgs, roundArgs,
    calendarSuffix,
  } = T;

  const DATE_NAMES = ['day', 'month', 'monthCode', 'year'];
  const TIME_NAMES = ['hour', 'minute', 'second', 'millisecond', 'microsecond', 'nanosecond'];
  const DATE_GETTERS = {
    year: 0, month: 1, day: 2, dayOfWeek: 3, dayOfYear: 4, weekOfYear: 5, yearOfWeek: 6,
    daysInMonth: 7, daysInYear: 8, monthsInYear: 9,
  };

  // Getters over the `fields` csv: 12 date fields, then 6 time fields.
  T.installDateGetters = (proto, cls) => {
    const table = { calendarId: () => 'iso8601' };
    for (const [name, index] of Object.entries(DATE_GETTERS)) {
      table[name] = function () { return Number(fieldsOf(this, cls)[index]); };
    }
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

  // [year, month, day] from a bag, filling gaps from the `base` fields.
  const dateParts = (bag, base) => {
    let month = bag.month === undefined ? undefined : toInt(bag.month, 'month', 1);
    if (bag.monthCode !== undefined) {
      const fromCode = monthFromCode(bag.monthCode);
      if (month !== undefined && month !== fromCode) throw new RangeError('month and monthCode disagree');
      month = fromCode;
    }
    const year = bag.year === undefined ? undefined : toInt(bag.year, 'year');
    const day = bag.day === undefined ? undefined : toInt(bag.day, 'day', 1);
    const parts = [year ?? (base && Number(base[0])), month ?? (base && Number(base[1])), day ?? (base && Number(base[2]))];
    if (parts.some((part) => part === undefined)) throw new TypeError('year, month and day are required');
    return parts;
  };
  const timeParts = (bag, base) =>
    TIME_NAMES.map((name, i) => toIntOr(bag[name], base ? Number(base[i]) : 0, name, 0));
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

  // ---- PlainDate ----
  const wrapDate = (str) => wrap(PlainDate.prototype, 'PlainDate', str);
  const timeText = (time) => (time === undefined ? '00:00:00' : strOf(Temporal.PlainTime.from(time), 'PlainTime'));

  class PlainDate {
    constructor(year, month, day) {
      const parts = [toInt(year, 'year'), toInt(month, 'month', 1), toInt(day, 'day', 1)];
      slots.set(this, { cls: 'PlainDate', str: op('PlainDate', 'create', ...parts, 'reject') });
    }

    static from(item, options) {
      const overflow = overflowOf(options);
      if (typeof item === 'string') return wrapDate(op('PlainDate', 'parse', item));
      if (notObject(item)) throw new TypeError('Invalid PlainDate');
      if (slotOf(item) !== undefined) {
        return wrapDate(op('PlainDate', 'parse', fromSlot(item, ['PlainDate', 'PlainDateTime', 'ZonedDateTime']).str));
      }
      return wrapDate(op('PlainDate', 'create', ...dateParts(readBag(item, DATE_NAMES)), overflow));
    }

    with(bag, options) {
      const base = fieldsOf(this, 'PlainDate');
      if (notObject(bag)) throw new TypeError('with() needs an object');
      noZoneOrCalendar(bag);
      const given = readBag(bag, DATE_NAMES);
      needSome(given, DATE_NAMES, 'with()');
      return wrapDate(op('PlainDate', 'create', ...dateParts(given, base), overflowOf(options)));
    }

    toPlainDateTime(time) {
      const str = `${strOf(this, 'PlainDate')}T${timeText(time)}`;
      return Temporal.PlainDateTime.from(str);
    }

    toZonedDateTime(item) {
      const bag = typeof item === 'object' && item !== null && slotOf(item) === undefined ? item : { timeZone: item };
      const time = bag.plainTime === undefined ? undefined : strOf(Temporal.PlainTime.from(bag.plainTime), 'PlainTime');
      const str = op('PlainDate', 'toZoned', strOf(this, 'PlainDate'), T.timeZoneIdOf(bag.timeZone), time);
      return wrap(Temporal.ZonedDateTime.prototype, 'ZonedDateTime', str);
    }

    toString(options) {
      return calendarSuffix(strOf(this, 'PlainDate'), options);
    }
  }
  T.installDateGetters(PlainDate.prototype, 'PlainDate');
  installCommon(PlainDate, 'PlainDate', wrapDate);

  // ---- PlainTime ----
  const wrapTime = (str) => wrap(PlainTime.prototype, 'PlainTime', str);

  class PlainTime {
    constructor(hour, minute, second, millisecond, microsecond, nanosecond) {
      const given = { hour, minute, second, millisecond, microsecond, nanosecond };
      slots.set(this, { cls: 'PlainTime', str: op('PlainTime', 'create', ...timeParts(given), 'reject') });
    }

    static from(item, options) {
      const overflow = overflowOf(options);
      if (typeof item === 'string') return wrapTime(op('PlainTime', 'parse', item));
      if (notObject(item)) throw new TypeError('Invalid PlainTime');
      if (slotOf(item) !== undefined) {
        return wrapTime(op('PlainTime', 'parse', fromSlot(item, ['PlainTime', 'PlainDateTime', 'ZonedDateTime']).str));
      }
      const bag = readBag(item, TIME_NAMES);
      needSome(bag, TIME_NAMES, 'PlainTime.from()');
      return wrapTime(op('PlainTime', 'create', ...timeParts(bag), overflow));
    }

    with(bag, options) {
      const base = fieldsOf(this, 'PlainTime');
      if (notObject(bag)) throw new TypeError('with() needs an object');
      noZoneOrCalendar(bag);
      const given = readBag(bag, TIME_NAMES);
      needSome(given, TIME_NAMES, 'with()');
      return wrapTime(op('PlainTime', 'create', ...timeParts(given, base), overflowOf(options)));
    }

    round(roundTo) {
      const options = typeof roundTo === 'string' ? { smallestUnit: roundTo } : roundTo;
      return wrapTime(op('PlainTime', 'round', strOf(this, 'PlainTime'), ...roundArgs(options)));
    }

    toString(options) {
      return op('PlainTime', 'toString', strOf(this, 'PlainTime'), ...stringArgs(options));
    }
  }
  T.installTimeGetters(PlainTime.prototype, 'PlainTime', 0);
  installCommon(PlainTime, 'PlainTime', wrapTime);

  // ---- PlainDateTime ----
  const wrapDateTime = (str) => wrap(PlainDateTime.prototype, 'PlainDateTime', str);

  class PlainDateTime {
    constructor(year, month, day, hour, minute, second, millisecond, microsecond, nanosecond) {
      const date = [toInt(year, 'year'), toInt(month, 'month', 1), toInt(day, 'day', 1)];
      const time = timeParts({ hour, minute, second, millisecond, microsecond, nanosecond });
      slots.set(this, { cls: 'PlainDateTime', str: op('PlainDateTime', 'create', ...date, ...time, 'reject') });
    }

    static from(item, options) {
      const overflow = overflowOf(options);
      if (typeof item === 'string') return wrapDateTime(op('PlainDateTime', 'parse', item));
      if (notObject(item)) throw new TypeError('Invalid PlainDateTime');
      if (slotOf(item) !== undefined) {
        const slot = fromSlot(item, ['PlainDate', 'PlainDateTime', 'ZonedDateTime']);
        const str = slot.cls === 'PlainDate' ? `${slot.str}T00:00:00` : slot.str;
        return wrapDateTime(op('PlainDateTime', 'parse', str));
      }
      const parts = [...dateParts(readBag(item, DATE_NAMES)), ...timeParts(readBag(item, TIME_NAMES))];
      return wrapDateTime(op('PlainDateTime', 'create', ...parts, overflow));
    }

    with(bag, options) {
      const base = fieldsOf(this, 'PlainDateTime');
      if (notObject(bag)) throw new TypeError('with() needs an object');
      noZoneOrCalendar(bag);
      const date = readBag(bag, DATE_NAMES);
      const time = readBag(bag, TIME_NAMES);
      needSome({ ...date, ...time }, [...DATE_NAMES, ...TIME_NAMES], 'with()');
      const parts = [...dateParts(date, base), ...timeParts(time, base.slice(12))];
      return wrapDateTime(op('PlainDateTime', 'create', ...parts, overflowOf(options)));
    }

    round(roundTo) {
      const options = typeof roundTo === 'string' ? { smallestUnit: roundTo } : roundTo;
      return wrapDateTime(op('PlainDateTime', 'round', strOf(this, 'PlainDateTime'), ...roundArgs(options)));
    }

    toPlainDate() {
      return PlainDate.from(this);
    }

    toPlainTime() {
      return PlainTime.from(this);
    }

    toZonedDateTime(timeZone, options) {
      const disambiguation = optionsOf(options).disambiguation;
      const str = op('PlainDateTime', 'toZoned', strOf(this, 'PlainDateTime'), T.timeZoneIdOf(timeZone), disambiguation);
      return wrap(Temporal.ZonedDateTime.prototype, 'ZonedDateTime', str);
    }

    toString(options) {
      const str = op('PlainDateTime', 'toString', strOf(this, 'PlainDateTime'), ...stringArgs(options));
      return calendarSuffix(str, options);
    }
  }
  T.installDateGetters(PlainDateTime.prototype, 'PlainDateTime');
  T.installTimeGetters(PlainDateTime.prototype, 'PlainDateTime', 12);
  installCommon(PlainDateTime, 'PlainDateTime', wrapDateTime);

  define(Temporal, { PlainDate, PlainTime, PlainDateTime });
})();
