'use strict';
// internal/readline: the Interface shared by readline and readline/promises. Written for this runtime; no Node source is copied.
//
// Known limits: line mode only. There is no line editing, history, completion or raw-mode key
// handling, so `terminal` is ignored and the cursor helpers write nothing (they only honor the
// callback). Lines split on \n, \r\n and a lone \r. Lines emitted before an async iterator is
// requested are not replayed, as in Node.
const errors = require('internal/errors');
const EventEmitter = require('events');
const { StringDecoder } = require('string_decoder');

const LINE_BREAK = /\r\n|\n|\r/;

function useAfterClose() {
  return errors.make(Error, 'ERR_USE_AFTER_CLOSE', 'readline was closed');
}

class Interface extends EventEmitter {
  constructor(input, output, completer, terminal) {
    super();
    const options = input && typeof input.on !== 'function' && typeof input === 'object' ? input : { input, output, completer, terminal };
    if (!options.input || typeof options.input.on !== 'function') throw errors.invalidArgType('options.input', 'Readable', options.input);
    this.input = options.input;
    this.output = options.output;
    this.terminal = false;
    this.line = '';
    this.closed = false;
    this.paused = false;
    this._prompt = options.prompt === undefined ? '> ' : String(options.prompt);
    this._questions = [];
    this._pendingCr = false;
    this._decoder = new StringDecoder('utf8');
    this._onData = (chunk) => this._feed(typeof chunk === 'string' ? chunk : this._decoder.write(chunk));
    this._onEnd = () => this._finish();
    this.input.on('data', this._onData);
    this.input.on('end', this._onEnd);
  }

  get [Symbol.toStringTag]() { return 'Interface'; }

  _feed(text) {
    if (this.closed) return;
    if (this._pendingCr && text[0] === '\n') text = text.slice(1);
    this._pendingCr = text.endsWith('\r');
    const parts = (this.line + text).split(LINE_BREAK);
    this.line = parts.pop();
    for (const line of parts) {
      if (this.closed) return;
      this._dispatch(line);
    }
  }

  _dispatch(line) {
    const answer = this._questions.shift();
    if (answer) answer(line);
    else this.emit('line', line);
  }

  _finish() {
    if (this.closed) return;
    const rest = this.line + this._decoder.end();
    this.line = '';
    if (rest) this._dispatch(rest);
    this.close();
  }

  getPrompt() { return this._prompt; }
  setPrompt(prompt) { this._prompt = String(prompt); }

  prompt() {
    if (this.closed) throw useAfterClose();
    if (this.paused) this.resume();
    if (this.output) this.output.write(this._prompt);
  }

  question(query, options, callback) {
    if (typeof options === 'function') callback = options;
    if (typeof callback !== 'function') throw errors.invalidArgType('callback', 'Function', callback);
    if (this.closed) throw useAfterClose();
    if (this.output) this.output.write(String(query));
    this._questions.push(callback);
  }

  write(data) {
    if (this.closed) throw useAfterClose();
    if (data !== undefined && data !== null) this._feed(String(data));
  }

  pause() {
    if (this.paused) return this;
    this.paused = true;
    this.input.pause();
    this.emit('pause');
    return this;
  }

  resume() {
    if (!this.paused) return this;
    this.paused = false;
    this.input.resume();
    this.emit('resume');
    return this;
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    this.input.removeListener('data', this._onData);
    this.input.removeListener('end', this._onEnd);
    this.input.pause();
    this.emit('close');
  }

  [Symbol.asyncIterator]() {
    const queue = [];
    const waiting = [];
    let ended = this.closed;
    const onLine = (line) => (waiting.length ? waiting.shift().resolve({ value: line, done: false }) : queue.push(line));
    const onClose = () => {
      ended = true;
      while (waiting.length) waiting.shift().resolve({ value: undefined, done: true });
    };
    this.on('line', onLine);
    this.once('close', onClose);
    const stop = () => {
      this.removeListener('line', onLine);
      this.removeListener('close', onClose);
      this.close();
      return { value: undefined, done: true };
    };
    return {
      next: () => {
        if (queue.length) return Promise.resolve({ value: queue.shift(), done: false });
        if (ended) return Promise.resolve({ value: undefined, done: true });
        return new Promise((resolve) => waiting.push({ resolve }));
      },
      return: () => Promise.resolve(stop()),
      [Symbol.asyncIterator]() { return this; },
    };
  }
}

module.exports = { Interface };
