
  // ---- public classes -----------------------------------------------------------------------------
  const SUBTLE_METHODS = Object.keys(subtle);
  let allowConstruct = false;

  class SubtleCrypto {
    constructor() {
      if (!allowConstruct) throw new TypeError("Illegal constructor");
    }
  }
  for (const method of SUBTLE_METHODS) {
    Object.defineProperty(SubtleCrypto.prototype, method, {
      value: {
        [method](...args) {
          if (!(this instanceof SubtleCrypto)) {
            return Promise.reject(new TypeError("Illegal invocation: receiver is not a SubtleCrypto"));
          }
          return subtle[method](...args);
        },
      }[method],
      writable: true,
      configurable: true,
      enumerable: true,
    });
  }
  Object.defineProperty(SubtleCrypto.prototype, Symbol.toStringTag, { value: "SubtleCrypto", configurable: true });

  allowConstruct = true;
  const subtleInstance = new SubtleCrypto();
  allowConstruct = false;

  class Crypto {
    constructor() {
      if (!allowConstruct) throw new TypeError("Illegal constructor");
    }
    get subtle() {
      if (!(this instanceof Crypto)) throw new TypeError("Illegal invocation: receiver is not a Crypto");
      return subtleInstance;
    }
    getRandomValues(array) {
      if (!(this instanceof Crypto)) throw new TypeError("Illegal invocation: receiver is not a Crypto");
      if (arguments.length < 1) throw new TypeError("getRandomValues requires 1 argument");
      return getRandomValues(array);
    }
    randomUUID() {
      if (!(this instanceof Crypto)) throw new TypeError("Illegal invocation: receiver is not a Crypto");
      return randomUUID();
    }
  }
  Object.defineProperty(Crypto.prototype, Symbol.toStringTag, { value: "Crypto", configurable: true });

  allowConstruct = true;
  const cryptoInstance = new Crypto();
  allowConstruct = false;

  const define = (name, value) =>
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  define("Crypto", Crypto);
  define("SubtleCrypto", SubtleCrypto);
  define("CryptoKey", CryptoKey);
  Object.defineProperty(global, "crypto", { get: () => cryptoInstance, configurable: true, enumerable: true });
})(globalThis);
