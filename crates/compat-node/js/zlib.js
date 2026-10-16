'use strict';
// node:zlib. Written for this runtime; no Node source is copied.
//
// Streams (Gzip, Gunzip, Deflate, Inflate, DeflateRaw, InflateRaw, BrotliCompress,
// BrotliDecompress) keep a native codec between writes (zlib_ops.rs), so they work chunk by chunk
// without buffering the whole input. The sync forms run on the isolate thread; the callback forms
// run on the blocking pool. Every decompression has an output ceiling (`maxOutputLength`, default
// 256 MiB; streams apply it per input slice) that fails with ERR_BUFFER_TOO_LARGE: the zip-bomb guard.
//
// Known limits: `level`, `windowBits` and `dictionary` are applied (flate2 on zlib-rs; no
// dictionary for gzip); `memLevel`, `strategy` and `finishFlush` are validated and ignored (flate2
// exposes none of them); `params()` changes the level but, once data has been written, only within
// one algorithm family (0, 1, 2, 3-6, 7-9); brotli supports `quality` only (default 4, see
// DEFAULT_QUALITY); the stream `flush()` is a sync flush for every kind; and an abandoned stream
// is freed by a finalizer rather than at once. zstd is decompression only (ruzstd): the stream
// buffers its input and emits at the end, `params` and `dictionary` are not applied, and every
// zstd compression entry point throws ERR_VELYR_UNSUPPORTED.
const errors = require('internal/errors');
const ops = require('internal/ops');
const { Buffer } = require('buffer');
const { Transform } = require('stream');

const MAX_OUTPUT = 256 * 1024 * 1024;
const SLICE = 16 * 1024;
const BROTLI_PARAM_QUALITY = 1;
// Quality used when the caller sets none. Node's constant says 11, but 11 is several times slower
// and needs far more memory; 4 compresses well enough at stream speeds. Pass the quality to override.
const DEFAULT_QUALITY = 4;
const NO_BYTES = new Uint8Array(0);

