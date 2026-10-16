// Velyr.readFile/readTextFile/writeFile/writeTextFile/stat on top of the private fs ops. Evaluated
// once per isolate, after the runtime-ops bootstrap created `Velyr`. Reads resolve with an
// ArrayBuffer; writeFile hands its ArrayBuffer/ArrayBufferView straight to the op.
(function (global) {
  "use strict";

  const {
    op_fs_read: readOp,
    op_fs_write: writeOp,
    op_fs_stat: statOp,
    op_fs_watch_open: watchOpen,
    op_fs_watch_next: watchNext,
    op_fs_watch_close: watchClose,
  } = global;
  delete global.op_fs_read;
  delete global.op_fs_write;
  delete global.op_fs_stat;
  delete global.op_fs_watch_open;
  delete global.op_fs_watch_next;
  delete global.op_fs_watch_close;

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

  // Async iterator of `{ kind, paths }` (kind: create, modify, remove, rename, other), debounced
  // by 50 ms. The paths are authorized (FsRead) when it is created; leaving the loop, `return()`
  // or an aborted `signal` closes the watch.
  function watchFs(paths, options = {}) {
    const list = (Array.isArray(paths) ? paths : [paths]).map(toPath);
    const id = watchOpen(JSON.stringify(list), options.recursive === true);
    let open = true;
    const close = () => {
      if (open) {
        open = false;
        watchClose(id);
      }
    };
    const { signal } = options;
    if (signal) {
      if (signal.aborted) close();
      else signal.addEventListener("abort", close, { once: true });
    }
    const iterator = {
      async next() {
        if (!open) return { value: undefined, done: true };
        const text = await watchNext(id).catch((error) => {
          if (open) throw error;
          return null;
        });
        if (text === null) {
          close();
          return { value: undefined, done: true };
        }
        return { value: JSON.parse(text), done: false };
      },
      async return() {
        close();
        return { value: undefined, done: true };
      },
      close,
      [Symbol.asyncIterator]() {
        return iterator;
      },
    };
    return iterator;
  }

  const api = {
    watchFs,
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
