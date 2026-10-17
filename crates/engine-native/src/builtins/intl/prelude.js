'use strict';
// ECMA-402 objects over the `__intl_*` native ops (see mod.rs). Run once per realm;
// `prelude_formats.js` (DateTimeFormat and the built-ins' toLocale* methods) runs next
// and picks up the shared helpers from `__intl_shared`.
(() => {
  const global = globalThis;
  const ops = {};
  for (const name of Object.getOwnPropertyNames(global)) {
    if (name.startsWith('__intl_')) {
      ops[name.slice(7)] = global[name];
      delete global[name];
    }
  }
  const { defineProperty, freeze } = Object;
  const typeError = (message) => { throw new TypeError(message); };
  const define = (target, methods) => {
    for (const name of Reflect.ownKeys(methods)) {
      defineProperty(target, name, { value: methods[name], writable: true, configurable: true });
    }
  };
  const tag = (target, name) =>
    defineProperty(target, Symbol.toStringTag, { value: name, configurable: true });

  const toObject = (value) => {
    if (value === undefined || value === null) typeError('Cannot convert undefined or null to object');
    return Object(value);
  };
  // CanonicalizeLocaleList.
  const canonicalLocales = (locales) => {
    if (locales === undefined) return [];
    const list = typeof locales === 'string' ? [locales] : toObject(locales);
    // Unary plus is ToNumber: it throws on Symbol and BigInt, as ToLength must.
    const length = Math.min(Math.max(Math.trunc(+list.length) || 0, 0), 2 ** 32);
    const seen = [];
    for (let k = 0; k < length; k++) {
      if (!(k in list)) continue;
      const value = list[k];
      if (typeof value !== 'string' && (typeof value !== 'object' || value === null)) {
        typeError('Language ID should be string or object.');
      }
      const canonical = ops.canonicalize(String(value));
      if (!seen.includes(canonical)) seen[seen.length] = canonical;
    }
    return seen;
  };
  // No RegExp in preludes: a regexp exec would overwrite the RegExp legacy statics ($1...).
  // Splits a tag around its -u- extension: { before, keywords, after } or null.
  const splitUnicodeExtension = (locale) => {
    const parts = locale.split('-');
    const start = parts.indexOf('u', 1);
    if (start < 0) return null;
    let end = start + 1;
    while (end < parts.length && parts[end].length >= 2) end++;
    if (end === start + 1) return null;
    return { before: parts.slice(0, start), keywords: parts.slice(start + 1, end), after: parts.slice(end) };
  };
  const withoutExtensions = (locale) => {
    const split = splitUnicodeExtension(locale);
    return split === null ? locale : split.before.concat(split.after).join('-');
  };
  // BestAvailableLocale: the longest supported prefix of `locale`.
  const bestAvailable = (locale) => {
    let candidate = withoutExtensions(locale);
    for (;;) {
      if (ops.supported(candidate)) return candidate;
      let cut = candidate.lastIndexOf('-');
      if (cut < 0) return undefined;
      if (cut >= 2 && candidate[cut - 2] === '-') cut -= 2;
      candidate = candidate.slice(0, cut);
    }
  };
  // Values of the -u- keywords the formatters resolve (ECMA-402 relevantExtensionKeys).
  const KEYWORD_VALUES = {
    nu: ['adlm', 'arab', 'arabext', 'beng', 'cakm', 'deva', 'fullwide', 'gujr', 'guru', 'hanidec',
      'hmnp', 'java', 'khmr', 'knda', 'laoo', 'latn', 'limb', 'mlym', 'mong', 'mtei', 'mymr',
      'nkoo', 'olck', 'orya', 'tamldec', 'telu', 'thai', 'tibt', 'vaii'],
    ca: ['buddhist', 'chinese', 'coptic', 'dangi', 'ethioaa', 'ethiopic', 'gregory', 'hebrew', 'indian',
      'islamic-civil', 'islamic-tbla', 'islamic-umalqura', 'iso8601', 'japanese', 'persian', 'roc'],
    hc: ['h11', 'h12', 'h23', 'h24'],
  };
  // The keywords of the -u- extension of a canonical tag: { ca: 'buddhist', nu: 'arab' }.
  const unicodeKeywords = (locale) => {
    const keywords = {};
    const split = splitUnicodeExtension(locale);
    if (split === null) return keywords;
    let key;
    for (const subtag of split.keywords) {
      if (subtag.length === 2) { key = subtag; keywords[key] = ''; }
      else if (key !== undefined) keywords[key] = keywords[key] === '' ? subtag : `${keywords[key]}-${subtag}`;
    }
    return keywords;
  };
  // ResolveLocale with lookup matching (best fit resolves the same: every CLDR locale is
  // available). `fromOptions` maps each relevant keyword ('nu', 'ca', 'hc') to the value of
  // its option or undefined. Returns the resolved `locale` (keywords kept only when taken
  // from the tag), `data` (the tag with every effective keyword, for the native ops) and
  // the effective `values`.
  const resolveLocaleKeys = (locales, fromOptions) => {
    let base = ops.default_locale();
    let keywords = {};
    for (const requested of canonicalLocales(locales)) {
      const found = bestAvailable(requested);
      if (found === undefined) continue;
      base = found;
      keywords = unicodeKeywords(requested);
      break;
    }
    const values = {};
    const kept = {};
    for (const key of Object.keys(fromOptions).sort()) {
      const supported = (value) => value !== undefined && KEYWORD_VALUES[key].includes(value);
      if (supported(keywords[key])) { values[key] = keywords[key]; kept[key] = keywords[key]; }
      const option = fromOptions[key];
      if (supported(option) && option !== values[key]) { values[key] = option; delete kept[key]; }
    }
    const tag = (map) => {
      const keys = Object.keys(map);
      return keys.length === 0 ? base : `${base}-u-${keys.map((key) => `${key}-${map[key]}`).join('-')}`;
    };
    return { locale: tag(kept), data: tag(values), values };
  };
  const resolveLocale = (locales) => resolveLocaleKeys(locales, {}).locale;
  const isAsciiAlpha = (c) => (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
  const isAlphanumeric = (text) => [...text].every((c) => isAsciiAlpha(c) || (c >= '0' && c <= '9'));
  const isUnicodeType = (text) => text.split('-').every((part) => part.length >= 3 && part.length <= 8 && isAlphanumeric(part));
  // A string option holding a Unicode type (numberingSystem, calendar), lowercased.
  const typeOption = (options, name) => {
    const value = getOption(options, name);
    if (value !== undefined && !isUnicodeType(value)) {
      throw new RangeError(`Invalid ${name} : ${value}`);
    }
    return value === undefined ? value : value.toLowerCase();
  };
  const supportedLocalesOf = (locales, options) => {
    const requested = canonicalLocales(locales);
    if (options !== undefined) getOption(toObject(options), 'localeMatcher', ['lookup', 'best fit']);
    return requested.filter((locale) => bestAvailable(locale) !== undefined);
  };
  const toOptions = (options) => (options === undefined ? Object.create(null) : Object(options));
  const getOption = (options, name, allowed, fallback) => {
    let value = options[name];
    if (value === undefined) return fallback;
    if (allowed === Boolean) return Boolean(value);
    value = `${value}`; // ToString: throws on Symbol, unlike String()
    if (allowed && !allowed.includes(value)) {
      throw new RangeError(`Value ${value} out of range for Intl options property ${name}`);
    }
    return value;
  };
  const getNumber = (options, name, min, max, fallback) => {
    const value = options[name];
    if (value === undefined) return fallback;
    const n = Number(value);
    if (!(n >= min && n <= max)) throw new RangeError(`${name} value is out of range.`);
    return Math.floor(n);
  };
  const decodeParts = (encoded) => {
    const parts = [];
    for (const part of encoded.split('\u0002')) {
      const cut = part.indexOf('\u0001');
      const value = part.slice(cut + 1);
      if (cut > 0 && value !== '') parts.push({ type: part.slice(0, cut), value });
    }
    return parts;
  };
  // Internal slots, by object; `slot` is the brand check.
  const slots = new WeakMap();
  const slot = (object, kind, method) => {
    const record = typeof object === 'object' && object !== null ? slots.get(object) : undefined;
    if (record === undefined || record.kind !== kind) {
      typeError(`Method Intl.${kind}.prototype.${method} called on incompatible receiver`);
    }
    return record;
  };
  // A getter returning a function bound to the instance (format, compare).
  const boundGetter = (proto, name, kind, make) => {
    const getter = {
      [`get ${name}`]() {
        const record = slot(this, kind, name);
        if (record.bound === undefined) record.bound = make(record);
        return record.bound;
      },
    }[`get ${name}`];
    defineProperty(proto, name, { get: getter, configurable: true });
  };
  const constructor = (C, kind, methods) => {
    define(C.prototype, methods);
    tag(C.prototype, `Intl.${kind}`);
    defineProperty(C, 'prototype', { writable: false });
    define(C, {
      supportedLocalesOf(locales, options = undefined) { return supportedLocalesOf(locales, options); },
    });
  };
  // The `this` of a constructor that may be called without `new`.
  const instance = (target, self, C) =>
    target === undefined ? Object.create(C.prototype) : self;

  // Digit options of NumberFormat and PluralRules.
  const digitOptions = (options, defaultMax) => {
    const minimumIntegerDigits = getNumber(options, 'minimumIntegerDigits', 1, 21, 1);
    const minFraction = options.minimumFractionDigits;
    const maxFraction = options.maximumFractionDigits;
    const minimumFractionDigits = getNumber(options, 'minimumFractionDigits', 0, 100, undefined);
    const maximumFractionDigits = getNumber(options, 'maximumFractionDigits', 0, 100, undefined);
    const min = minFraction === undefined ? Math.min(0, maximumFractionDigits ?? 0) : minimumFractionDigits;
    const max = maxFraction === undefined ? Math.max(min, defaultMax) : maximumFractionDigits;
    if (min > max) throw new RangeError('maximumFractionDigits value is out of range.');
    return { minimumIntegerDigits, minimumFractionDigits: min, maximumFractionDigits: max };
  };

  function PluralRules(locales = undefined, options = undefined) {
    if (new.target === undefined) typeError("Constructor Intl.PluralRules requires 'new'");
    options = toOptions(options);
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const locale = resolveLocale(locales);
    const type = getOption(options, 'type', ['cardinal', 'ordinal'], 'cardinal');
    slots.set(this, { kind: 'PluralRules', locale, type, ...digitOptions(options, 3) });
  }
  constructor(PluralRules, 'PluralRules', {
    select(value) {
      const r = slot(this, 'PluralRules', 'select');
      return ops.plural_select(r.locale, r.type === 'ordinal', Number(value),
        r.minimumFractionDigits, r.maximumFractionDigits);
    },
    resolvedOptions() {
      const r = slot(this, 'PluralRules', 'resolvedOptions');
      return {
        locale: r.locale, type: r.type, minimumIntegerDigits: r.minimumIntegerDigits,
        minimumFractionDigits: r.minimumFractionDigits, maximumFractionDigits: r.maximumFractionDigits,
        pluralCategories: ops.plural_categories(r.locale, r.type === 'ordinal').split(','),
        roundingMode: 'halfExpand',
      };
    },
  });

  function Collator(locales = undefined, options = undefined) {
    const self = instance(new.target, this, Collator);
    options = toOptions(options);
    const usage = getOption(options, 'usage', ['sort', 'search'], 'sort');
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const numeric = getOption(options, 'numeric', Boolean, false);
    const caseFirst = getOption(options, 'caseFirst', ['upper', 'lower', 'false'], 'false');
    const locale = resolveLocale(locales);
    const sensitivity = getOption(options, 'sensitivity', ['base', 'accent', 'case', 'variant'], 'variant');
    const ignorePunctuation = getOption(options, 'ignorePunctuation', Boolean, false);
    slots.set(self, { kind: 'Collator', locale, usage, numeric, caseFirst, sensitivity, ignorePunctuation });
    return self;
  }
  constructor(Collator, 'Collator', {
    resolvedOptions() {
      const r = slot(this, 'Collator', 'resolvedOptions');
      return {
        locale: r.locale, usage: r.usage, sensitivity: r.sensitivity,
        ignorePunctuation: r.ignorePunctuation, collation: 'default', numeric: r.numeric,
        caseFirst: r.caseFirst,
      };
    },
  });
  const compare = (r, x, y) => ops.collator_compare(r.locale, r.sensitivity, r.numeric,
    r.caseFirst, r.ignorePunctuation, String(x), String(y));
  boundGetter(Collator.prototype, 'compare', 'Collator', (r) => (x, y) => compare(r, x, y));

  function Segmenter(locales = undefined, options = undefined) {
    if (new.target === undefined) typeError("Constructor Intl.Segmenter requires 'new'");
    options = toOptions(options);
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const locale = resolveLocale(locales);
    const granularity = getOption(options, 'granularity', ['grapheme', 'word', 'sentence'], 'grapheme');
    slots.set(this, { kind: 'Segmenter', locale, granularity });
  }
  const SegmentsPrototype = {};
  const segmentData = (segments, index) => {
    const { input, boundaries, granularity } = segments;
    let i = boundaries.length - 1;
    while (i > 0 && boundaries[i].start > index) i--;
    const start = boundaries[i].start;
    const end = i + 1 < boundaries.length ? boundaries[i + 1].start : input.length;
    const data = { segment: input.slice(start, end), index: start, input };
    if (granularity === 'word') data.isWordLike = boundaries[i].wordLike;
    return data;
  };
  define(SegmentsPrototype, {
    containing(index = undefined) {
      const segments = slot(this, 'Segments', 'containing');
      const n = Math.trunc(Number(index)) || 0;
      if (n < 0 || n >= segments.input.length) return undefined;
      return segmentData(segments, n);
    },
    *[Symbol.iterator]() {
      const segments = slot(this, 'Segments', '@@iterator');
      for (const { start } of segments.boundaries) yield segmentData(segments, start);
    },
  });
  constructor(Segmenter, 'Segmenter', {
    segment(string) {
      const r = slot(this, 'Segmenter', 'segment');
      const input = String(string);
      const encoded = ops.segment(r.granularity, input);
      const boundaries = encoded === '' ? [] : encoded.split(';').map((entry) => {
        const [start, wordLike] = entry.split(':');
        return { start: Number(start), wordLike: wordLike === '1' };
      });
      const segments = Object.create(SegmentsPrototype);
      slots.set(segments, { kind: 'Segments', input, boundaries, granularity: r.granularity });
      return segments;
    },
    resolvedOptions() {
      const r = slot(this, 'Segmenter', 'resolvedOptions');
      return { locale: r.locale, granularity: r.granularity };
    },
  });

  const Intl = global.Intl;
  define(Intl, {
    getCanonicalLocales(locales) { return canonicalLocales(locales); },
    Collator, PluralRules, Segmenter,
  });
  tag(Intl, 'Intl');
  defineProperty(global, '__intl_shared', {
    value: freeze({ ops, define, tag, slots, slot, boundGetter, constructor, instance,
      resolveLocale, resolveLocaleKeys, KEYWORD_VALUES, typeOption, isAsciiAlpha, toOptions, getOption, getNumber, decodeParts,
      typeError }),
    configurable: true,
  });
})();