const constants = Object.freeze({
  Z_NO_FLUSH: 0, Z_PARTIAL_FLUSH: 1, Z_SYNC_FLUSH: 2, Z_FULL_FLUSH: 3, Z_FINISH: 4, Z_BLOCK: 5,
  Z_OK: 0, Z_STREAM_END: 1, Z_NEED_DICT: 2, Z_ERRNO: -1, Z_STREAM_ERROR: -2, Z_DATA_ERROR: -3,
  Z_MEM_ERROR: -4, Z_BUF_ERROR: -5, Z_VERSION_ERROR: -6,
  Z_NO_COMPRESSION: 0, Z_BEST_SPEED: 1, Z_BEST_COMPRESSION: 9, Z_DEFAULT_COMPRESSION: -1,
  Z_FILTERED: 1, Z_HUFFMAN_ONLY: 2, Z_RLE: 3, Z_FIXED: 4, Z_DEFAULT_STRATEGY: 0, ZLIB_VERNUM: 4880,
  DEFLATE: 1, INFLATE: 2, GZIP: 3, GUNZIP: 4, DEFLATERAW: 5, INFLATERAW: 6, UNZIP: 7,
  BROTLI_DECODE: 8, BROTLI_ENCODE: 9,
  Z_MIN_WINDOWBITS: 8, Z_MAX_WINDOWBITS: 15, Z_DEFAULT_WINDOWBITS: 15,
  Z_MIN_CHUNK: 64, Z_MAX_CHUNK: Infinity, Z_DEFAULT_CHUNK: 16384,
  Z_MIN_MEMLEVEL: 1, Z_MAX_MEMLEVEL: 9, Z_DEFAULT_MEMLEVEL: 8,
  Z_MIN_LEVEL: -1, Z_MAX_LEVEL: 9, Z_DEFAULT_LEVEL: -1,
  BROTLI_OPERATION_PROCESS: 0, BROTLI_OPERATION_FLUSH: 1, BROTLI_OPERATION_FINISH: 2, BROTLI_OPERATION_EMIT_METADATA: 3,
  BROTLI_PARAM_MODE: 0, BROTLI_MODE_GENERIC: 0, BROTLI_MODE_TEXT: 1, BROTLI_MODE_FONT: 2, BROTLI_DEFAULT_MODE: 0,
  BROTLI_PARAM_QUALITY, BROTLI_MIN_QUALITY: 0, BROTLI_MAX_QUALITY: 11, BROTLI_DEFAULT_QUALITY: 11,
  BROTLI_PARAM_LGWIN: 2, BROTLI_MIN_WINDOW_BITS: 10, BROTLI_MAX_WINDOW_BITS: 24, BROTLI_DEFAULT_WINDOW: 22,
  BROTLI_PARAM_LGBLOCK: 3, BROTLI_PARAM_SIZE_HINT: 5,
  ZSTD_COMPRESS: 10, ZSTD_DECOMPRESS: 11, ZSTD_e_continue: 0, ZSTD_e_flush: 1, ZSTD_e_end: 2,
  ZSTD_fast: 1, ZSTD_dfast: 2, ZSTD_greedy: 3, ZSTD_lazy: 4, ZSTD_lazy2: 5, ZSTD_btlazy2: 6, ZSTD_btopt: 7,
  ZSTD_btultra: 8, ZSTD_btultra2: 9, ZSTD_c_compressionLevel: 100, ZSTD_c_windowLog: 101, ZSTD_c_hashLog: 102,
  ZSTD_c_chainLog: 103, ZSTD_c_searchLog: 104, ZSTD_c_minMatch: 105, ZSTD_c_targetLength: 106, ZSTD_c_strategy: 107,
  ZSTD_c_enableLongDistanceMatching: 160, ZSTD_c_ldmHashLog: 161, ZSTD_c_ldmMinMatch: 162,
  ZSTD_c_ldmBucketSizeLog: 163, ZSTD_c_ldmHashRateLog: 164, ZSTD_c_contentSizeFlag: 200, ZSTD_c_checksumFlag: 201,
  ZSTD_c_dictIDFlag: 202, ZSTD_c_nbWorkers: 400, ZSTD_c_jobSize: 401, ZSTD_c_overlapLog: 402,
  ZSTD_d_windowLogMax: 100, ZSTD_CLEVEL_DEFAULT: 3,
  ZSTD_error_no_error: 0, ZSTD_error_GENERIC: 1, ZSTD_error_prefix_unknown: 10, ZSTD_error_version_unsupported: 12,
  ZSTD_error_frameParameter_unsupported: 14, ZSTD_error_frameParameter_windowTooLarge: 16,
  ZSTD_error_corruption_detected: 20, ZSTD_error_checksum_wrong: 22, ZSTD_error_literals_headerWrong: 24,
  ZSTD_error_dictionary_corrupted: 30, ZSTD_error_dictionary_wrong: 32, ZSTD_error_dictionaryCreation_failed: 34,
  ZSTD_error_parameter_unsupported: 40, ZSTD_error_parameter_combination_unsupported: 41,
  ZSTD_error_parameter_outOfBound: 42, ZSTD_error_tableLog_tooLarge: 44, ZSTD_error_maxSymbolValue_tooLarge: 46,
  ZSTD_error_maxSymbolValue_tooSmall: 48, ZSTD_error_stabilityCondition_notRespected: 50, ZSTD_error_stage_wrong: 60,
  ZSTD_error_init_missing: 62, ZSTD_error_memory_allocation: 64, ZSTD_error_workSpace_tooSmall: 66,
  ZSTD_error_dstSize_tooSmall: 70, ZSTD_error_srcSize_wrong: 72, ZSTD_error_dstBuffer_null: 74,
  ZSTD_error_noForwardProgress_destFull: 80, ZSTD_error_noForwardProgress_inputEmpty: 82,
});
const codes = Object.freeze({
  Z_OK: 0, Z_STREAM_END: 1, Z_NEED_DICT: 2, Z_ERRNO: -1, Z_STREAM_ERROR: -2, Z_DATA_ERROR: -3,
  Z_MEM_ERROR: -4, Z_BUF_ERROR: -5, Z_VERSION_ERROR: -6,
  0: 'Z_OK', 1: 'Z_STREAM_END', 2: 'Z_NEED_DICT', '-1': 'Z_ERRNO', '-2': 'Z_STREAM_ERROR', '-3': 'Z_DATA_ERROR',
  '-4': 'Z_MEM_ERROR', '-5': 'Z_BUF_ERROR', '-6': 'Z_VERSION_ERROR',
});

