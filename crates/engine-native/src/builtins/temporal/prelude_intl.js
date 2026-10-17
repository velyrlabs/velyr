'use strict';
// toLocaleString of every Temporal type, over Intl (the lazy shell captured here: reading
// `globalThis.Intl` does not materialize it, the first call does). Last Temporal prelude: it
// removes the shared helpers from the global.
(() => {
  const global = globalThis;
  const T = global.__temporal_shared;
  const { Temporal, defineMethods, strOf, slotOf } = T;
  const { fromEntries } = Object;
  const intl = global.Intl;

  // The Intl side leaves its formatting hook on the global when it materializes; take it once.
  let hook;
  const intlHook = () => {
    if (hook === undefined) {
      void intl.DateTimeFormat;
      hook = global.__intl_temporal;
      delete global.__intl_temporal;
    }
    return hook;
  };

  for (const cls of ['PlainDate', 'PlainTime', 'PlainDateTime', 'PlainYearMonth', 'PlainMonthDay', 'Instant', 'ZonedDateTime']) {
    defineMethods(Temporal[cls].prototype, {
      toLocaleString(locales = undefined, options = undefined) {
        strOf(this, cls);
        return intlHook()(cls, this, locales, options);
      },
    });
  }
  // Read from the slot, not through the getters, which user code may have replaced.
  const DURATION_UNITS = ['years', 'months', 'weeks', 'days', 'hours', 'minutes', 'seconds', 'milliseconds', 'microseconds', 'nanoseconds'];
  defineMethods(Temporal.Duration.prototype, {
    toLocaleString(locales = undefined, options = undefined) {
      const slot = slotOf(this);
      if (slot?.cls !== 'Duration') throw new TypeError('Receiver is not a Temporal.Duration');
      const fields = slot.fields.split(',');
      const duration = fromEntries(DURATION_UNITS.map((unit, i) => [unit, Number(fields[i])]));
      return new intl.DurationFormat(locales, options).format(duration);
    },
  });
})();
delete globalThis.__temporal_shared;
