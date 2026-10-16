'use strict';
// node:stream/web: the Web Streams classes the runtime already installs (web-streams crate).
// Getters keep the load free of ordering requirements: a host that installs web-streams after
// compat-node still gets the right classes on first access.
for (const name of [
  'ByteLengthQueuingStrategy', 'CompressionStream', 'CountQueuingStrategy', 'DecompressionStream',
  'ReadableByteStreamController', 'ReadableStream', 'ReadableStreamBYOBReader', 'ReadableStreamBYOBRequest',
  'ReadableStreamDefaultController', 'ReadableStreamDefaultReader', 'TextDecoderStream', 'TextEncoderStream',
  'TransformStream', 'TransformStreamDefaultController', 'WritableStream', 'WritableStreamDefaultController',
  'WritableStreamDefaultWriter',
]) {
  Object.defineProperty(exports, name, { enumerable: true, configurable: true, get: () => globalThis[name] });
}
