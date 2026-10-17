'use strict';
// node:readline. The Interface (line mode only) lives in internal/readline; see its limits.
const process = require('process');
const { Interface } = require('internal/readline');
const promises = require('readline/promises');

function createInterface(input, output, completer, terminal) {
  return new Interface(input, output, completer, terminal);
}

// Cursor helpers need a terminal. Without line editing there is nothing to move or clear, so they
// are safe no-ops that honor the callback contract and report "no backpressure".
function noop(stream, ...rest) {
  const callback = rest.find((arg) => typeof arg === 'function');
  if (callback) process.nextTick(callback, null);
  return true;
}

module.exports = {
  Interface,
  clearLine: noop,
  clearScreenDown: noop,
  createInterface,
  cursorTo: noop,
  emitKeypressEvents() {},
  moveCursor: noop,
  promises,
};