// ---- host op plumbing (same framed-result protocol as crypto_ops.rs) -----------------------------
const text = (value) => Buffer.from(String(value));

function failure(message) {
  const match = /^([A-Z][A-Z0-9_]+)\|([\s\S]*)$/.exec(message);
  if (!match) return new Error(message);
  const [, code, detail] = match;
  if (code === 'ERR_BUFFER_TOO_LARGE') return errors.make(RangeError, code, detail);
  const error = new Error(detail);
  error.errno = codes[code];
  error.code = code;
  return error;
}

function unframe(buffer) {
  const bytes = new Uint8Array(buffer);
  const parts = [];
  for (let at = 1; at < bytes.length;) {
    const size = ((bytes[at] << 24) | (bytes[at + 1] << 16) | (bytes[at + 2] << 8) | bytes[at + 3]) >>> 0;
    parts.push(Buffer.from(buffer, at + 4, size));
    at += 4 + size;
  }
  if (bytes[0] === 1) throw failure(parts[0].toString());
  return parts;
}

const call = (name, ...parts) => unframe(ops.zlibSync(name, ...parts));
const callAsync = (name, ...parts) => ops.zlibAsync(name, ...parts).then(unframe);

const registry = typeof FinalizationRegistry === 'function'
  ? new FinalizationRegistry((id) => { try { call('zlib_free', text(id)); } catch { /* already freed */ } })
  : undefined;

const unsupportedZstd = (what) => errors.make(Error, 'ERR_VELYR_UNSUPPORTED', `zlib zstd ${what} ${what === 'dictionaries' ? 'are' : 'is'} not supported by this runtime: only decompression is`);

// ---- options -------------------------------------------------------------------------------------
const KINDS = {
  Gzip: 'gzip', Gunzip: 'gunzip', Deflate: 'deflate', Inflate: 'inflate', DeflateRaw: 'deflateRaw',
  InflateRaw: 'inflateRaw', Unzip: 'unzip', BrotliCompress: 'brotliCompress', BrotliDecompress: 'brotliDecompress',
  ZstdDecompress: 'zstdDecompress',
};

function toInput(buffer) {
  if (typeof buffer === 'string') return Buffer.from(buffer);
  if (buffer instanceof ArrayBuffer) return new Uint8Array(buffer);
  if (ArrayBuffer.isView(buffer)) return new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
  throw errors.invalidArgType('buffer', ['string', 'Buffer', 'TypedArray', 'DataView', 'ArrayBuffer'], buffer);
}

function checkRange(value, name, min, max) {
  if (value === undefined) return;
  if (typeof value !== 'number') throw errors.invalidArgType(`options.${name}`, 'number', value);
  if (!Number.isInteger(value) || value < min || value > max) throw errors.outOfRange(`options.${name}`, `>= ${min} and <= ${max}`, value);
}

const DECODERS = new Set(['gunzip', 'inflate', 'inflateRaw', 'unzip', 'zstdDecompress']);

