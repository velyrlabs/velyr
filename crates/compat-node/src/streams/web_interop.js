'use strict';
// Readable/Writable <-> WHATWG streams adapters, written from the Streams Standard and the
// documented node:stream behavior (not vendored). Backpressure maps `desiredSize` to
// pause/resume on the Node side and `drain`/`ready` on the other.
const eos = require('internal/streams/end-of-stream');
const { AbortError, codes: { ERR_INVALID_ARG_TYPE } } = require('internal/streams/ours/errors');
const { isReadableNodeStream, isWritableNodeStream } = require('internal/streams/utils');

function toWebReadable(readable, options = {}) {
  if (!isReadableNodeStream(readable)) throw new ERR_INVALID_ARG_TYPE('readable', 'stream.Readable', readable);
  const highWaterMark = readable.readableHighWaterMark;
  const strategy = options.strategy ?? new CountQueuingStrategy({ highWaterMark });
  let controller;
  let finished = false;
  readable.pause();
  const cleanup = eos(readable, (error) => {
    cleanup();
    if (error && error.code === 'ERR_STREAM_PREMATURE_CLOSE') error = new AbortError();
    if (finished) return;
    finished = true;
    if (error) controller.error(error); else controller.close();
  });
  readable.on('data', (chunk) => {
    if (finished) return;
    controller.enqueue(chunk);
    if (controller.desiredSize <= 0) readable.pause();
  });
  return new ReadableStream({
    start(c) { controller = c; },
    pull() { readable.resume(); },
    cancel(reason) { finished = true; readable.destroy(reason instanceof Error ? reason : new AbortError(undefined, { cause: reason })); },
  }, strategy);
}

function fromWebReadable(webStream, options = {}) {
  if (!(webStream instanceof ReadableStream)) throw new ERR_INVALID_ARG_TYPE('readableStream', 'ReadableStream', webStream);
  if (webStream.locked) throw new TypeError('The ReadableStream is locked');
  const reader = webStream.getReader();
  let closed = false;
  const readable = new (require('internal/streams/readable'))({
    objectMode: options.objectMode,
    highWaterMark: options.highWaterMark,
    encoding: options.encoding,
    signal: options.signal,
    read() {
      reader.read().then(
        ({ done, value }) => { if (done) this.push(null); else this.push(value); },
        (error) => this.destroy(error));
    },
    destroy(error, callback) {
      const done = () => callback(error);
      if (closed) return done();
      reader.cancel(error).then(done, done);
    },
  });
  reader.closed.then(() => { closed = true; }, (error) => { closed = true; readable.destroy(error); });
  return readable;
}

function toWebWritable(writable, options = {}) {
  if (!isWritableNodeStream(writable)) throw new ERR_INVALID_ARG_TYPE('streamWritable', 'stream.Writable', writable);
  const highWaterMark = writable.writableHighWaterMark;
  const strategy = options.strategy ?? new CountQueuingStrategy({ highWaterMark });
  let controller;
  let finished = false;
  const cleanup = eos(writable, (error) => {
    cleanup();
    if (error && error.code === 'ERR_STREAM_PREMATURE_CLOSE') error = new AbortError();
    if (finished) return;
    finished = true;
    if (error) controller.error(error);
  });
  return new WritableStream({
    start(c) { controller = c; },
    write(chunk) {
      if (!writable.writableNeedDrain && writable.write(chunk)) return undefined;
      return new Promise((resolve, reject) => {
        const onDrain = () => { writable.off('error', onError); resolve(); };
        const onError = (error) => { writable.off('drain', onDrain); reject(error); };
        writable.once('drain', onDrain);
        writable.once('error', onError);
      });
    },
    close() {
      return new Promise((resolve, reject) => {
        eos(writable, { readable: false }, (error) => (error ? reject(error) : resolve()));
        writable.end();
      });
    },
    abort(reason) { writable.destroy(reason instanceof Error ? reason : new AbortError(undefined, { cause: reason })); },
  }, strategy);
}

function fromWebWritable(webStream, options = {}) {
  if (!(webStream instanceof WritableStream)) throw new ERR_INVALID_ARG_TYPE('writableStream', 'WritableStream', webStream);
  if (webStream.locked) throw new TypeError('The WritableStream is locked');
  const writer = webStream.getWriter();
  let closed = false;
  const writable = new (require('internal/streams/writable'))({
    objectMode: options.objectMode,
    highWaterMark: options.highWaterMark,
    decodeStrings: false,
    signal: options.signal,
    write(chunk, encoding, callback) {
      writer.ready.then(() => writer.write(chunk)).then(() => callback(), callback);
    },
    final(callback) { writer.close().then(() => { closed = true; callback(); }, callback); },
    destroy(error, callback) {
      const done = () => callback(error);
      if (closed) return done();
      writer.abort(error).then(done, done);
    },
  });
  writer.closed.then(() => { closed = true; }, (error) => { closed = true; writable.destroy(error); });
  return writable;
}

// Streams are Readable/Writable/Duplex constructors; Node adds these as statics.
module.exports = (stream) => {
  stream.Readable.toWeb = toWebReadable;
  stream.Readable.fromWeb = fromWebReadable;
  stream.Writable.toWeb = toWebWritable;
  stream.Writable.fromWeb = fromWebWritable;
  return stream;
};
