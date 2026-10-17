'use strict';
// Intl.NumberFormat (ECMA-402 chapter 15) over `__intl_number_format`. Runs after
// prelude.js and picks up its shared helpers from `__intl_shared`.
// shortcut: no unit style and no compact notation (they throw): ICU4X keeps that data in
// icu_experimental, which engine-native does not depend on yet.
(() => {
  const global = globalThis;
  const { ops, define, slots, slot, boundGetter, constructor, instance, resolveLocaleKeys,
    typeOption, isAsciiAlpha, toOptions, getOption, getNumber, decodeParts, typeError } = global.__intl_shared;
  const Intl = global.Intl;

  // The order `number.rs` reads the spec in.
  const SPEC_KEYS = ['minimumIntegerDigits', 'minimumFractionDigits', 'maximumFractionDigits',
    'minimumSignificantDigits', 'maximumSignificantDigits', 'useGrouping', 'style', 'currency',
    'currencyDisplay', 'currencySign', 'notation', 'signDisplay'];

  // SetNumberFormatDigitOptions: significant digits win over fraction digits.
  const digitOptions = (options, fractionDefault) => {
    const minimumIntegerDigits = getNumber(options, 'minimumIntegerDigits', 1, 21, 1);
    const { minimumFractionDigits: minFraction, maximumFractionDigits: maxFraction,
      minimumSignificantDigits: minSignificant, maximumSignificantDigits: maxSignificant } = options;
    if (minSignificant !== undefined || maxSignificant !== undefined) {
      const min = getNumber(options, 'minimumSignificantDigits', 1, 21, 1);
      const max = getNumber(options, 'maximumSignificantDigits', min, 21, 21);
      return { minimumIntegerDigits, minimumSignificantDigits: min, maximumSignificantDigits: max };
    }
    const min = getNumber(options, 'minimumFractionDigits', 0, 100, undefined);
    const max = getNumber(options, 'maximumFractionDigits', 0, 100, undefined);
    if (min === undefined && max === undefined) {
      return { minimumIntegerDigits, minimumFractionDigits: fractionDefault.min, maximumFractionDigits: fractionDefault.max };
    }
    if (min === undefined) {
      return { minimumIntegerDigits, minimumFractionDigits: Math.min(fractionDefault.min, max), maximumFractionDigits: max };
    }
    if (max === undefined) {
      return { minimumIntegerDigits, minimumFractionDigits: min, maximumFractionDigits: Math.max(fractionDefault.max, min) };
    }
    if (min > max) throw new RangeError('maximumFractionDigits value is out of range.');
    return { minimumIntegerDigits, minimumFractionDigits: min, maximumFractionDigits: max };
  };

  const groupingOption = (options) => {
    const grouping = options.useGrouping;
    if (grouping === false || grouping === null || grouping === 0 || grouping === '') return false;
    if (grouping === true) return 'always';
    if (grouping === undefined) return 'auto';
    const value = getOption(options, 'useGrouping', ['always', 'auto', 'min2', 'true', 'false']);
    return { true: 'always', false: false }[value] ?? value;
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
    const record = { kind: 'NumberFormat', locale, data, numberingSystem: values.nu ?? 'latn', style };
    let fractionDefault = { min: 0, max: style === 'percent' ? 0 : 3 };
    if (style === 'currency') {
      currency = currency.toUpperCase();
      record.currency = currency;
      record.currencyDisplay = getOption(options, 'currencyDisplay', ['code', 'symbol', 'narrowSymbol', 'name'], 'symbol');
      record.currencySign = getOption(options, 'currencySign', ['standard', 'accounting'], 'standard');
      if (record.currencyDisplay === 'name') {
        throw new RangeError('Intl.NumberFormat currencyDisplay "name" is not supported yet');
      }
      const digits = ops.currency_digits(currency);
      fractionDefault = { min: digits, max: digits };
    }
    if (style === 'unit') throw new RangeError('Intl.NumberFormat style "unit" is not supported yet');
    record.notation = getOption(options, 'notation', ['standard', 'scientific', 'engineering', 'compact'], 'standard');
    if (record.notation === 'compact') throw new RangeError('Intl.NumberFormat notation "compact" is not supported yet');
    Object.assign(record, digitOptions(options, fractionDefault));
    record.useGrouping = groupingOption(options);
    record.signDisplay = getOption(options, 'signDisplay', ['auto', 'never', 'always', 'exceptZero', 'negative'], 'auto');
    record.spec = SPEC_KEYS.map((key) => (record[key] === undefined ? '' : String(record[key]))).join(',');
    slots.set(self, record);
    return self;
  }
  const formatNumber = (record, value, parts) => {
    const x = typeof value === 'bigint' ? String(value) : Number(value);
    return ops.number_format(record.data, x, record.spec, parts);
  };
  const toNumeric = (value) => {
    const primitive = typeof value === 'object' && value !== null ? Number(value) : value;
    return typeof primitive === 'bigint' ? primitive : Number(primitive);
  };
  const RESOLVED_KEYS = ['minimumIntegerDigits', 'minimumFractionDigits', 'maximumFractionDigits',
    'minimumSignificantDigits', 'maximumSignificantDigits', 'useGrouping', 'notation', 'signDisplay'];
  constructor(NumberFormat, 'NumberFormat', {
    formatToParts(value) {
      return decodeParts(formatNumber(slot(this, 'NumberFormat', 'formatToParts'), toNumeric(value), true));
    },
    resolvedOptions() {
      const r = slot(this, 'NumberFormat', 'resolvedOptions');
      const result = { locale: r.locale, numberingSystem: r.numberingSystem, style: r.style };
      for (const key of ['currency', 'currencyDisplay', 'currencySign', ...RESOLVED_KEYS]) {
        if (r[key] !== undefined) result[key] = r[key];
      }
      return Object.assign(result, { roundingIncrement: 1, roundingMode: 'halfExpand',
        roundingPriority: 'auto', trailingZeroDisplay: 'auto' });
    },
  });
  boundGetter(NumberFormat.prototype, 'format', 'NumberFormat',
    (record) => (value) => formatNumber(record, toNumeric(value), false));
  define(Intl, { NumberFormat });
})();
