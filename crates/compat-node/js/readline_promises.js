'use strict';
// node:readline/promises: the same Interface, with a promise-returning question().
const { Interface: BaseInterface } = require('internal/readline');

class Interface extends BaseInterface {
  question(query, options) {
    return new Promise((resolve, reject) => {
      const signal = options && options.signal;
      if (signal && signal.aborted) return reject(signal.reason);
      try {
        super.question(query, resolve);
      } catch (error) {
        return reject(error);
      }
      if (signal) signal.addEventListener('abort', () => reject(signal.reason), { once: true });
    });
  }
}

function createInterface(input, output, completer, terminal) {
  return new Interface(input, output, completer, terminal);
}

module.exports = { Interface, createInterface };
