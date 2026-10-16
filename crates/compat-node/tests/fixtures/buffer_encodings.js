const { Buffer } = node("buffer");
const hex = (b) => b.toString("hex");
const roundTrip = (text, enc) => Buffer.from(text, enc).toString(enc);

test("utf8", () => {
  eq(hex(Buffer.from("héllo")), "68c3a96c6c6f");
  eq(hex(Buffer.from("€")), "e282ac");
  eq(hex(Buffer.from("\u{1F600}")), "f09f9880");
  eq(hex(Buffer.from("\ud800")), "efbfbd");
  eq(hex(Buffer.from("a\udc00b")), "61efbfbd62");
  eq(roundTrip("héllo € \u{1F600}", "utf8"), "héllo € \u{1F600}");
  eq(Buffer.from([0xff, 0x61]).toString(), "�a");
  eq(Buffer.from([0xe2, 0x82]).toString(), "�");
  eq(Buffer.from("x", "utf-8").toString("UTF8"), "x");
});
test("utf16le and ucs2", () => {
  eq(hex(Buffer.from("a€", "utf16le")), "6100ac20");
  eq(hex(Buffer.from("\u{1F600}", "ucs2")), "3dd800de");
  eq(roundTrip("a€\u{1F600}", "utf16le"), "a€\u{1F600}");
  eq(roundTrip("\ud800", "utf16le"), "\ud800");
  eq(roundTrip("\udc00x", "ucs-2"), "\udc00x");
  eq(Buffer.from([0x61, 0x00, 0x62]).toString("utf16le"), "a");
});
test("latin1, binary and ascii", () => {
  eq(hex(Buffer.from("éÿ", "latin1")), "e9ff");
  eq(hex(Buffer.from("ā", "binary")), "01");
  eq(roundTrip("café", "latin1"), "café");
  eq(Buffer.from([0xe9]).toString("ascii"), "i");
  eq(hex(Buffer.from("hié", "ascii")), "6869e9");
  eq(roundTrip("plain", "ascii"), "plain");
});
test("base64 and base64url", () => {
  eq(Buffer.from("hello world").toString("base64"), "aGVsbG8gd29ybGQ=");
  eq(Buffer.from("aGVsbG8gd29ybGQ", "base64").toString(), "hello world");
  eq(Buffer.from("aGVsbG8gd29ybGQ=", "base64").toString(), "hello world");
  eq(Buffer.from("a").toString("base64"), "YQ==");
  eq(Buffer.from("a").toString("base64url"), "YQ");
  eq(Buffer.from("YQ", "base64").toString(), "a");
  eq(Buffer.from("YWI", "base64url").toString(), "ab");
  eq(Buffer.from("").toString("base64"), "");
  const bytes = Buffer.from([0xfb, 0xff, 0xfe]);
  eq(bytes.toString("base64"), "+//+");
  eq(bytes.toString("base64url"), "-__-");
  eq(hex(Buffer.from("-__-", "base64url")), "fbfffe");
  eq(hex(Buffer.from("+//+", "base64url")), "fbfffe");
  eq(hex(Buffer.from("-__-", "base64")), "fbfffe");
  eq(Buffer.from(" aGVs\nbG8= ", "base64").toString(), "hello");
  eq(Buffer.from("YQ==YQ==", "base64").toString(), "a");
  eq(Buffer.from("Y", "base64").length, 0);
  for (let n = 0; n < 8; n++) {
    const b = Buffer.from(Array.from({ length: n }, (_, i) => i * 37 + 200));
    eq(hex(Buffer.from(b.toString("base64"), "base64")), hex(b), "base64 round trip " + n);
    eq(hex(Buffer.from(b.toString("base64url"), "base64url")), hex(b), "base64url round trip " + n);
  }
});
test("hex", () => {
  eq(Buffer.from("00ff7f", "hex").length, 3);
  eq(hex(Buffer.from("DEADbeef", "hex")), "deadbeef");
  eq(Buffer.from("abc", "hex").length, 1);
  eq(Buffer.from("zz", "hex").length, 0);
  eq(hex(Buffer.from("abzz", "hex")), "ab");
  eq(Buffer.from([1, 171, 255]).toString("hex"), "01abff");
});
test("every encoding round trips every byte", () => {
  const all = Buffer.from(Array.from({ length: 256 }, (_, i) => i));
  for (const enc of ["hex", "base64", "base64url", "latin1", "binary"]) {
    eq(hex(Buffer.from(all.toString(enc), enc)), hex(all), enc);
  }
  eq(hex(Buffer.from(all.toString("utf16le"), "utf16le")), hex(all), "utf16le");
});
test("toString ranges and unknown encodings", () => {
  const b = Buffer.from("hello");
  eq(b.toString("utf8", 1, 3), "el");
  eq(b.toString("utf8", 2), "llo");
  eq(b.toString("utf8", -5, 99), "hello");
  eq(b.toString("utf8", 4, 2), "");
  eq(Buffer.alloc(0).toString(), "");
  const e = code(() => b.toString("nope"), "ERR_UNKNOWN_ENCODING", TypeError);
  eq(e.message, "Unknown encoding: nope");
  code(() => Buffer.from("x", "nope"), "ERR_UNKNOWN_ENCODING", TypeError);
  eq(Buffer.isEncoding("UCS-2"), true);
  eq(Buffer.isEncoding("nope"), false);
  eq(Buffer.isEncoding(1), false);
});
test("byteLength", () => {
  eq(Buffer.byteLength("€"), 3);
  eq(Buffer.byteLength("€", "utf16le"), 2);
  eq(Buffer.byteLength("aGk=", "base64"), 2);
  eq(Buffer.byteLength("6869", "hex"), 2);
  eq(Buffer.byteLength(new Uint16Array(3)), 6);
  eq(Buffer.byteLength(new ArrayBuffer(7)), 7);
  eq(Buffer.byteLength("\ud800"), 3);
  code(() => Buffer.byteLength(1), "ERR_INVALID_ARG_TYPE", TypeError);
});
finish();
