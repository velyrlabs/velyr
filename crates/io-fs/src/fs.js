// Velyr.readFile/readTextFile/writeFile/writeTextFile/stat on top of the private fs ops. Evaluated
// once per isolate, after the runtime-ops bootstrap created `Velyr`. Reads resolve with an
// ArrayBuffer; writeFile hands its ArrayBuffer/ArrayBufferView straight to the op.
(function (global) {
  "use strict";

  const { op_fs_read: readOp, op_fs_write: writeOp, op_fs_stat: statOp } = global;
  delete global.op_fs_read;
  delete global.op_fs_write;
  delete global.op_fs_stat;

  function toPath(path) {
    if (typeof path === "string") return path;
    if (typeof URL !== "undefined" && path instanceof URL) {
      if (path.protocol !== "file:") throw new TypeError("Velyr fs: URL must use the file: scheme");
      // An encoded slash would change the path structure after decoding.
      if (/%2f/i.test(path.pathname)) throw new TypeError("Velyr fs: file URL contains an encoded slash");
      return decodeURIComponent(path.pathname);
    }
    throw new TypeError("Velyr fs: path must be a string or a file: URL");
  }

  const api = {
    async readFile(path) {
      return new Uint8Array(await readOp(toPath(path), false));
    },
    async readTextFile(path) {
      return readOp(toPath(path), true);
    },
    async writeFile(path, data) {
      await writeOp(toPath(path), data, false);
    },
    async writeTextFile(path, text) {
      await writeOp(toPath(path), String(text), true);
    },
    async stat(path) {
      const { size, isFile, isDirectory, mtimeMs } = JSON.parse(await statOp(toPath(path)));
      return { size, isFile, isDirectory, mtime: mtimeMs === null ? null : new Date(mtimeMs) };
    },
  };

  for (const [name, value] of Object.entries(api)) {
    Object.defineProperty(global.Velyr, name, {
      value,
      writable: false,
      configurable: true,
      enumerable: true,
    });
  }
})(globalThis);
