// Bootstrap: Event, CustomEvent and EventTarget (DOM Standard, without a node tree: `capture` is
// accepted but there is no capture or bubble phase, so every dispatch is at-target).
(function (global) {
  "use strict";

  const NONE = 0;
  const AT_TARGET = 2;

  // Dispatch state kept off the instance so user code cannot forge it.
  const states = new WeakMap();

  function define(name, value) {
    Object.defineProperty(global, name, {
      value,
      writable: true,
      configurable: true,
      enumerable: false,
    });
  }

  class Event {
    static NONE = 0;
    static CAPTURING_PHASE = 1;
    static AT_TARGET = 2;
    static BUBBLING_PHASE = 3;

    constructor(type, init = {}) {
      if (arguments.length === 0) throw new TypeError("Event requires 1 argument");
      if (init !== null && typeof init !== "object" && typeof init !== "function") {
        throw new TypeError("Event init must be an object");
      }
      const dict = init ?? {};
      states.set(this, {
        type: String(type),
        bubbles: Boolean(dict.bubbles),
        cancelable: Boolean(dict.cancelable),
        composed: Boolean(dict.composed),
        timeStamp: typeof performance === "object" && performance !== null ? performance.now() : Date.now(),
        target: null,
        currentTarget: null,
        eventPhase: NONE,
        canceled: false,
        stopped: false,
        stoppedImmediate: false,
        passive: false,
        dispatching: false,
      });
    }

    get type() { return states.get(this).type; }
    get target() { return states.get(this).target; }
    get srcElement() { return states.get(this).target; }
    get currentTarget() { return states.get(this).currentTarget; }
    get eventPhase() { return states.get(this).eventPhase; }
    get bubbles() { return states.get(this).bubbles; }
    get cancelable() { return states.get(this).cancelable; }
    get composed() { return states.get(this).composed; }
    get timeStamp() { return states.get(this).timeStamp; }
    get isTrusted() { return false; }
    get defaultPrevented() { return states.get(this).canceled; }
    get cancelBubble() { return states.get(this).stopped; }
    set cancelBubble(value) { if (value) states.get(this).stopped = true; }
    get returnValue() { return !states.get(this).canceled; }
    set returnValue(value) { if (!value) this.preventDefault(); }

    composedPath() {
      const { currentTarget, dispatching } = states.get(this);
      return dispatching && currentTarget !== null ? [currentTarget] : [];
    }

    preventDefault() {
      const state = states.get(this);
      if (state.cancelable && !state.passive) state.canceled = true;
    }

    stopPropagation() { states.get(this).stopped = true; }

    stopImmediatePropagation() {
      const state = states.get(this);
      state.stopped = true;
      state.stoppedImmediate = true;
    }

    get [Symbol.toStringTag]() { return "Event"; }
  }

  class CustomEvent extends Event {
    #detail;

    constructor(type, init = {}) {
      super(...arguments);
      this.#detail = init?.detail ?? null;
    }

    get detail() { return this.#detail; }
    get [Symbol.toStringTag]() { return "CustomEvent"; }
  }

  function reportListenerError(error) {
    if (typeof global.reportError === "function") global.reportError(error);
    else if (typeof global.console?.error === "function") global.console.error(error);
    // shortcut: with neither reportError nor console the error is dropped; wire reportError when the runtime has one.
  }

  function normalizeOptions(options) {
    if (typeof options === "boolean") return { capture: options, once: false, passive: false, signal: undefined };
    if (options === undefined || options === null) return { capture: false, once: false, passive: false, signal: undefined };
    if (typeof options !== "object" && typeof options !== "function") {
      return { capture: Boolean(options), once: false, passive: false, signal: undefined };
    }
    const { signal } = options;
    if (signal !== undefined && !(signal instanceof global.AbortSignal)) {
      throw new TypeError("The 'signal' option must be an AbortSignal");
    }
    return { capture: Boolean(options.capture), once: Boolean(options.once), passive: Boolean(options.passive), signal };
  }

  function normalizeCapture(options) {
    if (typeof options === "object" && options !== null) return Boolean(options.capture);
    return Boolean(options);
  }

  class EventTarget {
    // type -> listener records, in insertion order.
    #listeners = new Map();

    addEventListener(type, callback, options) {
      if (arguments.length < 2) throw new TypeError("addEventListener requires 2 arguments");
      if (callback === null || callback === undefined) return;
      const { capture, once, passive, signal } = normalizeOptions(options);
      if (signal?.aborted) return;
      const key = String(type);
      const list = this.#listeners.get(key) ?? [];
      if (list.some((l) => l.callback === callback && l.capture === capture)) return;
      const record = { callback, capture, once, passive, removed: false };
      list.push(record);
      this.#listeners.set(key, list);
      signal?.addEventListener("abort", () => this.#remove(key, record), { once: true });
    }

    removeEventListener(type, callback, options) {
      if (arguments.length < 2) throw new TypeError("removeEventListener requires 2 arguments");
      if (callback === null || callback === undefined) return;
      const key = String(type);
      const capture = normalizeCapture(options);
      const record = this.#listeners.get(key)?.find((l) => l.callback === callback && l.capture === capture);
      if (record !== undefined) this.#remove(key, record);
    }

    #remove(type, record) {
      record.removed = true;
      const list = this.#listeners.get(type);
      if (list === undefined) return;
      const index = list.indexOf(record);
      if (index !== -1) list.splice(index, 1);
      if (list.length === 0) this.#listeners.delete(type);
    }

    dispatchEvent(event) {
      if (!(event instanceof Event)) throw new TypeError("dispatchEvent expects an Event");
      const state = states.get(event);
      if (state.dispatching) {
        throw new global.DOMException("The event is already being dispatched.", "InvalidStateError");
      }
      state.dispatching = true;
      state.target = this;
      state.currentTarget = this;
      state.eventPhase = AT_TARGET;
      // Snapshot: listeners added mid-dispatch do not run; removed ones are skipped via `removed`.
      for (const record of [...(this.#listeners.get(state.type) ?? [])]) {
        if (record.removed) continue;
        if (record.once) this.#remove(state.type, record);
        this.#invoke(record, event, state);
        if (state.stoppedImmediate) break;
      }
      state.dispatching = false;
      state.currentTarget = null;
      state.eventPhase = NONE;
      state.stopped = false;
      state.stoppedImmediate = false;
      return !state.canceled;
    }

    #invoke(record, event, state) {
      state.passive = record.passive;
      try {
        const { callback } = record;
        if (typeof callback === "function") {
          Reflect.apply(callback, this, [event]);
        } else {
          const handler = callback.handleEvent;
          if (typeof handler !== "function") throw new TypeError("handleEvent is not a function");
          Reflect.apply(handler, callback, [event]);
        }
      } catch (error) {
        reportListenerError(error);
      } finally {
        state.passive = false;
      }
    }

    get [Symbol.toStringTag]() { return "EventTarget"; }
  }

  define("Event", Event);
  define("CustomEvent", CustomEvent);
  define("EventTarget", EventTarget);
})(globalThis);
