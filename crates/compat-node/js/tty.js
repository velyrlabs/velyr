'use strict';
// node:tty. Written for this runtime; no Node source is copied. `isatty` asks the OS through the
// os op (std::io::IsTerminal), so it needs no `unsafe`.
//
// Known limits: isatty answers only for fds 0, 1 and 2 (any other fd is false); ReadStream (fd 0)
// is the empty stdin of this runtime and setRawMode only records the flag; WriteStream (fd 1 or 2)
// writes through process.stdout/stderr and does not check that the fd is a terminal; the window
// size comes from COLUMNS/LINES when the env is readable, else 80x24 (no ioctl without unsafe);
// getColorDepth covers FORCE_COLOR, NO_COLOR/NODE_DISABLE_COLORS, TERM and COLORTERM, not the CI
// or terminal-program heuristics; cursor and line helpers are not provided.
const errors = require('internal/errors');
const ops = require('internal/ops');
const process = require('process');
const { Readable, Writable } = require('stream');

const isatty = (fd) => Number.isInteger(fd) && fd >= 0 && fd <= 2 && ops.os(`isatty:${fd}`) === true;

// Reading `process.env` throws when the env permission is denied: no env means "not set".
function readEnv(name) {
  try { return process.env[name]; } catch { return undefined; }
}
const COLOR_KEYS = ['FORCE_COLOR', 'NO_COLOR', 'NODE_DISABLE_COLORS', 'TERM', 'COLORTERM'];
const sampleEnv = () => Object.fromEntries(COLOR_KEYS.map((key) => [key, readEnv(key)]));

const FORCE_DEPTH = { '': 4, 1: 4, true: 4, 2: 8, 3: 24 };

function colorDepth(env) {
  if (env.FORCE_COLOR !== undefined) return FORCE_DEPTH[env.FORCE_COLOR] || 1;
  if (env.NODE_DISABLE_COLORS || (env.NO_COLOR !== undefined && env.NO_COLOR !== '') || env.TERM === 'dumb') return 1;
  if (env.COLORTERM === 'truecolor' || env.COLORTERM === '24bit') return 24;
  if (env.TERM) {
    if (/^xterm-256/.test(env.TERM)) return 8;
    if (/^screen|^xterm|^vt100|^vt220|^rxvt|color|ansi|cygwin|linux/i.test(env.TERM)) return 4;
  }
  return env.COLORTERM ? 4 : 1;
}

function validateFd(fd, allowed) {
  if (!Number.isInteger(fd) || fd < 0) throw errors.invalidArgType('fd', 'number', fd);
  if (!allowed.includes(fd)) {
    throw errors.make(Error, 'ERR_VELYR_UNSUPPORTED', `tty fd ${fd} is not supported by this runtime: only fd ${allowed.join(' or ')} is`);
  }
}

function dimension(name, fallback) {
  const value = Number(readEnv(name));
  return Number.isInteger(value) && value > 0 ? value : fallback;
}

class ReadStream extends Readable {
  constructor(fd, options) {
    validateFd(fd, [0]);
    super({ ...options, read() { this.push(null); } });
    this.fd = fd;
    this.isRaw = false;
    this.isTTY = true;
  }

  setRawMode(mode) {
    this.isRaw = !!mode;
    return this;
  }
}

class WriteStream extends Writable {
  constructor(fd) {
    validateFd(fd, [1, 2]);
    super({
      decodeStrings: false,
      write(chunk, encoding, callback) {
        const target = fd === 1 ? process.stdout : process.stderr;
        target.write(chunk, encoding, callback);
      },
    });
    this.fd = fd;
    this.isTTY = true;
    this.columns = dimension('COLUMNS', 80);
    this.rows = dimension('LINES', 24);
  }

  getWindowSize() { return [this.columns, this.rows]; }
  getColorDepth(env = sampleEnv()) { return colorDepth(env); }

  hasColors(count, env) {
    if (env === undefined && count !== undefined && typeof count === 'object') {
      env = count;
      count = undefined;
    }
    return (count === undefined ? 16 : count) <= 2 ** this.getColorDepth(env);
  }
}

module.exports = { ReadStream, WriteStream, isatty };
