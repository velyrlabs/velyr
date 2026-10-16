'use strict';
// fs.promises.FileHandle over a descriptor of the Rust fd table. Written for this runtime.
//
// Known limits: readFile() reads in 64 KiB chunks from the current position until EOF; there is
// no truncate/sync/readv/writev, and a handle that is never closed is released when the isolate
// shuts down (the resource table closes every entry).
const { Buffer } = require('buffer');
const { plans, execAsync } = require('internal/fs/ops');
const { getOptions } = require('internal/fs/util');

const CHUNK = 64 * 1024;

class FileHandle {
  constructor(fd) {
    Object.defineProperty(this, '_fd', { value: fd, writable: true });
  }

  get fd() {
    return this._fd;
  }

  async read(buffer, offset, length, position) {
    if (buffer === undefined || (buffer !== null && typeof buffer === 'object' && !ArrayBuffer.isView(buffer))) {
      ({ buffer = Buffer.alloc(CHUNK), offset, length, position } = buffer || {});
    }
    const bytesRead = await execAsync(plans.read(this._fd, buffer, offset, length, position));
    return { bytesRead, buffer };
  }

  async write(data, offset, length, position) {
    const bytesWritten = await execAsync(plans.write(this._fd, data, offset, length, position));
    return { bytesWritten, buffer: data };
  }

  stat() {
    return execAsync(plans.fstat(this._fd));
  }

  async readFile(options) {
    const { encoding } = getOptions(options);
    const chunks = [];
    for (;;) {
      const chunk = Buffer.alloc(CHUNK);
      const count = await execAsync(plans.read(this._fd, chunk, 0, CHUNK, null));
      if (count === 0) break;
      chunks.push(chunk.subarray(0, count));
    }
    const all = Buffer.concat(chunks);
    return encoding && encoding !== 'buffer' ? all.toString(encoding) : all;
  }

  async writeFile(data, options) {
    const { encoding } = getOptions(options);
    await execAsync(plans.write(this._fd, data, null, encoding));
  }

  appendFile(data, options) {
    return this.writeFile(data, options);
  }

  async close() {
    const fd = this._fd;
    if (fd === -1) return;
    this._fd = -1;
    await execAsync(plans.close(fd));
  }
}

module.exports = { FileHandle };
