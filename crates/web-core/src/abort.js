// Bootstrap: AbortController and AbortSignal. Needs Event, EventTarget and DOMException (event.js,
// dom_exception.js) and the timers bootstrap for `AbortSignal.timeout`.
(function (global) {
  "use strict";

  const { Event, EventTarget, DOMException } = global;
  const CONSTRUCT = {};
  let signalAbort;
  let createSignal;

  function define(name, value) {
    Object.defineProperty(global, name, {
      value,
      writable: true,
      configurable: true,
      enumerable: false,
    });
  }

  class AbortSignal extends EventTarget {
    #aborted = false;
    #reason = undefined;
    #onabort = null;

    constructor(token) {
      if (token !== CONSTRUCT) throw new TypeError("Illegal constructor");
      super();
    }

    static {
      createSignal = () => new AbortSignal(CONSTRUCT);
      signalAbort = (signal, reason) => {
        if (signal.#aborted) return;
        signal.#aborted = true;
        signal.#reason = reason === undefined
          ? new DOMException("signal is aborted without reason", "AbortError")
          : reason;
        signal.dispatchEvent(new Event("abort"));
      };
    }

    static abort(reason) {
      const signal = createSignal();
      signalAbort(signal, reason);
      return signal;
    }

    static timeout(milliseconds) {
      const delay = Number(milliseconds);
      if (!(delay >= 0)) throw new TypeError("AbortSignal.timeout requires a non-negative number");
      const signal = createSignal();
      // shortcut: the timer is ref'd (timers.js hides op_timer_ref), so a pending timeout keeps the loop alive until it fires; unref once timers expose it.
      global.setTimeout(
        () => signalAbort(signal, new DOMException("signal timed out", "TimeoutError")),
        delay,
      );
      return signal;
    }

    static any(signals) {
      const sources = Array.from(signals);
      for (const source of sources) {
        if (!(source instanceof AbortSignal)) throw new TypeError("AbortSignal.any expects AbortSignals");
      }
      const result = createSignal();
      const already = sources.find((source) => source.aborted);
      if (already !== undefined) {
        signalAbort(result, already.reason);
        return result;
      }
      // Dropping the listeners on the other sources once one fires keeps them from pinning `result`.
      const cleanup = new AbortController();
      for (const source of sources) {
        source.addEventListener("abort", () => {
          cleanup.abort();
          signalAbort(result, source.reason);
        }, { once: true, signal: cleanup.signal });
      }
      return result;
    }

    get aborted() { return this.#aborted; }
    get reason() { return this.#reason; }

    throwIfAborted() {
      if (this.#aborted) throw this.#reason;
    }

    get onabort() { return this.#onabort; }
    set onabort(handler) {
      const first = this.#onabort === null;
      this.#onabort = typeof handler === "function" ? handler : null;
      if (first && this.#onabort !== null) {
        this.addEventListener("abort", (event) => this.#onabort?.call(this, event));
      }
    }

    get [Symbol.toStringTag]() { return "AbortSignal"; }
  }

  class AbortController {
    #signal = createSignal();

    get signal() { return this.#signal; }

    abort(reason) {
      signalAbort(this.#signal, reason);
    }

    get [Symbol.toStringTag]() { return "AbortController"; }
  }

  define("AbortSignal", AbortSignal);
  define("AbortController", AbortController);
})(globalThis);
