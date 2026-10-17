'use strict';
// Intl.ListFormat (ECMA-402 13) over ops.list_format, which returns encoded parts.
(() => {
  const global = globalThis;
  const { ops, define, tag, slots, slot, constructor, resolveLocale, getOption, typeError } =
    global.__intl_shared;

  // StringListFromIterable; for-of closes the iterator when the body throws.
  const stringList = (iterable) => {
    const list = [];
    if (iterable === undefined) return list;
    for (const item of iterable) {
      if (typeof item !== 'string') typeError(`Iterable yielded ${String(item)} which is not a string`);
      list.push(item);
    }
    return list;
  };
  const decode = (encoded) => {
    const parts = [];
    if (encoded === '') return parts;
    for (const part of encoded.split('\u0002')) {
      const cut = part.indexOf('\u0001');
      parts.push({ type: part.slice(0, cut), value: part.slice(cut + 1) });
    }
    return parts;
  };
  const run = (r, list) => decode(ops.list_format(r.locale, r.type, r.style, ...stringList(list)));

  function ListFormat(locales = undefined, options = undefined) {
    if (new.target === undefined) typeError("Constructor Intl.ListFormat requires 'new'");
    if (options !== undefined && (typeof options !== 'object' || options === null) && typeof options !== 'function') {
      typeError('Options must be an object or undefined');
    }
    options = options === undefined ? Object.create(null) : options;
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const locale = resolveLocale(locales);
    const type = getOption(options, 'type', ['conjunction', 'disjunction', 'unit'], 'conjunction');
    const style = getOption(options, 'style', ['long', 'short', 'narrow'], 'long');
    slots.set(this, { kind: 'ListFormat', locale, type, style });
  }
  constructor(ListFormat, 'ListFormat', {
    format(list) {
      const parts = run(slot(this, 'ListFormat', 'format'), list);
      return parts.map((part) => part.value).join('');
    },
    formatToParts(list) { return run(slot(this, 'ListFormat', 'formatToParts'), list); },
    resolvedOptions() {
      const r = slot(this, 'ListFormat', 'resolvedOptions');
      return { locale: r.locale, type: r.type, style: r.style };
    },
  });
  define(global.Intl, { ListFormat });
})();
