'use strict';
// Intl.RelativeTimeFormat (ECMA-402 17) over ops.relative_time, which returns encoded parts.
(() => {
  const global = globalThis;
  const { ops, define, slots, slot, constructor, resolveLocaleKeys, typeOption, getOption,
    decodeParts, typeError } = global.__intl_shared;

  const UNITS = ['second', 'minute', 'hour', 'day', 'week', 'month', 'quarter', 'year'];
  // SingularRelativeTimeUnit: the plural spellings are accepted too.
  const singularUnit = (unit) => {
    const name = `${unit}`;
    const singular = name.endsWith('s') ? name.slice(0, -1) : name;
    if (!UNITS.includes(singular)) throw new RangeError(`Invalid unit argument for format() '${name}'`);
    return singular;
  };
  const run = (r, value, unit) => {
    const number = Number(value);
    const singular = singularUnit(unit);
    if (!Number.isFinite(number)) throw new RangeError('Value need to be finite number for Intl.RelativeTimeFormat.prototype.format()');
    const parts = decodeParts(ops.relative_time(r.data, r.style, r.numeric, number, singular));
    return parts.map((part) => (part.type === 'literal' ? part : { ...part, unit: singular }));
  };

  function RelativeTimeFormat(locales = undefined, options = undefined) {
    if (new.target === undefined) typeError("Constructor Intl.RelativeTimeFormat requires 'new'");
    if (options === null) typeError('Options must be an object or undefined');
    options = options === undefined ? Object.create(null) : Object(options);
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const numberingSystem = typeOption(options, 'numberingSystem');
    const { locale, data, values } = resolveLocaleKeys(locales, { nu: numberingSystem });
    const style = getOption(options, 'style', ['long', 'short', 'narrow'], 'long');
    const numeric = getOption(options, 'numeric', ['always', 'auto'], 'always');
    slots.set(this, { kind: 'RelativeTimeFormat', locale, data, style, numeric, numberingSystem: values.nu ?? 'latn' });
  }
  constructor(RelativeTimeFormat, 'RelativeTimeFormat', {
    format(value, unit) {
      const parts = run(slot(this, 'RelativeTimeFormat', 'format'), value, unit);
      return parts.map((part) => part.value).join('');
    },
    formatToParts(value, unit) { return run(slot(this, 'RelativeTimeFormat', 'formatToParts'), value, unit); },
    resolvedOptions() {
      const r = slot(this, 'RelativeTimeFormat', 'resolvedOptions');
      return { locale: r.locale, style: r.style, numeric: r.numeric, numberingSystem: r.numberingSystem };
    },
  });
  define(global.Intl, { RelativeTimeFormat });
})();
