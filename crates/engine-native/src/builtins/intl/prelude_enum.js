'use strict';
// Intl.supportedValuesOf and the week info and time zone members of Intl.Locale
// (ECMA-402 Intl-enumeration, Intl.Locale-info). Calendars and numbering systems come from
// the tables the constructors validate against; the other lists come from the native ops.
(() => {
  const global = globalThis;
  const { ops, define, slot, KEYWORD_VALUES } = global.__intl_shared;
  const { likelyRegion, field } = global.__intl_locale_shared;
  const { defineProperty } = Object;
  const SEPARATOR = '\u0001';
  const FIRST_DAY = { mon: 1, tue: 2, wed: 3, thu: 4, fri: 5, sat: 6, sun: 7 };
  const list = (text) => (text === '' ? [] : text.split(SEPARATOR));

  const sources = {
    calendar: () => KEYWORD_VALUES.ca.slice(),
    numberingSystem: () => KEYWORD_VALUES.nu.slice(),
    // shortcut: Collator only resolves the default collation, so there is nothing to list.
    collation: () => [],
    currency: () => list(ops.supported_values('currency')),
    timeZone: () => list(ops.supported_values('timeZone')),
    unit: () => list(ops.supported_values('unit')),
  };
  define(global.Intl, {
    supportedValuesOf(key) {
      key = `${key}`;
      if (!Object.hasOwn(sources, key)) throw new RangeError(`Invalid key : ${key}`);
      return sources[key]().sort();
    },
  });

  const weekInfo = (record) => {
    const [first, weekend] = (ops.locale_week(likelyRegion(record)) || ops.locale_week('001')).split(SEPARATOR);
    return {
      firstDay: FIRST_DAY[field(record, 'fw')] ?? Number(first),
      weekend: weekend === '' ? [] : weekend.split(',').map(Number),
    };
  };
  const timeZones = (record) => {
    const region = field(record, 'region');
    return region === undefined ? undefined : list(ops.locale_zones(region));
  };
  const info = { weekInfo, timeZones };
  const Locale = global.Intl.Locale;
  const methods = {};
  for (const name of Object.keys(info)) {
    const method = `get${name[0].toUpperCase()}${name.slice(1)}`;
    methods[method] = { [method]() { return info[name](slot(this, 'Locale', method)); } }[method];
    const get = { [`get ${name}`]() { return info[name](slot(this, 'Locale', name)); } }[`get ${name}`];
    defineProperty(Locale.prototype, name, { get, configurable: true });
  }
  define(Locale.prototype, methods);
})();
