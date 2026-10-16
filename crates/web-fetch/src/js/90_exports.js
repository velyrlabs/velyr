
  for (const [name, value] of [["Headers", Headers], ["FormData", FormData], ["Request", Request], ["Response", Response], ["fetch", fetch]]) {
    Object.defineProperty(global, name, { value, writable: true, configurable: true, enumerable: false });
  }
})(globalThis);
