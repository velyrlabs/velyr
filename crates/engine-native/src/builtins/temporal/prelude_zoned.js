'use strict';
// Instant, ZonedDateTime and Now (see prelude.js); the last prelude, so it removes the
// shared helpers from the global.
(() => {
  const T = globalThis.__temporal_shared;
  delete globalThis.__temporal_shared;
  const {
    op, slots, define, getters, wrap, slotOf, strOf, fieldsOf, toInt, optionsOf, optionText,
    overflowOf, readBag, Temporal, installCommon, stringArgs, roundArgs, calendarSuffix,
  } = T;

  const NS_PER_MS = 1000000n;
  const notObject = (item) => typeof item !== 'object' || item === null;

  // ---- Instant ----
  const wrapInstant = (str) => wrap(Instant.prototype, 'Instant', str);
  const toBigInt = (value) => {
    if (typeof value !== 'bigint') throw new TypeError('epoch nanoseconds must be a BigInt');
    return value;
  };

  class Instant {
    constructor(epochNanoseconds) {
      slots.set(this, { cls: 'Instant', str: op('Instant', 'create', toBigInt(epochNanoseconds)) });
    }

    static from(item) {
      if (typeof item === 'string') return wrapInstant(op('Instant', 'parse', item));
      const slot = slotOf(item);
      if (slot !== undefined && slot.cls === 'Instant') return wrapInstant(slot.str);
      if (slot !== undefined && slot.cls === 'ZonedDateTime') return item.toInstant();
      throw new TypeError('Invalid Instant');
    }

    static fromEpochMilliseconds(epochMilliseconds) {
      const ms = toInt(epochMilliseconds, 'epochMilliseconds');
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
      const options = typeof roundTo === 'string' ? { smallestUnit: roundTo } : roundTo;
      return wrapInstant(op('Instant', 'round', strOf(this, 'Instant'), ...roundArgs(options)));
    }

    toZonedDateTimeISO(timeZone) {
      const str = op('Instant', 'toZoned', strOf(this, 'Instant'), T.timeZoneIdOf(timeZone));
      return wrap(ZonedDateTime.prototype, 'ZonedDateTime', str);
    }

    toString(options) {
      const o = optionsOf(options);
      const timeZone = o.timeZone === undefined ? undefined : T.timeZoneIdOf(o.timeZone);
      return op('Instant', 'toString', strOf(this, 'Instant'), timeZone, ...stringArgs(options));
    }
  }
  installCommon(Instant, 'Instant', wrapInstant);

  // ---- ZonedDateTime ----
  const wrapZoned = (str) => wrap(ZonedDateTime.prototype, 'ZonedDateTime', str);
  const ZONED_BAG = ['day', 'hour', 'microsecond', 'millisecond', 'minute', 'month', 'monthCode', 'nanosecond', 'offset', 'second', 'timeZone', 'year'];
  const choices = (options) => {
    const o = optionsOf(options);
    return [optionText(o, 'disambiguation'), optionText(o, 'offset')];
  };
  // The local date-time of a bag (via PlainDateTime) plus its offset and zone.
  const zonedFromBag = (bag, timeZone, overflow, offset, options) => {
    const local = Temporal.PlainDateTime.from(bag, { overflow }).toString();
    const [disambiguation, offsetOption] = choices(options);
    const text = `${local}${offset ?? ''}[${timeZone}]`;
    return wrapZoned(op('ZonedDateTime', 'parse', text, disambiguation, offsetOption));
  };

  class ZonedDateTime {
    constructor(epochNanoseconds, timeZone) {
      const str = op('ZonedDateTime', 'create', toBigInt(epochNanoseconds), T.timeZoneIdOf(timeZone));
      slots.set(this, { cls: 'ZonedDateTime', str });
    }

    static from(item, options) {
      const overflow = overflowOf(options);
      const [disambiguation, offsetOption] = choices(options);
      if (typeof item === 'string') return wrapZoned(op('ZonedDateTime', 'parse', item, disambiguation, offsetOption));
      if (notObject(item)) throw new TypeError('Invalid ZonedDateTime');
      const slot = slotOf(item);
      if (slot !== undefined) {
        if (slot.cls !== 'ZonedDateTime') throw new TypeError('Invalid ZonedDateTime');
        return wrapZoned(slot.str);
      }
      if (item.timeZone === undefined) throw new TypeError('timeZone is required');
      const bag = readBag(item, ZONED_BAG);
      const offset = bag.offset === undefined ? undefined : String(bag.offset);
      return zonedFromBag(bag, T.timeZoneIdOf(item.timeZone), overflow, offset, options);
    }

    static compare(a, b) {
      return Number(op('ZonedDateTime', 'compare', strOf(ZonedDateTime.from(a), 'ZonedDateTime'), strOf(ZonedDateTime.from(b), 'ZonedDateTime')));
    }

    get timeZoneId() { return fieldsOf(this, 'ZonedDateTime')[21]; }
    get offset() { return fieldsOf(this, 'ZonedDateTime')[18]; }
    get offsetNanoseconds() { return Number(fieldsOf(this, 'ZonedDateTime')[19]); }
    get hoursInDay() { return Number(fieldsOf(this, 'ZonedDateTime')[22]); }
    get epochNanoseconds() { return BigInt(fieldsOf(this, 'ZonedDateTime')[20]); }
    get epochMilliseconds() { return Number(this.epochNanoseconds / NS_PER_MS - (this.epochNanoseconds % NS_PER_MS < 0n ? 1n : 0n)); }

    with(bag, options) {
      const current = fieldsOf(this, 'ZonedDateTime');
      if (notObject(bag)) throw new TypeError('with() needs an object');
      if (bag.calendar !== undefined || bag.timeZone !== undefined) throw new TypeError('calendar and timeZone cannot be changed here');
      const given = readBag(bag, ZONED_BAG);
      if (Object.keys(given).length === 0) throw new TypeError('with() needs at least one recognized property');
      const local = Temporal.PlainDateTime.from(this).with(given, { overflow: overflowOf(options) }).toString();
      const [disambiguation, offsetOption] = choices(options);
      const offset = given.offset === undefined ? current[18] : String(given.offset);
      const text = `${local}${offset}[${current[21]}]`;
      return wrapZoned(op('ZonedDateTime', 'parse', text, disambiguation, offsetOption ?? 'prefer'));
    }

    withTimeZone(timeZone) {
      return wrapZoned(op('ZonedDateTime', 'withTimeZone', strOf(this, 'ZonedDateTime'), T.timeZoneIdOf(timeZone)));
    }

    startOfDay() {
      return wrapZoned(op('ZonedDateTime', 'startOfDay', strOf(this, 'ZonedDateTime')));
    }

    round(roundTo) {
      const options = typeof roundTo === 'string' ? { smallestUnit: roundTo } : roundTo;
      return wrapZoned(op('ZonedDateTime', 'round', strOf(this, 'ZonedDateTime'), ...roundArgs(options)));
    }

    toInstant() {
      return wrapInstant(op('ZonedDateTime', 'toInstant', strOf(this, 'ZonedDateTime')));
    }

    toPlainDate() { return Temporal.PlainDate.from(this); }
    toPlainTime() { return Temporal.PlainTime.from(this); }
    toPlainDateTime() { return Temporal.PlainDateTime.from(this); }

    toString(options) {
      const o = optionsOf(options);
      const display = ['offset', 'timeZoneName', 'calendarName'].map((name) => optionText(o, name));
      const [offset, zoneName, calendar] = display;
      const str = op('ZonedDateTime', 'toString', strOf(this, 'ZonedDateTime'), offset, zoneName, 'auto', ...stringArgs(options));
      return calendarSuffix(str, { calendarName: calendar });
    }
  }
  T.installDateGetters(ZonedDateTime.prototype, 'ZonedDateTime');
  T.installTimeGetters(ZonedDateTime.prototype, 'ZonedDateTime', 12);
  installCommon(ZonedDateTime, 'ZonedDateTime', wrapZoned, true);

  // ---- Now ----
  const Now = {};
  T.tag(Now, 'Temporal.Now');
  const nowInstant = () => wrapInstant(op('Instant', 'create', op('Now', 'epochNanoseconds')));
  const zonedNow = (timeZone) =>
    nowInstant().toZonedDateTimeISO(timeZone === undefined ? op('Now', 'timeZoneId') : timeZone);
  define(Now, {
    instant: nowInstant,
    timeZoneId: () => op('Now', 'timeZoneId'),
    zonedDateTimeISO: zonedNow,
    plainDateTimeISO: (timeZone) => zonedNow(timeZone).toPlainDateTime(),
    plainDateISO: (timeZone) => zonedNow(timeZone).toPlainDate(),
    plainTimeISO: (timeZone) => zonedNow(timeZone).toPlainTime(),
  });

  define(Temporal, { Instant, ZonedDateTime, Now });
})();
