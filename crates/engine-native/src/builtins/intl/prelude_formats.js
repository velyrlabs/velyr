'use strict';
// Intl.DateTimeFormat and the locale-sensitive methods of the built-ins, which delegate
// to Intl (ECMA-402 chapters 19-21). Runs after prelude.js.
(() => {
  const global = globalThis;
  const shared = global.__intl_shared;
  const { ops, define, slots, slot, constructor, instance, resolveLocaleKeys, typeOption,
    toOptions, getOption, getNumber, decodeParts, typeError } = shared;
  const { defineProperty } = Object;
  const Intl = global.Intl;

  const DATE_FIELDS = ['weekday', 'year', 'month', 'day'];
  const TIME_FIELDS = ['dayPeriod', 'hour', 'minute', 'second', 'fractionalSecondDigits'];
  // The order `datetime.rs` reads the spec in.
  const SPEC_KEYS = ['dateStyle', 'timeStyle', 'weekday', 'era', 'year', 'month', 'day',
    'hour', 'minute', 'second', 'fractionalSecondDigits', 'hourCycle', 'timeZoneName', 'dayPeriod'];
  const TEXT = ['narrow', 'short', 'long'];
  const NUMERIC = ['2-digit', 'numeric'];
  const COMPONENTS = [
    ['weekday', TEXT], ['era', TEXT], ['year', NUMERIC], ['month', [...NUMERIC, ...TEXT]],
    ['day', NUMERIC], ['dayPeriod', TEXT], ['hour', NUMERIC], ['minute', NUMERIC],
    ['second', NUMERIC],
  ];
  const STYLES = ['full', 'long', 'medium', 'short'];

  // 12-hour clock of locales whose hour12 resolves to something else than h12.
  const TWELVE_HOUR = new Map([['ja', 'h11']]);
  const CALENDAR_ALIASES = new Map([['ethiopic-amete-alem', 'ethioaa'], ['islamicc', 'islamic-civil']]);
  const specOf = (record) => SPEC_KEYS.map((key) => (record[key] === undefined ? '' : String(record[key]))).join(',');
  // The hour cycle a locale uses by default, read off how it writes 0:00, 11:00 and 13:00.
  const defaultCycles = new Map();
  const defaultHourCycle = (base) => {
    let cycle = defaultCycles.get(base);
    if (cycle === undefined) {
      const spec = specOf(Object.assign(Object.create(null), { hour: 'numeric' }));
      const at = (hour) => decodeParts(ops.date_time_format(base, spec, hour * 3600000, 'UTC', true));
      if (!at(13).some((part) => part.type === 'dayPeriod')) {
        cycle = 'h23';
      } else {
        const text = (hour) => at(hour).find((part) => part.type === 'hour').value;
        cycle = text(0).length < text(11).length ? 'h11' : 'h12';
      }
      defaultCycles.set(base, cycle);
    }
    return cycle;
  };
  const resolveHourCycle = (base, hour12, requested) => {
    const fallback = defaultHourCycle(base);
    if (hour12 === undefined) return requested ?? fallback;
    if (!hour12) return 'h23';
    return fallback === 'h11' || fallback === 'h12' ? fallback : TWELVE_HOUR.get(base.split('-')[0]) ?? 'h12';
  };

  // ChainDateTimeFormat: `Intl.DateTimeFormat.call(obj)` on an instance hangs the real
  // formatter on `obj` under a private symbol, which format/resolvedOptions unwrap.
  const FALLBACK = Symbol('IntlLegacyConstructedSymbol');
  function DateTimeFormat(locales = undefined, options = undefined) {
    const record = createDateTimeFormat(locales, options, 'any', 'date');
    if (new.target === undefined && DateTimeFormat.prototype.isPrototypeOf(this)) {
      const fallback = Object.create(DateTimeFormat.prototype);
      slots.set(fallback, record);
      defineProperty(this, FALLBACK, { value: fallback });
      return this;
    }
    const self = instance(new.target, this, DateTimeFormat);
    slots.set(self, record);
    return self;
  }
  const unwrap = (self, method) => {
    const own = typeof self === 'object' && self !== null && slots.get(self)?.kind === 'DateTimeFormat';
    const target = !own && DateTimeFormat.prototype.isPrototypeOf(self) ? self[FALLBACK] : self;
    return slot(target, 'DateTimeFormat', method);
  };
  const hasAny = (record, fields) => fields.some((field) => record[field] !== undefined);
  // `zone`: the fixed time zone of a Temporal.ZonedDateTime, which rejects a `timeZone` option.
  const createDateTimeFormat = (locales, options, required, defaults, zone = undefined) => {
    if (options === null) typeError('Cannot convert undefined or null to object');
    options = toOptions(options);
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const calendarOption = typeOption(options, 'calendar');
    const calendar = CALENDAR_ALIASES.get(calendarOption) ?? calendarOption;
    const numberingSystem = typeOption(options, 'numberingSystem');
    const hour12 = getOption(options, 'hour12', Boolean);
    const hourCycleOption = getOption(options, 'hourCycle', ['h11', 'h12', 'h23', 'h24']);
    const { locale, data, values } = resolveLocaleKeys(locales,
      { ca: calendar, nu: numberingSystem, hc: hour12 === undefined ? hourCycleOption : (hour12 ? 'h12' : 'h23') });
    let timeZone = options.timeZone;
    if (zone !== undefined) {
      if (timeZone !== undefined) typeError('The timeZone option is not allowed for a Temporal.ZonedDateTime');
      timeZone = ops.canonical_zone(zone) ?? zone;
    } else if (timeZone === undefined) {
      timeZone = ops.default_zone();
    } else {
      const name = String(timeZone);
      timeZone = ops.canonical_zone(name);
      if (timeZone === undefined) throw new RangeError(`Invalid time zone specified: ${name}`);
    }
    const record = Object.create(null);
    Object.assign(record, { kind: 'DateTimeFormat', locale, data, calendar: values.ca ?? 'gregory',
      numberingSystem: values.nu ?? 'latn', timeZone });
    let explicit;
    for (const [name, allowed] of COMPONENTS) {
      const value = getOption(options, name, allowed);
      if (value !== undefined) { record[name] = value; explicit = name; }
    }
    const fractional = getNumber(options, 'fractionalSecondDigits', 1, 3, undefined);
    if (fractional !== undefined) { record.fractionalSecondDigits = fractional; explicit = 'fractionalSecondDigits'; }
    const timeZoneName = getOption(options, 'timeZoneName',
      ['short', 'long', 'shortOffset', 'longOffset', 'shortGeneric', 'longGeneric']);
    if (timeZoneName !== undefined) { record.timeZoneName = timeZoneName; explicit = 'timeZoneName'; }
    getOption(options, 'formatMatcher', ['basic', 'best fit']);
    const dateStyle = getOption(options, 'dateStyle', STYLES);
    const timeStyle = getOption(options, 'timeStyle', STYLES);
    // What was asked for, before the defaults: each Temporal type derives its own fields from it.
    const requested = Object.create(null);
    for (const key of SPEC_KEYS) requested[key] = record[key];
    Object.assign(requested, { dateStyle, timeStyle });
    record.requested = requested;
    record.cycle = [data.split('-u-')[0], hour12, values.hc ?? hourCycleOption];
    if (dateStyle !== undefined || timeStyle !== undefined) {
      if (explicit !== undefined) typeError(`Can't set option ${explicit} when dateStyle or timeStyle is used`);
      if (required === 'date' && timeStyle !== undefined) typeError('Invalid option : timeStyle');
      if (required === 'time' && dateStyle !== undefined) typeError('Invalid option : dateStyle');
      record.dateStyle = dateStyle;
      record.timeStyle = timeStyle;
    } else {
      const needDefaults = !(required !== 'time' && hasAny(record, DATE_FIELDS))
        && !(required !== 'date' && hasAny(record, TIME_FIELDS));
      if (needDefaults && (defaults === 'date' || defaults === 'all')) Object.assign(record, { year: 'numeric', month: 'numeric', day: 'numeric' });
      if (needDefaults && (defaults === 'time' || defaults === 'all')) Object.assign(record, { hour: 'numeric', minute: 'numeric', second: 'numeric' });
      if (needDefaults && zone !== undefined && record.timeZoneName === undefined) record.timeZoneName = 'short';
    }
    if (record.hour !== undefined || timeStyle !== undefined) {
      record.hourCycle = resolveHourCycle(...record.cycle);
    }
    record.spec = specOf(record);
    return record;
  };
  const timeClip = (value) => {
    const t = Math.abs(value) > 8.64e15 ? NaN : Math.trunc(value) + 0;
    if (t !== t) throw new RangeError('Invalid time value');
    return t;
  };

  // Temporal inputs (HandleDateTimeValue). The values are recognized by the tag on their
  // prototype and read through their public API.
  // shortcut: a user-patched Temporal prototype changes what is read; the Temporal side
  // would have to hand over its internal slots to avoid it.
  const KINDS = ['PlainDate', 'PlainTime', 'PlainDateTime', 'PlainYearMonth', 'PlainMonthDay', 'Instant', 'ZonedDateTime'];
  const temporalKind = (value) => {
    if (typeof value !== 'object' || value === null) return undefined;
    const prototype = Object.getPrototypeOf(value);
    const tag = prototype === null ? undefined : Object.getOwnPropertyDescriptor(prototype, Symbol.toStringTag)?.value;
    return typeof tag === 'string' && tag.startsWith('Temporal.') && KINDS.includes(tag.slice(9)) ? tag.slice(9) : undefined;
  };
  // The fields of each Temporal type that a formatter's options can show.
  const DATE_PART = ['weekday', 'era', 'year', 'month', 'day'];
  const TIME_PART = ['dayPeriod', 'hour', 'minute', 'second', 'fractionalSecondDigits'];
  const SHOWN = { PlainDate: DATE_PART, PlainTime: TIME_PART, PlainDateTime: [...DATE_PART, ...TIME_PART],
    PlainYearMonth: ['era', 'year', 'month'], PlainMonthDay: ['month', 'day'],
    Instant: [...DATE_PART, ...TIME_PART, 'timeZoneName'] };
  // The fields that stop a type from using its default ones (`era` and `timeZoneName` do not).
  const EXPLICIT = ['weekday', 'year', 'month', 'day', ...TIME_PART];
  const DEFAULTS = { PlainDate: ['year', 'month', 'day'], PlainTime: ['hour', 'minute', 'second'],
    PlainDateTime: ['year', 'month', 'day', 'hour', 'minute', 'second'], PlainYearMonth: ['year', 'month'],
    PlainMonthDay: ['month', 'day'], Instant: ['year', 'month', 'day', 'hour', 'minute', 'second'] };
  // dateStyle for the types that have no day or no year: the same style as components.
  const PARTIAL_STYLES = {
    PlainYearMonth: { long: { year: 'numeric', month: 'long' }, medium: { year: 'numeric', month: 'short' },
      short: { year: '2-digit', month: 'numeric' } },
    PlainMonthDay: { long: { month: 'long', day: 'numeric' }, medium: { month: 'short', day: 'numeric' },
      short: { month: 'numeric', day: 'numeric' } },
  };
  // The options one Temporal type formats with; a formatter with nothing in common with it throws.
  const typeView = (record, kind) => {
    const { requested } = record;
    const view = Object.create(null);
    if (requested.dateStyle !== undefined || requested.timeStyle !== undefined) {
      const partial = PARTIAL_STYLES[kind];
      const dateStyle = kind === 'PlainTime' ? undefined : requested.dateStyle;
      if (dateStyle !== undefined && partial !== undefined) Object.assign(view, partial[dateStyle === 'full' ? 'long' : dateStyle]);
      else view.dateStyle = dateStyle;
      if (kind === 'PlainTime' || kind === 'PlainDateTime' || kind === 'Instant') view.timeStyle = requested.timeStyle;
      if (!hasAny(view, [...EXPLICIT, 'dateStyle', 'timeStyle'])) typeError(`The formatter has no field to show for a Temporal.${kind}`);
    } else if (hasAny(requested, EXPLICIT)) {
      for (const field of SHOWN[kind]) view[field] = requested[field];
      if (!hasAny(view, EXPLICIT)) typeError(`The formatter has no field to show for a Temporal.${kind}`);
    } else {
      for (const field of ['era', 'timeZoneName']) if (SHOWN[kind].includes(field)) view[field] = requested[field];
      for (const field of DEFAULTS[kind]) view[field] = 'numeric';
    }
    if (view.hour !== undefined || view.timeStyle !== undefined) view.hourCycle = resolveHourCycle(...record.cycle);
    return { data: record.data, timeZone: record.timeZone, spec: specOf(view) };
  };
  // Days since 1970-01-01 of a proleptic Gregorian date.
  const daysFromCivil = (year, month, day) => {
    const y = year - (month <= 2 ? 1 : 0);
    const era = Math.floor(y / 400);
    const yoe = y - era * 400;
    const doy = Math.floor((153 * (month + (month > 2 ? -3 : 9)) + 2) / 5) + day - 1;
    return era * 146097 + yoe * 365 + Math.floor(yoe / 4) - Math.floor(yoe / 100) + doy - 719468;
  };
  const ISO_DATE = /^(?:([+-]\d{6}|\d{4})-)?(\d\d)(?:-(\d\d))?/;
  const wallClock = (kind, value) => {
    let days = 0;
    if (kind !== 'PlainTime') {
      const partial = kind === 'PlainYearMonth' || kind === 'PlainMonthDay';
      const [, year = '1972', month, day = '1'] = ISO_DATE.exec(value.toString({ calendarName: partial ? 'always' : 'never' }));
      days = daysFromCivil(Number(year), Number(month), Number(day));
    }
    const hasTime = kind === 'PlainTime' || kind === 'PlainDateTime';
    const time = hasTime ? ((value.hour * 60 + value.minute) * 60 + value.second) * 1000 + value.millisecond : 0;
    return days * 86400000 + time;
  };
  // Only a PlainDate-like value may carry the ISO calendar into another one's formatter.
  const checkCalendar = (record, kind, value) => {
    if (kind === 'PlainTime' || kind === 'Instant') return;
    const calendar = value.calendarId;
    const anyCalendar = calendar === 'iso8601' && kind !== 'PlainYearMonth' && kind !== 'PlainMonthDay';
    if (calendar !== record.calendar && !anyCalendar) {
      throw new RangeError(`The calendar ${calendar} does not match the formatter calendar ${record.calendar}`);
    }
  };
  // ToDateTimeFormattable: a Temporal value stays as it is, anything else becomes a number.
  const formattable = (date) => {
    const kind = temporalKind(date);
    return { kind, value: kind === undefined ? Number(date) : date };
  };
  // What the native op needs for one value: the options, a time (or wall clock) and its kind.
  const resolveValue = (record, { kind, value }) => {
    if (kind === undefined) return { record, t: timeClip(value), wall: false };
    if (kind === 'ZonedDateTime') typeError('Temporal.ZonedDateTime is not supported by Intl.DateTimeFormat');
    checkCalendar(record, kind, value);
    record.views ??= Object.create(null);
    record.views[kind] ??= typeView(record, kind);
    const t = kind === 'Instant' ? value.epochMilliseconds : wallClock(kind, value);
    return { record: record.views[kind], t, wall: kind !== 'Instant' };
  };
  const renderValue = ({ record, t, wall }, parts) => ops.date_time_format(record.data, record.spec, t, record.timeZone, parts, wall);
  const formatDate = (record, date, parts) => {
    const item = date === undefined ? { kind: undefined, value: Date.now() } : formattable(date);
    return renderValue(resolveValue(record, item), parts);
  };

  // formatRange: both ends are formatted with the same options and joined.
  // shortcut: no CLDR interval patterns. Only a shared date before the time and, for the
  // month-day-year order, a shared year and month are collapsed; everything else repeats.
  const TIME_PARTS = ['hour', 'minute', 'second', 'fractionalSecond', 'dayPeriod', 'timeZoneName'];
  const fieldTypes = (parts) => parts.filter((part) => part.type !== 'literal').map((part) => part.type);
  const textOf = (parts) => parts.map((part) => part.value).join('');
  const rangeSeparator = (locale) => (locale.split('-')[0] === 'en' ? ' – ' : ' – ');
  const withSource = (parts, source) => parts.map((part) => ({ type: part.type, value: part.value, source }));
  const valueOf = (parts, type) => parts.find((part) => part.type === type)?.value;
  // The pieces of a range: parts shared before and after, and what differs in between.
  const collapseMonthDayYear = (start, end) => {
    const order = fieldTypes(start).join();
    if (order !== 'month,day,year' || fieldTypes(end).join() !== order || !/\D/.test(valueOf(start, 'month'))) return undefined;
    if (valueOf(start, 'year') !== valueOf(end, 'year')) return undefined;
    const dayAt = (parts) => parts.findIndex((part) => part.type === 'day');
    const sameMonth = valueOf(start, 'month') === valueOf(end, 'month');
    const from = sameMonth ? dayAt(start) : 0;
    return { prefix: start.slice(0, from), start: start.slice(from, dayAt(start) + 1),
      end: end.slice(from, dayAt(end) + 1), suffix: start.slice(dayAt(start) + 1) };
  };
  const timeStart = (parts) => parts.findIndex((part) => TIME_PARTS.includes(part.type));
  const collapseRange = (start, end) => {
    const [split, splitEnd] = [timeStart(start), timeStart(end)];
    if (split > 0 && textOf(start.slice(0, split)) === textOf(end.slice(0, splitEnd))) {
      return { prefix: start.slice(0, split), start: start.slice(split), end: end.slice(splitEnd), suffix: [] };
    }
    return (split < 0 && collapseMonthDayYear(start, end)) || { prefix: [], start, end, suffix: [] };
  };
  const rangeParts = (record, startDate, endDate) => {
    if (startDate === undefined || endDate === undefined) typeError('startDate and endDate are required');
    const items = [formattable(startDate), formattable(endDate)];
    if (items[0].kind !== items[1].kind) typeError('startDate and endDate must be the same kind of value');
    const [start, end] = items.map((item) => decodeParts(renderValue(resolveValue(record, item), true)));
    if (textOf(start) === textOf(end)) return withSource(start, 'shared');
    const separator = [{ type: 'literal', value: rangeSeparator(record.locale), source: 'shared' }];
    const range = collapseRange(start, end);
    return [...withSource(range.prefix, 'shared'), ...withSource(range.start, 'startRange'), ...separator,
      ...withSource(range.end, 'endRange'), ...withSource(range.suffix, 'shared')];
  };
  constructor(DateTimeFormat, 'DateTimeFormat', {
    formatToParts(date) {
      return decodeParts(formatDate(slot(this, 'DateTimeFormat', 'formatToParts'), date, true));
    },
    formatRange(startDate, endDate) {
      return textOf(rangeParts(slot(this, 'DateTimeFormat', 'formatRange'), startDate, endDate));
    },
    formatRangeToParts(startDate, endDate) {
      return rangeParts(slot(this, 'DateTimeFormat', 'formatRangeToParts'), startDate, endDate);
    },
    resolvedOptions() {
      const r = unwrap(this, 'resolvedOptions');
      const result = {};
      // defineProperty: a plain assignment would run setters planted on Object.prototype.
      const put = (key, value) => defineProperty(result, key,
        { value, writable: true, enumerable: true, configurable: true });
      put('locale', r.locale);
      put('calendar', r.calendar);
      put('numberingSystem', r.numberingSystem);
      put('timeZone', r.timeZone);
      if (r.hourCycle !== undefined) {
        put('hourCycle', r.hourCycle);
        put('hour12', r.hourCycle === 'h11' || r.hourCycle === 'h12');
      }
      for (const key of [...COMPONENTS.map(([name]) => name), 'fractionalSecondDigits', 'timeZoneName', 'dateStyle', 'timeStyle']) {
        if (r[key] !== undefined) put(key, r[key]);
      }
      return result;
    },
  });
  const formatGetter = {
    get format() {
      const record = unwrap(this, 'format');
      record.bound ??= (date) => formatDate(record, date, false);
      return record.bound;
    },
  };
  defineProperty(DateTimeFormat.prototype, 'format',
    { get: Object.getOwnPropertyDescriptor(formatGetter, 'format').get, configurable: true });
  define(Intl, { DateTimeFormat });

  // The Temporal toLocaleString methods (temporal/prelude_intl.js) pick this up once.
  // The styles a type cannot show make the options invalid (ToDateTimeOptions `required`).
  const REQUIRED = { PlainDate: 'date', PlainYearMonth: 'date', PlainMonthDay: 'date', PlainTime: 'time',
    PlainDateTime: 'any', Instant: 'any', ZonedDateTime: 'any' };
  defineProperty(global, '__intl_temporal', {
    value: (kind, value, locales, options) => {
      const zone = kind === 'ZonedDateTime' ? value.timeZoneId : undefined;
      const record = createDateTimeFormat(locales, options, REQUIRED[kind], zone === undefined ? 'date' : 'all', zone);
      if (zone === undefined) return renderValue(resolveValue(record, { kind, value }), false);
      checkCalendar(record, kind, value);
      return renderValue({ record, t: value.epochMilliseconds, wall: false }, false);
    },
    configurable: true,
  });

  // Locale-sensitive methods of the built-ins. Default parameters keep their `length`.
  const { NumberFormat, Collator } = Intl;
  const thisNumber = Number.prototype.valueOf;
  const BigIntPrototype = BigInt.prototype;
  const thisBigInt = BigIntPrototype.valueOf;
  const thisTime = Date.prototype.getTime;
  const call = Function.prototype.call.bind(Function.prototype.call);
  define(Number.prototype, {
    toLocaleString(locales = undefined, options = undefined) {
      return new NumberFormat(locales, options).format(call(thisNumber, this));
    },
  });
  define(BigIntPrototype, {
    toLocaleString(locales = undefined, options = undefined) {
      return new NumberFormat(locales, options).format(call(thisBigInt, this));
    },
  });
  define(String.prototype, {
    localeCompare(that, locales = undefined, options = undefined) {
      if (this === undefined || this === null) typeError('String.prototype.localeCompare called on null or undefined');
      return new Collator(locales, options).compare(String(this), String(that));
    },
  });
  const localeDateString = (self, locales, options, required, defaults) => {
    const t = call(thisTime, self);
    if (t !== t) return 'Invalid Date';
    return formatDate(createDateTimeFormat(locales, options, required, defaults), t, false);
  };
  define(Date.prototype, {
    toLocaleString(locales = undefined, options = undefined) {
      return localeDateString(this, locales, options, 'any', 'all');
    },
    toLocaleDateString(locales = undefined, options = undefined) {
      return localeDateString(this, locales, options, 'date', 'date');
    },
    toLocaleTimeString(locales = undefined, options = undefined) {
      return localeDateString(this, locales, options, 'time', 'time');
    },
  });
  // shortcut: no cycle detection, like the plain Array.prototype.toLocaleString it replaces.
  define(Array.prototype, {
    toLocaleString(locales = undefined, options = undefined) {
      const array = Object(this);
      const length = Math.min(Math.max(Math.trunc(Number(array.length)) || 0, 0), 2 ** 53 - 1);
      let result = '';
      for (let k = 0; k < length; k++) {
        if (k > 0) result += ',';
        const element = array[k];
        if (element !== undefined && element !== null) {
          result += String(element.toLocaleString(locales, options));
        }
      }
      return result;
    },
  });
})();
