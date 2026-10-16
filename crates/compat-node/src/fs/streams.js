'use strict';
// fs.ReadStream / fs.WriteStream on top of node:stream and the fd ops. Written for this runtime.
//
// The file is opened in _construct (so open errors, including permission denials, surface as the
// stream's 'error'), reads and writes go through the blocking pool one chunk at a time, and
// destroy() closes the descriptor.
//
// Known limits: no `fs` option (custom fs implementation), no `signal`, no `fsync` option.
const { Buffer } = require('buffer');
const { Readable, Writable } = require('stream');
const { runAsync, toPath, toMode } = require('internal/fs/util');

const CHUNK = 64 * 1024;

function openFile(stream, callback) {
  if (stream.fd !== null) return callback();
  runAsync('open', stream.path, stream.flags, stream.mode).then(([fd]) => {
    stream.fd = Number(fd.toString());
    stream.emit('open', stream.fd);
    stream.emit('ready');
    callback();
  }, callback);
}

function closeFile(stream, error, callback) {
  const fd = stream.fd;
  stream.fd = null;
  if (fd === null) return callback(error);
  runAsync('close', fd).then(() => callback(error), (closeError) => callback(error || closeError));
}

function streamOptions(options) {
  return typeof options === 'string' ? { encoding: options } : { ...options };
}

class ReadStream extends Readable {
  constructor(file, options) {
    options = streamOptions(options);
    super({
      ...options,
      highWaterMark: options.highWaterMark ?? CHUNK,
      autoDestroy: options.autoClose ?? true,
      emitClose: options.emitClose ?? true,
    });
    this.fd = options.fd ?? null;
    this.path = this.fd === null ? toPath(file) : file;
    this.flags = options.flags ?? 'r';
    this.mode = toMode(options.mode, 0o666);
    this.start = options.start;
    this.end = options.end ?? Infinity;
    this.pos = options.start ?? 0;
    this.bytesRead = 0;
    this.autoClose = options.autoClose ?? true;
  }

  _construct(callback) {
    openFile(this, callback);
  }

  _read(size) {
    const length = Math.min(size, this.end - this.pos + 1);
    if (length <= 0) {
      this.push(null);
      return;
    }
    runAsync('read', this.fd, length, this.start === undefined ? -1 : this.pos).then(([data]) => {
      if (data.length === 0) {
        this.push(null);
        return;
      }
      this.pos += data.length;
      this.bytesRead += data.length;
      this.push(data);
    }, (error) => this.destroy(error));
  }

  _destroy(error, callback) {
    closeFile(this, error, callback);
  }

  close(callback) {
    if (typeof callback === 'function') this.once('close', callback);
    this.destroy();
  }
}

class WriteStream extends Writable {
  constructor(file, options) {
    options = streamOptions(options);
    super({
      ...options,
      autoDestroy: options.autoClose ?? true,
      emitClose: options.emitClose ?? true,
    });
    this.fd = options.fd ?? null;
    this.path = this.fd === null ? toPath(file) : file;
    this.flags = options.flags ?? 'w';
    this.mode = toMode(options.mode, 0o666);
    this.start = options.start;
    this.pos = options.start;
    this.bytesWritten = 0;
    this.autoClose = options.autoClose ?? true;
  }

  _construct(callback) {
    openFile(this, callback);
  }

  _write(chunk, encoding, callback) {
    const data = typeof chunk === 'string' ? Buffer.from(chunk, encoding) : chunk;
    runAsync('write', this.fd, data, this.pos === undefined ? -1 : this.pos).then(() => {
      this.bytesWritten += data.length;
      if (this.pos !== undefined) this.pos += data.length;
      callback();
    }, callback);
  }

  _destroy(error, callback) {
    closeFile(this, error, callback);
  }

  close(callback) {
    if (typeof callback === 'function') this.once('close', callback);
    if (!this.writableEnded) this.end();
    else this.destroy();
  }
}

const createReadStream = (file, options) => new ReadStream(file, options);
const createWriteStream = (file, options) => new WriteStream(file, options);

module.exports = { ReadStream, WriteStream, createReadStream, createWriteStream };
