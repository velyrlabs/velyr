
  // ---- FormData --------------------------------------------------------------------------

  const parseMultipartOp = global.op_multipart_parse;
  delete global.op_multipart_parse;
  // Parsed bodies keep file bytes untouched, so text fields are decoded without BOM stripping.
  // Created on first use: the constructor calls a host function, and this script is snapshotted.
  let rawDecoderInstance;
  const rawDecoder = () => (rawDecoderInstance ??=new TextDecoder("utf-8", { ignoreBOM: true }));
  const formEntries = new WeakMap();

  const usv = (value) => String(value).toWellFormed();
  const crlf = (text) => text.replace(/\r\n|\r|\n/g, "\r\n");

  function entriesOf(form) {
    if (!formEntries.has(form)) throw new TypeError("Illegal invocation");
    return formEntries.get(form);
  }

  // The value stored for append(name, value, filename): strings stay text, Blobs become Files.
  function formValue(argc, value, filename) {
    if (argc < 2) throw new TypeError("FormData needs a name and a value");
    const isBlob = blobInternal.isBlob(value);
    if (argc > 2 && !isBlob) throw new TypeError("The filename argument needs a Blob value");
    if (!isBlob) return usv(value);
    if (value instanceof File && filename === undefined) return value;
    return new File([value], filename === undefined ? "blob" : usv(filename), {
      type: value.type,
      lastModified: value instanceof File ? value.lastModified : undefined,
    });
  }

  class FormData {
    constructor(form = undefined) {
      if (form !== undefined) throw new TypeError("FormData cannot be built from a form element");
      formEntries.set(this, []);
    }

    append(name, value, filename = undefined) {
      const stored = formValue(arguments.length, value, filename);
      entriesOf(this).push([usv(name), stored]);
    }

    delete(name) {
      const key = usv(name);
      const list = entriesOf(this);
      for (let i = list.length - 1; i >= 0; i--) if (list[i][0] === key) list.splice(i, 1);
    }

    get(name) {
      const key = usv(name);
      return entriesOf(this).find(([entry]) => entry === key)?.[1] ?? null;
    }

    getAll(name) {
      const key = usv(name);
      return entriesOf(this).filter(([entry]) => entry === key).map(([, value]) => value);
    }

    has(name) {
      const key = usv(name);
      return entriesOf(this).some(([entry]) => entry === key);
    }

    set(name, value, filename = undefined) {
      const stored = formValue(arguments.length, value, filename);
      const key = usv(name);
      const list = entriesOf(this);
      const first = list.findIndex(([entry]) => entry === key);
      if (first === -1) {
        list.push([key, stored]);
        return;
      }
      list[first] = [key, stored];
      for (let i = list.length - 1; i > first; i--) if (list[i][0] === key) list.splice(i, 1);
    }

    forEach(callback, thisArg = undefined) {
      if (typeof callback !== "function") throw new TypeError("FormData.forEach needs a function");
      for (const [name, value] of this) callback.call(thisArg, value, name, this);
    }

    *entries() {
      const list = entriesOf(this);
      for (let i = 0; i < list.length; i++) yield [list[i][0], list[i][1]];
    }

    *keys() {
      for (const [name] of this.entries()) yield name;
    }

    *values() {
      for (const [, value] of this.entries()) yield value;
    }

    [Symbol.iterator]() {
      return this.entries();
    }
  }
  defineTag(FormData, "FormData");
  enumerable(FormData, ["append", "delete", "get", "getAll", "has", "set", "forEach", "entries", "keys", "values"]);

  const escapeParam = (text) => crlf(text).replace(/\r/g, "%0D").replace(/\n/g, "%0A").replace(/"/g, "%22");

  // multipart/form-data (RFC 7578) for a FormData; file bytes are copied once.
  function encodeMultipart(form) {
    const boundary = `----VelyrFormBoundary${Math.random().toString(16).slice(2)}${Math.random().toString(16).slice(2)}`;
    const chunks = [];
    const text = (value) => chunks.push(encoder.encode(value));
    for (const [name, value] of entriesOf(form)) {
      const disposition = `--${boundary}\r\nContent-Disposition: form-data; name="${escapeParam(name)}"`;
      if (typeof value === "string") {
        text(`${disposition}\r\n\r\n${crlf(value)}\r\n`);
      } else {
        text(`${disposition}; filename="${escapeParam(value.name)}"\r\nContent-Type: ${value.type || "application/octet-stream"}\r\n\r\n`);
        chunks.push(blobInternal.readBytes(value));
        text("\r\n");
      }
    }
    text(`--${boundary}--\r\n`);
    const bytes = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.byteLength, 0));
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return { boundary, bytes };
  }

  function boundaryOf(params) {
    for (const param of params) {
      const [key, ...rest] = param.split("=");
      if (key.trim().toLowerCase() === "boundary") return rest.join("=").trim().replace(/^"(.*)"$/s, "$1");
    }
    return "";
  }

  // The body mixin's formData(): urlencoded or multipart, anything else is a TypeError.
  function parseFormData(bytes, contentType) {
    const [essence, ...params] = String(contentType ?? "").split(";");
    const type = essence.trim().toLowerCase();
    const form = new FormData();
    if (type === "application/x-www-form-urlencoded") {
      for (const [name, value] of new URLSearchParams(rawDecoder().decode(bytes))) form.append(name, value);
      return form;
    }
    const boundary = type === "multipart/form-data" ? boundaryOf(params) : "";
    if (boundary === "") throw new TypeError("The body is not multipart/form-data with a boundary or urlencoded");
    for (const [name, filename, fileType, data] of parseMultipartOp(bytes, boundary)) {
      form.append(name, filename === null ? rawDecoder().decode(data) : new File([data], filename, { type: fileType ?? "" }));
    }
    return form;
  }
