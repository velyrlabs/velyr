'use strict';
// Intl.NumberFormat (ECMA-402 chapter 15) over `__intl_number_format`. Runs after
// prelude.js and picks up its shared helpers from `__intl_shared`.
// shortcut: no unit style and no compact notation (they throw): ICU4X keeps that data in
// icu_experimental, which engine-native does not depend on yet.
(() => {
  const global = globalThis;
  const { ops, define, slots, slot, boundGetter, constructor, instance, resolveLocale, resolveLocaleKeys,
    typeOption, isAsciiAlpha, toOptions, getOption, getNumber, decodeParts, typeError } = global.__intl_shared;
  const Intl = global.Intl;

  // The order `number/digits.rs` reads the spec in.
  const SPEC_KEYS = ['minimumIntegerDigits', 'minimumFractionDigits', 'maximumFractionDigits',
    'minimumSignificantDigits', 'maximumSignificantDigits', 'useGrouping', 'style', 'currency',
    'currencyDisplay', 'currencySign', 'notation', 'signDisplay', 'roundingMode', 'roundingIncrement',
    'roundingPriority', 'trailingZeroDisplay'];
  const specOf = (record) => SPEC_KEYS.map((key) => (record[key] === undefined ? '' : String(record[key]))).join(',');

  const ROUNDING_MODES = ['ceil', 'floor', 'expand', 'trunc', 'halfCeil', 'halfFloor', 'halfExpand',
    'halfTrunc', 'halfEven'];
  const ROUNDING_INCREMENTS = [1, 2, 5, 10, 20, 25, 50, 100, 200, 250, 500, 1000, 2000, 2500, 5000];

  const defaultNumber = (value, min, max, fallback, name) => {
    if (value === undefined) return fallback;
    const n = Number(value);
    if (!(n >= min && n <= max)) throw new RangeError(`${name} value is out of range.`);
    return Math.floor(n);
  };

  // SetNumberFormatDigitOptions (ECMA-402 v3): the digit, rounding and trailing zero options.
  const digitOptions = (options, mnfdDefault, mxfdDefault, notation) => {
    const minimumIntegerDigits = getNumber(options, 'minimumIntegerDigits', 1, 21, 1);
    const { minimumFractionDigits: mnfdRaw, maximumFractionDigits: mxfdRaw,
      minimumSignificantDigits: mnsdRaw, maximumSignificantDigits: mxsdRaw } = options;
    const roundingIncrement = getNumber(options, 'roundingIncrement', 1, 5000, 1);
    if (!ROUNDING_INCREMENTS.includes(roundingIncrement)) throw new RangeError('roundingIncrement value is out of range.');
    const roundingMode = getOption(options, 'roundingMode', ROUNDING_MODES, 'halfExpand');
    let roundingPriority = getOption(options, 'roundingPriority', ['auto', 'morePrecision', 'lessPrecision'], 'auto');
    const trailingZeroDisplay = getOption(options, 'trailingZeroDisplay', ['auto', 'stripIfInteger'], 'auto');
    if (roundingIncrement !== 1) mxfdDefault = mnfdDefault;
    const hasSd = mnsdRaw !== undefined || mxsdRaw !== undefined;
    const hasFd = mnfdRaw !== undefined || mxfdRaw !== undefined;
    let needSd = true;
    let needFd = true;
    if (roundingPriority === 'auto') {
      needSd = hasSd;
      if (needSd || (!hasFd && notation === 'compact')) needFd = false;
    }
    const result = { minimumIntegerDigits };
    if (needSd) {
      const min = hasSd ? defaultNumber(mnsdRaw, 1, 21, 1, 'minimumSignificantDigits') : 1;
      const max = hasSd ? defaultNumber(mxsdRaw, min, 21, 21, 'maximumSignificantDigits') : 21;
      Object.assign(result, { minimumSignificantDigits: min, maximumSignificantDigits: max });
    }
    if (needFd) {
      let min = mnfdDefault;
      let max = mxfdDefault;
      if (hasFd) {
        min = defaultNumber(mnfdRaw, 0, 100, undefined, 'minimumFractionDigits');
        max = defaultNumber(mxfdRaw, 0, 100, undefined, 'maximumFractionDigits');
        if (min === undefined) min = Math.min(mnfdDefault, max);
        else if (max === undefined) max = Math.max(mxfdDefault, min);
        else if (min > max) throw new RangeError('maximumFractionDigits value is out of range.');
      }
      Object.assign(result, { minimumFractionDigits: min, maximumFractionDigits: max });
    }
    if (!needSd && !needFd) {
      Object.assign(result, { minimumFractionDigits: 0, maximumFractionDigits: 0,
        minimumSignificantDigits: 1, maximumSignificantDigits: 2 });
      roundingPriority = 'morePrecision';
    }
    if (roundingIncrement !== 1) {
      if (result.maximumSignificantDigits !== undefined) typeError('roundingIncrement requires fraction digit rounding');
      if (result.maximumFractionDigits !== result.minimumFractionDigits) {
        throw new RangeError('maximumFractionDigits must equal minimumFractionDigits with roundingIncrement');
      }
    }
    return Object.assign(result, { roundingIncrement, roundingMode, roundingPriority, trailingZeroDisplay });
  };
  const DIGIT_KEYS = ['minimumIntegerDigits', 'minimumFractionDigits', 'maximumFractionDigits',
    'minimumSignificantDigits', 'maximumSignificantDigits'];
  const ROUNDING_KEYS = ['roundingIncrement', 'roundingMode', 'roundingPriority', 'trailingZeroDisplay'];

  // GetBooleanOrStringNumberFormatOption for useGrouping.
  const groupingOption = (options, fallback) => {
    const value = options.useGrouping;
    if (value === undefined) return fallback;
    if (value === true) return 'always';
    if (!value) return false;
    const text = getOption(options, 'useGrouping', ['min2', 'auto', 'always', 'true', 'false'], fallback);
    return text === 'true' || text === 'false' ? fallback : text;
  };

  function NumberFormat(locales = undefined, options = undefined) {
    const self = instance(new.target, this, NumberFormat);
    options = toOptions(options);
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const numberingSystem = typeOption(options, 'numberingSystem');
    const { locale, data, values } = resolveLocaleKeys(locales, { nu: numberingSystem });
    const style = getOption(options, 'style', ['decimal', 'percent', 'currency', 'unit'], 'decimal');
    let currency = getOption(options, 'currency');
    if (currency !== undefined && (currency.length !== 3 || ![...currency].every(isAsciiAlpha))) {
      throw new RangeError(`Invalid currency code : ${currency}`);
    }
    if (style === 'currency' && currency === undefined) typeError('Currency code is required with currency style.');
    const currencyDisplay = getOption(options, 'currencyDisplay', ['code', 'symbol', 'narrowSymbol', 'name'], 'symbol');
    const currencySign = getOption(options, 'currencySign', ['standard', 'accounting'], 'standard');
    // N44 plugs the unit table in here; the reads keep the spec order meanwhile.
    const unit = getOption(options, 'unit');
    getOption(options, 'unitDisplay', ['short', 'narrow', 'long'], 'short');
    if (style === 'unit' && unit === undefined) typeError('Unit is required with unit style.');
    const record = { kind: 'NumberFormat', locale, data, numberingSystem: values.nu ?? 'latn', style };
    let fractionDefault = { min: 0, max: style === 'percent' ? 0 : 3 };
    if (style === 'currency') {
      currency = currency.toUpperCase();
      Object.assign(record, { currency, currencyDisplay, currencySign });
      if (currencyDisplay === 'name') {
        throw new RangeError('Intl.NumberFormat currencyDisplay "name" is not supported yet');
      }
      const digits = ops.currency_digits(currency);
      fractionDefault = { min: digits, max: digits };
    }
    if (style === 'unit') throw new RangeError('Intl.NumberFormat style "unit" is not supported yet');
    record.notation = getOption(options, 'notation', ['standard', 'scientific', 'engineering', 'compact'], 'standard');
    if (record.notation === 'compact') throw new RangeError('Intl.NumberFormat notation "compact" is not supported yet');
    Object.assign(record, digitOptions(options, fractionDefault.min, fractionDefault.max, record.notation));
    getOption(options, 'compactDisplay', ['short', 'long'], 'short');
    record.useGrouping = groupingOption(options, 'auto');
    record.signDisplay = getOption(options, 'signDisplay', ['auto', 'never', 'always', 'exceptZero', 'negative'], 'auto');
    record.spec = specOf(record);
    slots.set(self, record);
    return self;
  }
  // ToIntlMathematicalValue: a Number, or the exact decimal text of a BigInt or numeric string.
  const DECIMAL_TEXT = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/;
  const toPrimitive = (value) => {
    if (typeof value !== 'object' || value === null) return value;
    const exotic = value[Symbol.toPrimitive];
    if (exotic !== undefined && exotic !== null) {
      const result = exotic.call(value, 'number');
      if (typeof result !== 'object' || result === null) return result;
      typeError('Cannot convert object to primitive value');
    }
    for (const name of ['valueOf', 'toString']) {
      const method = value[name];
      if (typeof method === 'function') {
        const result = method.call(value);
        if (typeof result !== 'object' || result === null) return result;
      }
    }
    return typeError('Cannot convert object to primitive value');
  };
  const toNumeric = (value) => {
    const primitive = toPrimitive(value);
    if (typeof primitive === 'bigint') return String(primitive);
    if (typeof primitive === 'string') {
      const text = primitive.trim();
      return DECIMAL_TEXT.test(text) ? text.replace(/^\+/, '') : Number(text);
    }
    return Number(primitive);
  };
  const formatNumber = (record, value, parts) => ops.number_format(record.data, value, record.spec, parts);
  const RESOLVED_KEYS = ['minimumIntegerDigits', 'minimumFractionDigits', 'maximumFractionDigits',
    'minimumSignificantDigits', 'maximumSignificantDigits', 'useGrouping', 'notation', 'signDisplay'];

  // formatRange: the separator between the two numbers. shortcut: CLDR range patterns only
  // for en (default) and pt-PT, and `~` as the approximately sign everywhere; upgrade when
  // icu4x ships range data.
  const RANGE_SEPARATORS = { 'pt-PT': ' - ' };
  const NUMBER_PART_TYPES = ['integer', 'group', 'decimal', 'fraction', 'exponentSeparator',
    'exponentMinusSign', 'exponentInteger', 'nan', 'infinity'];
  const withSource = (parts, source) => parts.map(({ type, value }) => ({ type, value, source }));
  const joinText = (parts) => parts.map((part) => part.value).join('');
  const splitAffixes = (parts) => {
    const first = parts.findIndex((part) => NUMBER_PART_TYPES.includes(part.type));
    const last = parts.findLastIndex((part) => NUMBER_PART_TYPES.includes(part.type));
    return { prefix: parts.slice(0, first), number: parts.slice(first, last + 1), suffix: parts.slice(last + 1) };
  };
  const rangeParts = (record, x, y) => {
    if (Number.isNaN(x) || Number.isNaN(y)) throw new RangeError('Invalid number range');
    const start = decodeParts(formatNumber(record, x, true));
    const end = decodeParts(formatNumber(record, y, true));
    if (joinText(start) === joinText(end)) {
      const lead = start[0]?.type === 'plusSign' ? 1 : 0;
      const approximately = { type: 'approximatelySign', value: '~' };
      return withSource([approximately, ...start.slice(lead)], 'shared');
    }
    const a = splitAffixes(start);
    const b = splitAffixes(end);
    const affixes = joinText(a.prefix) + joinText(a.suffix);
    const same = affixes === joinText(b.prefix) + joinText(b.suffix);
    // ICU collapses identical affixes only when they are longer than one code point.
    const collapse = same && [...affixes].length > 1;
    const repeated = !collapse && affixes + joinText(b.prefix) + joinText(b.suffix) !== '';
    let separator = RANGE_SEPARATORS[record.locale] ?? '–';
    if (repeated && !separator.startsWith(' ')) separator = ` ${separator} `;
    const middle = { type: 'literal', value: separator, source: 'shared' };
    if (!collapse) return [...withSource(start, 'startRange'), middle, ...withSource(end, 'endRange')];
    return [...withSource(a.prefix, 'shared'), ...withSource(a.number, 'startRange'), middle,
      ...withSource(b.number, 'endRange'), ...withSource(a.suffix, 'shared')];
  };
  const rangeOperands = (start, end) => {
    if (start === undefined || end === undefined) typeError('formatRange requires start and end');
    const x = toNumeric(start);
    return [x, toNumeric(end)];
  };
  constructor(NumberFormat, 'NumberFormat', {
    formatToParts(value) {
      return decodeParts(formatNumber(slot(this, 'NumberFormat', 'formatToParts'), toNumeric(value), true));
    },
    formatRange(start, end) {
      const record = slot(this, 'NumberFormat', 'formatRange');
      return joinText(rangeParts(record, ...rangeOperands(start, end)));
    },
    formatRangeToParts(start, end) {
      const record = slot(this, 'NumberFormat', 'formatRangeToParts');
      return rangeParts(record, ...rangeOperands(start, end));
    },
    resolvedOptions() {
      const r = slot(this, 'NumberFormat', 'resolvedOptions');
      const result = { locale: r.locale, numberingSystem: r.numberingSystem, style: r.style };
      for (const key of ['currency', 'currencyDisplay', 'currencySign', ...RESOLVED_KEYS, ...ROUNDING_KEYS]) {
        if (r[key] !== undefined) result[key] = r[key];
      }
      return result;
    },
  });
  boundGetter(NumberFormat.prototype, 'format', 'NumberFormat',
    (record) => (value) => formatNumber(record, toNumeric(value), false));

  function PluralRules(locales = undefined, options = undefined) {
    if (new.target === undefined) typeError("Constructor Intl.PluralRules requires 'new'");
    options = toOptions(options);
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const locale = resolveLocale(locales);
    const type = getOption(options, 'type', ['cardinal', 'ordinal'], 'cardinal');
    const notation = getOption(options, 'notation', ['standard', 'scientific', 'engineering', 'compact'], 'standard');
    const compactDisplay = getOption(options, 'compactDisplay', ['short', 'long'], 'short');
    const record = { kind: 'PluralRules', locale, type, notation, ...digitOptions(options, 0, 3, notation) };
    if (notation === 'compact') record.compactDisplay = compactDisplay;
    record.spec = specOf(record);
    slots.set(this, record);
  }
  const selectCategory = (r, number) => ops.plural_select(r.locale, r.type === 'ordinal', number, r.spec);
  // shortcut: no CLDR plural range data in icu_plurals; categories made of one/other resolve
  // to `other` unless both ends are `one`, anything else to the category of the end.
  const resolveRange = (startCategory, endCategory) => {
    const simple = ['one', 'other'];
    if (!simple.includes(startCategory) || !simple.includes(endCategory)) return endCategory;
    return startCategory === 'one' && endCategory === 'one' ? 'one' : 'other';
  };
  constructor(PluralRules, 'PluralRules', {
    select(value) {
      return selectCategory(slot(this, 'PluralRules', 'select'), Number(value));
    },
    selectRange(start, end) {
      const r = slot(this, 'PluralRules', 'selectRange');
      if (start === undefined || end === undefined) typeError('selectRange requires start and end');
      const x = Number(start);
      const y = Number(end);
      if (Number.isNaN(x) || Number.isNaN(y)) throw new RangeError('Invalid number range');
      return resolveRange(selectCategory(r, x), selectCategory(r, y));
    },
    resolvedOptions() {
      const r = slot(this, 'PluralRules', 'resolvedOptions');
      const result = { locale: r.locale, type: r.type, notation: r.notation };
      for (const key of ['compactDisplay', ...DIGIT_KEYS]) {
        if (r[key] !== undefined) result[key] = r[key];
      }
      result.pluralCategories = ops.plural_categories(r.locale, r.type === 'ordinal').split(',');
      for (const key of ROUNDING_KEYS) result[key] = r[key];
      return result;
    },
  });
  define(Intl, { NumberFormat, PluralRules });
})();
