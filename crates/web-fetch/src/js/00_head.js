// Fetch API (Headers, Request, Response, fetch) written from the Fetch Standard, minus what a
// server-side runtime has no use for: cookie jar, CORS, service workers, referrer handling.
// The fragments share one private scope: this file opens it and 90_exports.js closes it.
(function (global) {
  "use strict";

  const OPS = ["begin", "send", "detach", "read", "write", "end", "close", "release", "abort_upload"];
  const op = {};
  for (const name of OPS) {
    op[name] = global[`op_fetch_${name}`];
    delete global[`op_fetch_${name}`];
  }
  const { createNativeReadable, isDisturbed } = global[Symbol.for("velyr.internal.streams")];

  // Shape of values the standard types as token / reason-phrase.
  const TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
  const REASON_PHRASE = /^[\t\x20-\x7e\x80-\xff]*$/;
  const REDIRECT_STATUSES = [301, 302, 303, 307, 308];
  const NULL_BODY_STATUSES = [101, 204, 205, 304];

  function closeQuietly(id) {
    try {
      op.close(id);
    } catch {
      // Already closed.
    }
  }

  function defineTag(Class, tag) {
    Object.defineProperty(Class.prototype, Symbol.toStringTag, { value: tag, configurable: true });
  }

  function enumerable(Class, names) {
    for (const name of names) {
      const descriptor = Object.getOwnPropertyDescriptor(Class.prototype, name);
      descriptor.enumerable = true;
      Object.defineProperty(Class.prototype, name, descriptor);
    }
  }

  function requireEnum(value, allowed, what) {
    const text = String(value);
    if (!allowed.includes(text)) throw new TypeError(`'${text}' is not a valid value for ${what}`);
    return text;
  }
