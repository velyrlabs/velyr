const { StringDecoder } = node("string_decoder");
const { Buffer } = node("buffer");
const bytes = (...b) => Buffer.from(b);

test("module shape", () => {
  eq(node("string_decoder").StringDecoder, StringDecoder);
  eq(typeof StringDecoder.prototype.write, "function");
  eq(typeof StringDecoder.prototype.end, "function");
});
test("encoding names", () => {
  eq(new StringDecoder().encoding, "utf8");
  eq(new StringDecoder("utf-8").encoding, "utf8");
  eq(new StringDecoder("UCS2").encoding, "utf16le");
  eq(new StringDecoder("binary").encoding, "latin1");
  eq(new StringDecoder("base64url").encoding, "base64url");
  const e = code(() => new StringDecoder("nope"), "ERR_UNKNOWN_ENCODING", TypeError);
  eq(e.message, "Unknown encoding: nope");
});
test("utf8 multi-byte split across writes", () => {
  const d = new StringDecoder("utf8");
  eq(d.write(bytes(0xe2, 0x82)), "");
  eq(d.write(bytes(0xac)), "€");
  eq(d.end(), "");
  const euro = Buffer.from("€");
  const e = new StringDecoder();
  eq([...euro].map((b) => e.write(bytes(b))).join("|"), "||€");
  const emoji = Buffer.from("a\u{1F600}b");
  const f = new StringDecoder();
  eq([...emoji].map((b) => f.write(bytes(b))).join(""), "a\u{1F600}b");
  const g = new StringDecoder();
  eq(g.write(emoji.subarray(0, 3)) + g.write(emoji.subarray(3)), "a\u{1F600}b");
});
test("utf8 incomplete input and invalid bytes", () => {
  const d = new StringDecoder();
  eq(d.write(bytes(0xe2, 0x82)), "");
  eq(d.end(), "�");
  eq(d.end(), "");
  const e = new StringDecoder();
  eq(e.write(bytes(0xe2)), "");
  eq(e.write(bytes(0x61)), "�a");
  eq(new StringDecoder().write(bytes(0xff, 0x62)), "�b");
  const f = new StringDecoder();
  eq(f.write(bytes(0x61, 0xe2)), "a");
  eq(f.end(bytes(0x82, 0xac)), "€");
  eq(new StringDecoder().end(Buffer.from("hi")), "hi");
});
test("utf16le with surrogate pairs and odd bytes", () => {
  const d = new StringDecoder("utf16le");
  const parts = [0x3d, 0xd8, 0x00, 0xde].map((b) => d.write(bytes(b)));
  eq(parts.join("|"), "|||\u{1F600}");
  const e = new StringDecoder("utf16le");
  eq(e.write(bytes(0x61)), "");
  eq(e.write(bytes(0x00, 0x62, 0x00)), "ab");
  const f = new StringDecoder("ucs2");
  eq(f.write(bytes(0x3d, 0xd8)), "");
  eq(f.end(), "\ud83d");
  const g = new StringDecoder("utf16le");
  eq(g.write(bytes(0x61, 0x00, 0x3d, 0xd8)), "a");
  eq(g.write(bytes(0x00, 0xde)), "\u{1F600}");
});
test("base64 and base64url hold partial groups", () => {
  const d = new StringDecoder("base64");
  eq(d.write(Buffer.from("ab")), "");
  eq(d.write(Buffer.from("c")), "YWJj");
  eq(d.write(Buffer.from("de")), "");
  eq(d.end(), "ZGU=");
  const e = new StringDecoder("base64");
  eq(e.write(Buffer.from("abcd")), "YWJj");
  eq(e.end(), "ZA==");
  const u = new StringDecoder("base64url");
  eq(u.write(Buffer.from([0xfb, 0xff, 0xfe, 0xfb])), "-__-");
  eq(u.end(), "-w");
  eq(new StringDecoder("base64").end(), "");
});
test("stateless encodings", () => {
  eq(new StringDecoder("hex").write(bytes(1, 0xab)), "01ab");
  eq(new StringDecoder("latin1").write(bytes(0xe9)), "é");
  eq(new StringDecoder("ascii").write(bytes(0xe9)), "i");
  eq(new StringDecoder("hex").end(bytes(0xff)), "ff");
});
test("input types", () => {
  const d = new StringDecoder();
  eq(d.write("plain"), "plain");
  eq(d.write(new Uint8Array([104, 105])), "hi");
  eq(d.write(new DataView(new Uint8Array([104]).buffer)), "h");
  const e = code(() => d.write(1), "ERR_INVALID_ARG_TYPE", TypeError);
  eq(e.message.startsWith('The "buf" argument must be an instance of Buffer, TypedArray, or DataView.'), true);
  code(() => d.end(1), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("lastNeed and lastTotal", () => {
  const d = new StringDecoder();
  d.write(bytes(0xe2));
  eq(d.lastNeed, 2);
  eq(d.lastTotal, 3);
  d.write(bytes(0x82, 0xac));
  eq(d.lastNeed, 0);
});
finish();
