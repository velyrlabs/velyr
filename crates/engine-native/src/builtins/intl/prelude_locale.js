'use strict';
// Intl.Locale (ECMA-402 14). The canonical tag lives in the WeakMap slot; the native ops
// build, split and expand it. Week info and time zones are added by prelude_enum.js.
(() => {
  const global = globalThis;
  const { ops, define, tag, slots, slot, typeOption, getOption, typeError } = global.__intl_shared;
  const { defineProperty } = Object;
  const SEPARATOR = '\u0001';
  // The fields of ops.locale_parts, by index.
  const FIELD = { base: 0, language: 1, script: 2, region: 3, variants: 4, ca: 5, co: 6, hc: 7,
    kf: 8, kn: 9, nu: 10, fw: 11, rg: 12, sd: 13 };
  const WEEKDAYS = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'];
  const RTL_SCRIPTS = ['Adlm', 'Arab', 'Armi', 'Avst', 'Chrs', 'Cprt', 'Elym', 'Hatr', 'Hebr', 'Hung',
    'Khar', 'Lydi', 'Mand', 'Mani', 'Mend', 'Merc', 'Mero', 'Narb', 'Nbat', 'Nkoo', 'Orkh', 'Ougr',
    'Palm', 'Phli', 'Phlp', 'Phnx', 'Prti', 'Rohg', 'Samr', 'Sarb', 'Sogd', 'Sogo', 'Syrc', 'Thaa',
    'Wara', 'Yezi'];
  const NO_DIRECTION = ['Brai', 'Zinh', 'Zmth', 'Zsye', 'Zsym', 'Zxxx', 'Zyyy', 'Zzzz'];
  // shortcut: small tables instead of CLDR preference data; replace once the engine links it.
  const CALENDARS = {
    TH: ['buddhist', 'gregory'], JP: ['gregory', 'japanese'], IN: ['gregory', 'indian'],
    CN: ['gregory', 'chinese'], TW: ['gregory', 'roc', 'chinese'], IL: ['gregory', 'hebrew'],
    AF: ['persian', 'gregory'], IR: ['persian', 'gregory'],
    SA: ['islamic-umalqura', 'gregory'], AE: ['gregory', 'islamic-umalqura'],
    KW: ['gregory', 'islamic-umalqura'], QA: ['gregory', 'islamic-umalqura'],
    BH: ['gregory', 'islamic-umalqura'],
  };
  const H12_REGIONS = ['AU', 'BD', 'CA', 'CO', 'EG', 'IN', 'KR', 'MX', 'MY', 'NZ', 'PH', 'PK', 'SA', 'US'];
  const H23_LANGUAGE_REGIONS = ['fr-CA', 'en-GB', 'en-IE', 'en-ZA'];
  const COLLATIONS = { de: ['phonebk'], es: ['trad'], sv: ['trad'], ja: ['unihan'], ko: ['unihan'],
    zh: ['pinyin', 'stroke', 'unihan', 'zhuyin'] };
  const NUMBERING = { ar: 'arab', fa: 'arabext', bn: 'beng', mr: 'deva', ne: 'deva', my: 'mymr' };

  const parts = (canonical) => canonical === undefined ? [] : ops.locale_parts(canonical).split(SEPARATOR);
  const field = (record, name) => {
    const value = record.parts[FIELD[name]];
    return value === '' ? undefined : value;
  };
  // A keyword getter: absent is undefined and a valueless keyword is the empty string.
  const keyword = (record, name) => {
    const value = field(record, name);
    return value === 'true' ? '' : value;
  };
  const make = (canonical) => {
    const locale = Object.create(Locale.prototype);
    slots.set(locale, { kind: 'Locale', tag: canonical, parts: parts(canonical) });
    return locale;
  };
  const weekday = (value) => (value.length === 1 && value >= '0' && value <= '7' ? WEEKDAYS[value] : value);

  // The empty string is an invalid subtag, not "unset" (the ops take "" as unset).
  const nonEmpty = (options, name) => {
    const value = getOption(options, name);
    if (value === '') throw new RangeError(`Incorrect locale information provided: ${name}`);
    return value;
  };

  function Locale(tagArg, options = undefined) {
    if (new.target === undefined) typeError("Constructor Intl.Locale requires 'new'");
    if (typeof tagArg !== 'string' && (typeof tagArg !== 'object' || tagArg === null)) {
      typeError('First argument to Intl.Locale constructor can\'t be empty or missing');
    }
    const other = typeof tagArg === 'object' ? slots.get(tagArg) : undefined;
    const text = other !== undefined && other.kind === 'Locale' ? other.tag : `${tagArg}`;
    if (options === null) typeError('Cannot convert null to object');
    options = options === undefined ? Object.create(null) : Object(options);
    ops.locale_parts(text);
    const language = nonEmpty(options, 'language');
    const script = nonEmpty(options, 'script');
    const region = nonEmpty(options, 'region');
    const variants = nonEmpty(options, 'variants');
    const calendar = typeOption(options, 'calendar');
    const collation = typeOption(options, 'collation');
    const hourCycle = getOption(options, 'hourCycle', ['h11', 'h12', 'h23', 'h24']);
    const caseFirst = getOption(options, 'caseFirst', ['upper', 'lower', 'false']);
    const numeric = getOption(options, 'numeric', Boolean);
    const numberingSystem = typeOption(options, 'numberingSystem');
    let firstDay = nonEmpty(options, 'firstDayOfWeek');
    if (firstDay !== undefined) firstDay = weekday(firstDay);
    const canonical = ops.locale_canonical(text, language ?? '', script ?? '', region ?? '',
      variants ?? '', calendar ?? '', collation ?? '', hourCycle ?? '', caseFirst ?? '',
      numeric === undefined ? '' : String(numeric), numberingSystem ?? '', firstDay ?? '');
    const self = this;
    slots.set(self, { kind: 'Locale', tag: canonical, parts: parts(canonical) });
    return self;
  }

  const getters = {
    baseName: (r) => r.parts[FIELD.base],
    language: (r) => r.parts[FIELD.language],
    script: (r) => field(r, 'script'),
    region: (r) => field(r, 'region'),
    variants: (r) => field(r, 'variants'),
    calendar: (r) => keyword(r, 'ca'),
    collation: (r) => keyword(r, 'co'),
    hourCycle: (r) => keyword(r, 'hc'),
    caseFirst: (r) => keyword(r, 'kf'),
    numeric: (r) => field(r, 'kn') === 'true',
    numberingSystem: (r) => keyword(r, 'nu'),
    firstDayOfWeek: (r) => keyword(r, 'fw'),
  };
  for (const name of Object.keys(getters)) {
    const get = { [`get ${name}`]() { return getters[name](slot(this, 'Locale', name)); } }[`get ${name}`];
    defineProperty(Locale.prototype, name, { get, configurable: true });
  }

  const likelyRegion = (record) => {
    const rg = field(record, 'rg');
    if (rg !== undefined && rg.length >= 2) return rg.slice(0, 2).toUpperCase();
    const region = field(record, 'region');
    if (region !== undefined) return region;
    const sd = field(record, 'sd');
    if (sd !== undefined && sd.length >= 2) return sd.slice(0, 2).toUpperCase();
    return parts(ops.locale_expand(record.parts[FIELD.base], 'max'))[FIELD.region] || '001';
  };
  const calendars = (r) => {
    const explicit = field(r, 'ca');
    return explicit !== undefined ? [explicit] : (CALENDARS[likelyRegion(r)] ?? ['gregory', 'iso8601']).slice();
  };
  const hourCycles = (r) => {
    const explicit = field(r, 'hc');
    if (explicit !== undefined) return [explicit];
    const language = r.parts[FIELD.language];
    const region = likelyRegion(r);
    if (H23_LANGUAGE_REGIONS.includes(`${language}-${region}`)) return ['h23'];
    const h12 = H12_REGIONS.includes(region) || (region === '001' && (language === 'en' || language === 'ar'));
    return [h12 ? 'h12' : 'h23'];
  };
  const collations = (r) => {
    const explicit = field(r, 'co');
    if (explicit === 'true') return [];
    if (explicit !== undefined) return [explicit];
    return [...(COLLATIONS[r.parts[FIELD.language]] ?? []), 'emoji', 'eor'].sort();
  };
  const numberingSystems = (r) => {
    const explicit = field(r, 'nu');
    if (explicit === 'true') return [];
    return [explicit ?? NUMBERING[r.parts[FIELD.language]] ?? 'latn'];
  };
  const textInfo = (r) => {
    const script = field(r, 'script') ?? parts(ops.locale_expand(r.parts[FIELD.base], 'max'))[FIELD.script];
    const unknown = script === undefined || NO_DIRECTION.includes(script) || (script >= 'Qaaa' && script <= 'Qabx');
    const direction = unknown ? undefined : RTL_SCRIPTS.includes(script) ? 'rtl' : 'ltr';
    return { direction };
  };
  const info = { calendars, collations, hourCycles, numberingSystems, textInfo };
  const methods = {
    maximize() { return make(ops.locale_expand(slot(this, 'Locale', 'maximize').tag, 'max')); },
    minimize() { return make(ops.locale_expand(slot(this, 'Locale', 'minimize').tag, 'min')); },
    toString() { return slot(this, 'Locale', 'toString').tag; },
  };
  for (const name of Object.keys(info)) {
    const method = `get${name[0].toUpperCase()}${name.slice(1)}`;
    methods[method] = { [method]() { return info[name](slot(this, 'Locale', method)); } }[method];
    const get = { [`get ${name}`]() { return info[name](slot(this, 'Locale', name)); } }[`get ${name}`];
    defineProperty(Locale.prototype, name, { get, configurable: true });
  }
  define(Locale.prototype, methods);
  tag(Locale.prototype, 'Intl.Locale');
  defineProperty(Locale, 'prototype', { writable: false });
  define(global.Intl, { Locale });
  // Read by prelude_enum.js; removed by the cleanup prelude.
  defineProperty(global, '__intl_locale_shared', {
    value: Object.freeze({ likelyRegion, field }), configurable: true,
  });
})();
