// Bootstrap: the worker global scope (postMessage, onmessage, close, reportError, name).
// Called with the toolkit messaging.js returned and the worker's name.
(function (toolkit, name) {
  "use strict";

  const { op, send, startPump, defineHandler, messageHandlers } = toolkit;
  const state = { id: op.worker_scope_port(), refd: true, pumping: false };
  const target = new EventTarget();
  let closing = false;

  function define(key, value) {
    Object.defineProperty(globalThis, key, { value, writable: true, configurable: true, enumerable: false });
  }

  const handlers = messageHandlers(target);
  // The parent went away: stop listening so the worker can finish.
  handlers.closed = undefined;

  // shortcut: listeners run with `this` and `event.target` set to an internal EventTarget, not
  // the global object; upgrade when the engine can make the global an EventTarget instance.
  define("addEventListener", function addEventListener(type, ...rest) {
    if (type === "message") startPump(target, state, handlers);
    return Reflect.apply(target.addEventListener, target, [type, ...rest]);
  });
  define("removeEventListener", function removeEventListener(...args) {
    return Reflect.apply(target.removeEventListener, target, args);
  });
  define("dispatchEvent", function dispatchEvent(event) {
    return target.dispatchEvent(event);
  });
  defineHandler(globalThis, "message");
  defineHandler(globalThis, "messageerror");

  define("postMessage", function postMessage(message, transfer) {
    if (arguments.length === 0) throw new TypeError("postMessage requires 1 argument");
    if (!closing) send(state.id, message, transfer, null);
  });
  define("close", function close() {
    if (closing) return;
    closing = true;
    op.worker_close();
  });
  // Errors thrown by listeners land here (see EventTarget); the parent sees an `error` event.
  define("reportError", function reportError(error) {
    op.worker_report_error(
      error instanceof Error ? error.message : String(error),
      error instanceof Error ? error.stack : undefined,
    );
  });
  define("name", String(name));
  if (!Object.hasOwn(globalThis, "self")) define("self", globalThis);
})
