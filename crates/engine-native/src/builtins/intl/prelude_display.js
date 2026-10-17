'use strict';
// Intl.DisplayNames (ECMA-402 12) over ops.display_name, which returns undefined without data.
(() => {
  const global = globalThis;
  const { ops, define, slots, slot, constructor, resolveLocale, getOption, typeError } =
    global.__intl_shared;

  const FIELDS = ['era', 'year', 'quarter', 'month', 'weekOfYear', 'weekday', 'day', 'dayPeriod',
    'hour', 'minute', 'second', 'timeZoneName'];
  const CALENDAR_ALIASES = { islamicc: 'islamic-civil', ethioaa: 'ethiopic-amete-alem' };
  const LANGUAGE = /^(?:[a-z]{2,3}|[a-z]{5,8})(?:-[a-z]{4})?(?:-(?:[a-z]{2}|[0-9]{3}))?(?:-(?:[a-z0-9]{5,8}|[0-9][a-z0-9]{3}))*$/i;

  const invalid = (code, type) => { throw new RangeError(`Invalid ${type} code: ${code}`); };
  const match = (pattern, code, type) => (pattern.test(code) ? code : invalid(code, type));
  const title = (code) => code[0].toUpperCase() + code.slice(1).toLowerCase();

  // CanonicalCodeForDisplayNames.
  const canonicalCode = (type, code) => {
    switch (type) {
      case 'language': return ops.canonicalize(match(LANGUAGE, code, type));
      case 'region': return match(/^(?:[a-z]{2}|[0-9]{3})$/i, code, type).toUpperCase();
      case 'script': return title(match(/^[a-z]{4}$/i, code, type));
      case 'currency': return match(/^[a-z]{3}$/i, code, type).toUpperCase();
      case 'calendar': {
        const id = match(/^[a-z0-9]{3,8}(?:-[a-z0-9]{3,8})*$/i, code, type).toLowerCase();
        return CALENDAR_ALIASES[id] ?? id;
      }
      default: return FIELDS.includes(code) ? code : invalid(code, type);
    }
  };

  function DisplayNames(locales, options) {
    if (new.target === undefined) typeError("Constructor Intl.DisplayNames requires 'new'");
    if (options === undefined || (typeof options !== 'object' && typeof options !== 'function') || options === null) {
      typeError('Options must be an object');
    }
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const locale = resolveLocale(locales);
    const style = getOption(options, 'style', ['narrow', 'short', 'long'], 'long');
    const type = getOption(options, 'type',
      ['language', 'region', 'script', 'currency', 'calendar', 'dateTimeField']);
    if (type === undefined) typeError('Required option "type" is missing');
    const fallback = getOption(options, 'fallback', ['code', 'none'], 'code');
    const languageDisplay = getOption(options, 'languageDisplay', ['dialect', 'standard'], 'dialect');
    slots.set(this, { kind: 'DisplayNames', locale, style, type, fallback, languageDisplay });
  }
  constructor(DisplayNames, 'DisplayNames', {
    of(code) {
      const r = slot(this, 'DisplayNames', 'of');
      const canonical = canonicalCode(r.type, `${code}`);
      const name = ops.display_name(r.locale, r.type, r.style, r.languageDisplay, canonical);
      if (name !== undefined) return name;
      return r.fallback === 'code' ? canonical : undefined;
    },
    resolvedOptions() {
      const r = slot(this, 'DisplayNames', 'resolvedOptions');
      const out = { locale: r.locale, style: r.style, type: r.type, fallback: r.fallback };
      if (r.type === 'language') out.languageDisplay = r.languageDisplay;
      return out;
    },
  });
  define(global.Intl, { DisplayNames });
})();