/** Validates `options` and returns what the native codec takes. */
function settings(kind, options) {
  if (options === undefined || options === null) options = {};
  errors.validateObject(options, 'options');
  let dictionary = NO_BYTES;
  if (kind === 'zstdDecompress' && options.dictionary !== undefined) throw unsupportedZstd('dictionaries');
  if (options.dictionary !== undefined) {
    const { dictionary: given } = options;
    if (!ArrayBuffer.isView(given) && !(given instanceof ArrayBuffer)) {
      throw errors.invalidArgType('options.dictionary', ['Buffer', 'TypedArray', 'DataView', 'ArrayBuffer'], given);
    }
    dictionary = toInput(given);
  }
  checkRange(options.level, 'level', -1, 9);
  // Decoders also take 0, meaning "whatever the stream header says" (we use the maximum window).
  checkRange(options.windowBits, 'windowBits', DECODERS.has(kind) ? 0 : 8, 15);
  checkRange(options.memLevel, 'memLevel', 1, 9);
  checkRange(options.chunkSize, 'chunkSize', 64, Number.MAX_SAFE_INTEGER);
  checkRange(options.maxOutputLength, 'maxOutputLength', 1, Number.MAX_SAFE_INTEGER);
  let quality = DEFAULT_QUALITY;
  if (kind === 'brotliCompress' && options.params !== undefined) {
    errors.validateObject(options.params, 'options.params');
    const wanted = options.params[BROTLI_PARAM_QUALITY];
    if (wanted !== undefined) { checkRange(wanted, 'params[BROTLI_PARAM_QUALITY]', 0, 11); quality = wanted; }
  }
  return {
    level: options.level === undefined ? -1 : options.level,
    quality,
    limit: options.maxOutputLength === undefined ? MAX_OUTPUT : options.maxOutputLength,
    windowBits: options.windowBits === undefined ? 15 : options.windowBits,
    dictionary,
  };
}

/** The codec arguments every native call takes after the kind, in wire order. */
const shared = ({ level, quality, limit, windowBits, dictionary }) =>
  [text(level), text(quality), text(limit), text(windowBits), dictionary];

const onceArgs = (kind, options, buffer) => ['zlib_once', text(kind), ...shared(settings(kind, options)), toInput(buffer)];

function oneShot(kind) {
  const sync = (buffer, options) => call(...onceArgs(kind, options, buffer))[0];
  const callback = (buffer, options, done) => {
    if (typeof options === 'function') [done, options] = [options, undefined];
    errors.validateFunction(done, 'callback');
    let args;
    try { args = onceArgs(kind, options, buffer); } catch (error) { require('process').nextTick(done, error); return; }
    const process = require('process');
    callAsync(...args).then(([out]) => process.nextTick(done, null, out), (error) => process.nextTick(done, error));
  };
  return [sync, callback];
}

// ---- streams -------------------------------------------------------------------------------------
class ZlibStream extends Transform {
  #kind;
  #settings;
  #id;
  #written = 0;

  constructor(kind, options) {
    super({ highWaterMark: options && options.chunkSize });
    this.#kind = kind;
    this.#settings = settings(kind, options);
  }

