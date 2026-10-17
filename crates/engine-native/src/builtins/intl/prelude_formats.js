'use strict';
// Intl.DateTimeFormat and the locale-sensitive methods of the built-ins, which delegate
// to Intl (ECMA-402 chapters 19-21). Runs after prelude.js.
(() => {
  const global = globalThis;
  const shared = global.__intl_shared;
  const { ops, define, slots, slot, boundGetter, constructor, instance, resolveLocaleKeys, typeOption,
    toOptions, getOption, getNumber, decodeParts, typeError } = shared;
  const { defineProperty } = Object;
  const Intl = global.Intl;

  const DATE_FIELDS = ['weekday', 'year', 'month', 'day'];
  const TIME_FIELDS = ['dayPeriod', 'hour', 'minute', 'second', 'fractionalSecondDigits'];
  // The order `datetime.rs` reads the spec in.
  const SPEC_KEYS = ['dateStyle', 'timeStyle', 'weekday', 'era', 'year', 'month', 'day',
    'hour', 'minute', 'second', 'fractionalSecondDigits', 'hourCycle', 'timeZoneName'];
  const TEXT = ['narrow', 'short', 'long'];
  const NUMERIC = ['2-digit', 'numeric'];
  const COMPONENTS = [
    ['weekday', TEXT], ['era', TEXT], ['year', NUMERIC], ['month', [...NUMERIC, ...TEXT]],
    ['day', NUMERIC], ['dayPeriod', TEXT], ['hour', NUMERIC], ['minute', NUMERIC],
    ['second', NUMERIC],
  ];
  const STYLES = ['full', 'long', 'medium', 'short'];

  // ToDateTimeOptions, kept for the toLocale*String methods: `required` is
  // 'date' | 'time' | 'any', `defaults` is 'date' | 'time' | 'all'.
  const toDateTimeOptions = (options, required, defaults) => {
    const result = Object.create(options === undefined ? null : Object(options));
    let needDefaults = true;
    if (required !== 'time' && DATE_FIELDS.some((f) => result[f] !== undefined)) needDefaults = false;
    if (required !== 'date' && TIME_FIELDS.some((f) => result[f] !== undefined)) needDefaults = false;
    const dateStyle = result.dateStyle;
    const timeStyle = result.timeStyle;
    if (dateStyle !== undefined || timeStyle !== undefined) needDefaults = false;
    if (required === 'date' && timeStyle !== undefined) typeError('Invalid option : timeStyle');
    if (required === 'time' && dateStyle !== undefined) typeError('Invalid option : dateStyle');
    const fill = (fields) => {
      for (const field of fields) defineProperty(result, field, { value: 'numeric', writable: true, enumerable: true, configurable: true });
    };
    if (needDefaults && (defaults === 'date' || defaults === 'all')) fill(['year', 'month', 'day']);
    if (needDefaults && (defaults === 'time' || defaults === 'all')) fill(['hour', 'minute', 'second']);
    return result;
  };

  function DateTimeFormat(locales = undefined, options = undefined) {
    const self = instance(new.target, this, DateTimeFormat);
    const record = createDateTimeFormat(locales, options, 'any', 'date');
    slots.set(self, record);
    return self;
  }
  const createDateTimeFormat = (locales, options, required, defaults) => {
    options = toDateTimeOptions(options === undefined ? undefined : toOptions(options), required, defaults);
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const calendar = typeOption(options, 'calendar');
    const numberingSystem = typeOption(options, 'numberingSystem');
    const hour12 = options.hour12;
    let hourCycle = getOption(options, 'hourCycle', ['h11', 'h12', 'h23', 'h24']);
    if (hour12 !== undefined) hourCycle = hour12 ? 'h12' : 'h23';
    const { locale, data, values } = resolveLocaleKeys(locales, { ca: calendar, nu: numberingSystem, hc: hourCycle });
    hourCycle ??= values.hc;
    let timeZone = options.timeZone;
    if (timeZone === undefined) {
      timeZone = ops.default_zone();
    } else {
      const name = String(timeZone);
      timeZone = ops.canonical_zone(name);
      if (timeZone === undefined) throw new RangeError(`Invalid time zone specified: ${name}`);
    }
    const record = { kind: 'DateTimeFormat', locale, data, calendar: values.ca ?? 'gregory',
      numberingSystem: values.nu ?? 'latn', timeZone, hourCycle };
    let explicit;
    for (const [name, values] of COMPONENTS) {
      const value = getOption(options, name, values);
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
      record.dateStyle = dateStyle;
      record.timeStyle = timeStyle;
    }
    const hasHour = record.hour !== undefined || timeStyle !== undefined;
    if (!hasHour) record.hourCycle = undefined;
    record.spec = SPEC_KEYS.map((key) => (record[key] === undefined ? '' : String(record[key]))).join(',');
    return record;
  };
  const formatDate = (record, date, parts) => {
    let t = date === undefined ? Date.now() : Number(date);
    t = Math.abs(t) > 8.64e15 ? NaN : Math.trunc(t) + 0;
    if (t !== t) throw new RangeError('Invalid time value');
    return ops.date_time_format(record.data, record.spec, t, record.timeZone, parts);
  };
  constructor(DateTimeFormat, 'DateTimeFormat', {
    formatToParts(date) {
      return decodeParts(formatDate(slot(this, 'DateTimeFormat', 'formatToParts'), date, true));
    },
    resolvedOptions() {
      const r = slot(this, 'DateTimeFormat', 'resolvedOptions');
      const result = { locale: r.locale, calendar: r.calendar, numberingSystem: r.numberingSystem, timeZone: r.timeZone };
      if (r.hourCycle !== undefined) {
        result.hourCycle = r.hourCycle;
        result.hour12 = r.hourCycle === 'h11' || r.hourCycle === 'h12';
      }
      for (const key of [...COMPONENTS.map(([name]) => name), 'fractionalSecondDigits', 'timeZoneName', 'dateStyle', 'timeStyle']) {
        if (r[key] !== undefined) result[key] = r[key];
      }
      return result;
    },
  });
  boundGetter(DateTimeFormat.prototype, 'format', 'DateTimeFormat',
    (record) => (date = undefined) => formatDate(record, date, false));
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
