'use strict';
// Intl.DurationFormat (ECMA-402 stage 4) in JS over Intl.ListFormat and Intl.PluralRules.
// shortcut: NumberFormat has no unit style yet, so the unit labels are an `en`/`pt` table
// (every other locale uses the `en` one); the ':' separator and '-' sign are not localized.
// Upgrade when NumberFormat unit style lands (N44).
(() => {
  const global = globalThis;
  const { define, slots, slot, constructor, resolveLocaleKeys, typeOption, getOption, getNumber,
    toOptions, typeError } = global.__intl_shared;
  const { NumberFormat, ListFormat, PluralRules } = global.Intl;

  const NAMED = ['long', 'short', 'narrow'];
  const ROWS = [
    { unit: 'year', field: 'years', styles: NAMED, digitalBase: 'short' },
    { unit: 'month', field: 'months', styles: NAMED, digitalBase: 'short' },
    { unit: 'week', field: 'weeks', styles: NAMED, digitalBase: 'short' },
    { unit: 'day', field: 'days', styles: NAMED, digitalBase: 'short' },
    { unit: 'hour', field: 'hours', styles: [...NAMED, 'numeric', '2-digit'], digitalBase: 'numeric' },
    { unit: 'minute', field: 'minutes', styles: [...NAMED, 'numeric', '2-digit'], digitalBase: 'numeric' },
    { unit: 'second', field: 'seconds', styles: [...NAMED, 'numeric', '2-digit'], digitalBase: 'numeric' },
    { unit: 'millisecond', field: 'milliseconds', styles: [...NAMED, 'numeric'], digitalBase: 'numeric' },
    { unit: 'microsecond', field: 'microseconds', styles: [...NAMED, 'numeric'], digitalBase: 'numeric' },
    { unit: 'nanosecond', field: 'nanoseconds', styles: [...NAMED, 'numeric'], digitalBase: 'numeric' },
  ];
  const NUMERIC_STYLES = ['numeric', '2-digit'];
  const CLOCK = ['hours', 'minutes', 'seconds'];

  // Labels [one, other] per style; narrow glues the label to the number.
  const LABELS = {
    en: {
      long: { year: ['year', 'years'], month: ['month', 'months'], week: ['week', 'weeks'], day: ['day', 'days'],
        hour: ['hour', 'hours'], minute: ['minute', 'minutes'], second: ['second', 'seconds'],
        millisecond: ['millisecond', 'milliseconds'], microsecond: ['microsecond', 'microseconds'],
        nanosecond: ['nanosecond', 'nanoseconds'] },
      short: { year: ['yr', 'yrs'], month: ['mth', 'mths'], week: ['wk', 'wks'], day: ['day', 'days'],
        hour: ['hr', 'hr'], minute: ['min', 'min'], second: ['sec', 'sec'], millisecond: ['ms', 'ms'],
        microsecond: ['μs', 'μs'], nanosecond: ['ns', 'ns'] },
      narrow: { year: ['y', 'y'], month: ['m', 'm'], week: ['w', 'w'], day: ['d', 'd'], hour: ['h', 'h'],
        minute: ['m', 'm'], second: ['s', 's'], millisecond: ['ms', 'ms'], microsecond: ['μs', 'μs'],
        nanosecond: ['ns', 'ns'] },
    },
    pt: {
      long: { year: ['ano', 'anos'], month: ['mês', 'meses'], week: ['semana', 'semanas'], day: ['dia', 'dias'],
        hour: ['hora', 'horas'], minute: ['minuto', 'minutos'], second: ['segundo', 'segundos'],
        millisecond: ['milissegundo', 'milissegundos'], microsecond: ['microssegundo', 'microssegundos'],
        nanosecond: ['nanossegundo', 'nanossegundos'] },
      short: { year: ['ano', 'anos'], month: ['mês', 'meses'], week: ['sem.', 'sem.'], day: ['dia', 'dias'],
        hour: ['h', 'h'], minute: ['min', 'min'], second: ['s', 's'], millisecond: ['ms', 'ms'],
        microsecond: ['μs', 'μs'], nanosecond: ['ns', 'ns'] },
      narrow: { year: ['a', 'a'], month: ['m', 'm'], week: ['sem.', 'sem.'], day: ['d', 'd'], hour: ['h', 'h'],
        minute: ['min', 'min'], second: ['s', 's'], millisecond: ['ms', 'ms'], microsecond: ['μs', 'μs'],
        nanosecond: ['ns', 'ns'] },
    },
  };

  // GetDurationUnitOptions.
  const unitOptions = (row, options, baseStyle, prevStyle) => {
    const { unit, field } = row;
    let style = getOption(options, field, row.styles);
    let displayDefault = 'always';
    if (style === undefined) {
      if (baseStyle === 'digital') {
        if (!CLOCK.includes(field)) displayDefault = 'auto';
        style = row.digitalBase;
      } else if (prevStyle === 'fractional' || NUMERIC_STYLES.includes(prevStyle)) {
        if (field !== 'minutes' && field !== 'seconds') displayDefault = 'auto';
        style = 'numeric';
      } else {
        displayDefault = 'auto';
        style = baseStyle;
      }
    }
    if (style === 'numeric' && ['millisecond', 'microsecond', 'nanosecond'].includes(unit)) {
      style = 'fractional';
      displayDefault = 'auto';
    }
    const display = getOption(options, `${field}Display`, ['auto', 'always'], displayDefault);
    if (display === 'always' && style === 'fractional') {
      throw new RangeError(`${field}Display cannot be "always" with a fractional style`);
    }
    if (prevStyle === 'fractional' && style !== 'fractional') {
      throw new RangeError(`${field} style must be fractional after a fractional unit`);
    }
    if (NUMERIC_STYLES.includes(prevStyle) && style !== 'fractional' && !NUMERIC_STYLES.includes(style)) {
      throw new RangeError(`${field} style must be numeric after a numeric unit`);
    }
    if ((field === 'minutes' || field === 'seconds') && NUMERIC_STYLES.includes(prevStyle)) style = '2-digit';
    return { style, display };
  };

  // ToDurationRecord, read in the property order the spec uses.
  const READ_ORDER = ['days', 'hours', 'microseconds', 'milliseconds', 'minutes', 'months', 'nanoseconds',
    'seconds', 'weeks', 'years'];
  // shortcut: fractions are only accepted on seconds, not on hours or minutes.
  const ISO_DURATION = /^([+\u2212-])?P(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)W)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)(?:[.,](\d{1,9}))?S)?)?$/i;
  const parseDurationString = (text) => {
    const match = ISO_DURATION.exec(text);
    if (match === null || text.endsWith('T') || text.slice(1).toUpperCase() === 'P') {
      throw new RangeError(`Invalid duration string: ${text}`);
    }
    const [, sign, years, months, weeks, days, hours, minutes, seconds, fraction = ''] = match;
    const factor = sign === '-' || sign === '\u2212' ? -1 : 1;
    const sub = fraction.padEnd(9, '0');
    const unit = (digits) => factor * Number(digits ?? 0) || 0;
    return { years: unit(years), months: unit(months), weeks: unit(weeks), days: unit(days), hours: unit(hours),
      minutes: unit(minutes), seconds: unit(seconds), milliseconds: unit(sub.slice(0, 3)),
      microseconds: unit(sub.slice(3, 6)), nanoseconds: unit(sub.slice(6, 9)) };
  };
  const toDurationRecord = (input) => {
    if (typeof input === 'string') return parseDurationString(input);
    if (typeof input !== 'object' || input === null) typeError('Duration must be an object or string');
    const record = {};
    let any = false;
    for (const field of READ_ORDER) {
      const raw = input[field];
      if (raw === undefined) { record[field] = 0; continue; }
      any = true;
      const number = +raw;
      if (!Number.isInteger(number)) throw new RangeError(`${field} must be an integer`);
      record[field] = number === 0 ? 0 : number;
    }
    if (!any) typeError('Duration must have at least one unit');
    return record;
  };
  const durationSign = (record) => {
    let sign = 0;
    for (const field of READ_ORDER) {
      const value = record[field];
      if (value === 0) continue;
      const next = value < 0 ? -1 : 1;
      if (sign !== 0 && sign !== next) throw new RangeError('Duration units must all have the same sign');
      sign = next;
    }
    return sign;
  };
  const checkRange = (record) => {
    for (const field of ['years', 'months', 'weeks']) {
      if (Math.abs(record[field]) >= 2 ** 32) throw new RangeError(`${field} is out of range`);
    }
    const nanos = BigInt(record.days) * 86400000000000n + BigInt(record.hours) * 3600000000000n +
      BigInt(record.minutes) * 60000000000n + BigInt(record.seconds) * 1000000000n +
      BigInt(record.milliseconds) * 1000000n + BigInt(record.microseconds) * 1000n + BigInt(record.nanoseconds);
    if ((nanos < 0n ? -nanos : nanos) >= 9007199254740992000000000n) throw new RangeError('Duration is out of range');
  };

  const SUB_SECOND = [{ field: 'seconds', digits: 9 }, { field: 'milliseconds', digits: 6 },
    { field: 'microseconds', digits: 3 }, { field: 'nanoseconds', digits: 0 }];
  // |value| of the unit at `from` (a ROWS index >= 6) with every smaller unit folded in as a
  // fraction, truncated to `maxDigits`.
  const foldFraction = (record, from, maxDigits) => {
    let nanos = 0n;
    for (let i = from - 6; i < SUB_SECOND.length; i++) {
      const magnitude = BigInt(Math.abs(record[SUB_SECOND[i].field]));
      nanos += magnitude * 10n ** BigInt(SUB_SECOND[i].digits);
    }
    const scale = SUB_SECOND[from - 6].digits;
    const text = nanos.toString().padStart(scale + 1, '0');
    const integer = text.slice(0, text.length - scale);
    const fraction = text.slice(text.length - scale).slice(0, maxDigits ?? 9);
    return Number(fraction === '' ? integer : `${integer}.${fraction}`);
  };

  const annotate = (parts, unit) => parts.map((part) => ({ ...part, unit }));

  // The parts of one unit; `fraction` is the fraction-digit options of a folded unit.
  const unitParts = (r, row, value, style, negative, fraction) => {
    const numeric = NUMERIC_STYLES.includes(style);
    const digits = { ...fraction, ...(style === '2-digit' ? { minimumIntegerDigits: 2 } : {}), ...(numeric ? { useGrouping: false } : {}) };
    const formatter = new NumberFormat(r.data, { numberingSystem: r.numberingSystem, ...digits });
    const parts = [...(negative ? [{ type: 'minusSign', value: '-' }] : []), ...formatter.formatToParts(value)];
    if (numeric) return annotate(parts, row.unit);
    const labels = (LABELS[r.locale.split('-')[0]] ?? LABELS.en)[style][row.unit];
    const label = new PluralRules(r.data).select(value) === 'one' ? labels[0] : labels[1];
    const glue = style === 'narrow' ? [] : [{ type: 'literal', value: ' ' }];
    return annotate([...parts, ...glue, { type: 'unit', value: label }], row.unit);
  };

  // Zero numeric minutes stay visible between numeric hours and displayed seconds.
  const secondsPresent = (r, record) => r.seconds.display === 'always' || record.seconds !== 0 ||
    record.milliseconds !== 0 || record.microseconds !== 0 || record.nanoseconds !== 0;

  // PartitionDurationFormatPattern: one parts array per list element.
  const partition = (r, record) => {
    const sign = durationSign(record);
    checkRange(record);
    const elements = [];
    let negative = sign < 0;
    let group;
    for (let i = 0; i < ROWS.length; i++) {
      const row = ROWS[i];
      const field = row.field;
      const { style, display } = r[field];
      if (style === 'fractional') continue;
      const folded = i >= 6 && i < 9 && r[ROWS[i + 1].field].style === 'fractional';
      const value = folded ? foldFraction(record, i, r.fractionalDigits) : Math.abs(record[row.field]);
      const minutesRequired = field === 'minutes' && group !== undefined && secondsPresent(r, record);
      // Truncation to fractionalDigits must not hide a folded unit that was non-zero.
      const zero = folded ? SUB_SECOND.slice(i - 6).every((sub) => record[sub.field] === 0) : value === 0;
      if (display === 'auto' && zero && !minutesRequired) continue;
      const fraction = folded
        ? { minimumFractionDigits: r.fractionalDigits ?? 0, maximumFractionDigits: r.fractionalDigits ?? 9 }
        : undefined;
      const parts = unitParts(r, row, value, style, negative, fraction);
      negative = false;
      if (NUMERIC_STYLES.includes(style) && group !== undefined) {
        group.push({ type: 'literal', value: ':' }, ...parts);
      } else if (NUMERIC_STYLES.includes(style)) {
        group = parts;
        elements.push(group);
      } else {
        elements.push(parts);
      }
      if (folded) break;
    }
    return elements;
  };

  const run = (r, duration) => {
    const elements = partition(r, toDurationRecord(duration));
    if (elements.length === 0) return [];
    const style = r.style === 'digital' ? 'short' : r.style;
    const list = new ListFormat(r.data, { type: 'unit', style });
    const listed = list.formatToParts(elements.map((_, index) => String(index)));
    return listed.flatMap((part) => (part.type === 'element' ? elements[Number(part.value)] : [part]));
  };

  function DurationFormat(locales = undefined, options = undefined) {
    if (new.target === undefined) typeError("Constructor Intl.DurationFormat requires 'new'");
    if (options !== undefined && (typeof options !== 'object' || options === null) && typeof options !== 'function') {
      typeError('Options must be an object or undefined');
    }
    options = toOptions(options);
    getOption(options, 'localeMatcher', ['lookup', 'best fit']);
    const numberingSystem = typeOption(options, 'numberingSystem');
    const { locale, data, values } = resolveLocaleKeys(locales, { nu: numberingSystem });
    const style = getOption(options, 'style', ['long', 'short', 'narrow', 'digital'], 'short');
    const record = { kind: 'DurationFormat', locale, data, numberingSystem: values.nu ?? 'latn', style };
    let prevStyle = '';
    for (const row of ROWS) {
      record[row.field] = unitOptions(row, options, style, prevStyle);
      if (['hours', 'minutes', 'seconds', 'milliseconds', 'microseconds'].includes(row.field)) {
        prevStyle = record[row.field].style;
      }
    }
    record.fractionalDigits = getNumber(options, 'fractionalDigits', 0, 9, undefined);
    slots.set(this, record);
  }
  constructor(DurationFormat, 'DurationFormat', {
    format(duration) {
      return run(slot(this, 'DurationFormat', 'format'), duration).map((part) => part.value).join('');
    },
    formatToParts(duration) { return run(slot(this, 'DurationFormat', 'formatToParts'), duration); },
    resolvedOptions() {
      const r = slot(this, 'DurationFormat', 'resolvedOptions');
      const out = { locale: r.locale, numberingSystem: r.numberingSystem, style: r.style };
      for (const row of ROWS) {
        out[row.field] = r[row.field].style === 'fractional' ? 'numeric' : r[row.field].style;
        out[`${row.field}Display`] = r[row.field].display;
      }
      if (r.fractionalDigits !== undefined) out.fractionalDigits = r.fractionalDigits;
      return out;
    },
  });
  define(global.Intl, { DurationFormat });
})();