  get bytesWritten() { return this.#written; }

  #context() {
    if (this.#id === undefined) {
      this.#id = call('zlib_new', text(this.#kind), ...shared(this.#settings))[0].toString();
      if (registry) registry.register(this, this.#id, this);
    }
    return text(this.#id);
  }

  #release() {
    if (this.#id === undefined) return;
    if (registry) registry.unregister(this);
    try { call('zlib_free', text(this.#id)); } catch { /* finished already */ }
    this.#id = undefined;
  }

  _transform(chunk, encoding, callback) {
    try {
      const id = this.#context();
      const bytes = toInput(chunk);
      // Slices keep the output of one call bounded even for highly compressible data.
      for (let at = 0; at < bytes.length; at += SLICE) {
        const out = call('zlib_write', id, bytes.subarray(at, at + SLICE), text(0))[0];
        if (out.length > 0) this.push(out);
      }
      this.#written += bytes.length;
      callback();
    } catch (error) {
      callback(error);
    }
  }

  _flush(callback) {
    try {
      const out = call('zlib_write', this.#context(), NO_BYTES, text(2))[0];
      this.#id = undefined;
      if (registry) registry.unregister(this);
      if (out.length > 0) this.push(out);
      callback();
    } catch (error) {
      this.#release();
      callback(error);
    }
  }

  flush(kind, callback) {
    if (typeof kind === 'function') [callback, kind] = [kind, undefined];
    try {
      const out = call('zlib_write', this.#context(), NO_BYTES, text(1))[0];
      if (out.length > 0) this.push(out);
    } catch (error) {
      if (typeof callback !== 'function') throw error;
      require('process').nextTick(callback, error);
      return;
    }
    if (typeof callback === 'function') require('process').nextTick(callback);
  }

  /**
   * Flushes, then switches the compression level (`strategy` is validated and ignored). Once data
   * has gone through, the new level must be in the same algorithm family as the old one (see
   * Deflater::set_level); otherwise the callback gets Z_STREAM_ERROR.
   */
  params(level, strategy, callback) {
    if (this.#kind.startsWith('brotli') || DECODERS.has(this.#kind)) throw errors.make(Error, 'ERR_VELYR_UNSUPPORTED', 'params() is for deflate streams');
    checkRange(level, 'level', -1, 9);
    checkRange(strategy, 'strategy', 0, 4);
    errors.validateFunction(callback, 'callback');
    const process = require('process');
    try {
      const out = call('zlib_params', this.#context(), text(level))[0];
      this.#settings = { ...this.#settings, level };
      if (out.length > 0) this.push(out);
    } catch (error) {
      process.nextTick(callback, error);
      return;
    }
    process.nextTick(callback);
  }

  /** Drops codec state so the stream can start over (a new gzip member, a new brotli stream). */
  reset() {
    if (this.#id !== undefined) call('zlib_reset', text(this.#id));
  }

  close(callback) {
    if (typeof callback === 'function') this.once('close', callback);
    this.destroy();
  }

  _destroy(error, callback) {
    this.#release();
    callback(error);
  }
}

const classes = {};
for (const [name, kind] of Object.entries(KINDS)) {
  const stream = { [name]: class extends ZlibStream {
    constructor(options) { super(kind, options); }
  } }[name];
  classes[name] = stream;
}

// ---- exports -------------------------------------------------------------------------------------
function crc32(data, value = 0) {
  if (typeof data !== 'string' && !ArrayBuffer.isView(data)) throw errors.invalidArgType('data', ['string', 'Buffer', 'TypedArray', 'DataView'], data);
  if (typeof value !== 'number') throw errors.invalidArgType('value', 'number', value);
  if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) throw errors.outOfRange('value', '>= 0 and <= 4294967295', value);
  return Number(call('crc32', toInput(data), text(value))[0].toString());
}

const api = { constants, codes, crc32, ...classes };
for (const [name, kind] of Object.entries(KINDS)) {
  const base = name[0].toLowerCase() + name.slice(1);
  const [sync, callback] = oneShot(kind);
  api[base] = callback;
  api[`${base}Sync`] = sync;
  api[`create${name}`] = (options) => new classes[name](options);
}

// zstd compression is not offered (ruzstd decodes only): constructing or calling it fails loudly.
api.ZstdCompress = class ZstdCompress { constructor() { throw unsupportedZstd('compression'); } };
api.zstdCompress = api.zstdCompressSync = api.createZstdCompress = () => { throw unsupportedZstd('compression'); };

for (const [key, value] of Object.entries(constants)) {
  if (key.startsWith('Z_')) Object.defineProperty(api, key, { value, enumerable: false, configurable: true, writable: true });
}
module.exports = api;
