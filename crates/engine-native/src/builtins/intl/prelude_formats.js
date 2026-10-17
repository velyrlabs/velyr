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
  const createDateTimeFormat = (locales, options, required, defaults) => {
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
    if (timeZone === undefined) {
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
    }
    if (record.hour !== undefined || timeStyle !== undefined) {
      const base = data.split('-u-')[0];
      record.hourCycle = resolveHourCycle(base, hour12, values.hc ?? hourCycleOption);
    }
    record.spec = specOf(record);
    return record;
  };
  const timeClip = (value) => {
    const t = Math.abs(value) > 8.64e15 ? NaN : Math.trunc(value) + 0;
    if (t !== t) throw new RangeError('Invalid time value');
    return t;
  };
  const formatDate = (record, date, parts) => {
    const t = timeClip(date === undefined ? Date.now() : Number(date));
    return ops.date_time_format(record.data, record.spec, t, record.timeZone, parts);
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
    const [a, b] = [Number(startDate), Number(endDate)];
    const [start, end] = [timeClip(a), timeClip(b)].map((t) => decodeParts(
      ops.date_time_format(record.data, record.spec, t, record.timeZone, true)));
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
