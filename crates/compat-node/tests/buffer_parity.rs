//! Buffer encode/decode parity across every encoding, plus the 64 KiB allocation pool.

mod common;

const PARITY: &str = r#"
const { Buffer } = node("buffer");
const big = "héllo wörld ".repeat((1 << 20) / 12);
const samples = { empty: "", a: "a", accent: "héllo", surrogate: "x\ud800y", big };
const encodings = ["utf8", "latin1", "ascii", "utf16le", "base64", "base64url", "hex"];
const expected = {
  utf8: (s) => s.replace(/[\ud800-\udfff]/g, "�"),
  latin1: (s) => s.replace(/[^\x00-\xff]/g, (c) => String.fromCharCode(c.charCodeAt(0) & 255)),
  ascii: (s) => s.replace(/[^\x00-\xff]/g, (c) => String.fromCharCode(c.charCodeAt(0) & 255))
    .replace(/[\x80-\xff]/g, (c) => String.fromCharCode(c.charCodeAt(0) & 127)),
  utf16le: (s) => s,
};
test("round trips every encoding", () => {
  for (const [name, text] of Object.entries(samples)) {
    for (const encoding of encodings) {
      if (["base64", "base64url", "hex"].includes(encoding)) {
        const bytes = Buffer.from(text, "utf8");
        eq(Buffer.from(bytes.toString(encoding), encoding).equals(bytes), true, `${name}/${encoding}`);
      } else {
        eq(Buffer.from(text, encoding).toString(encoding), expected[encoding](text), `${name}/${encoding}`);
      }
    }
  }
});
test("byteLength matches the encoded length", () => {
  for (const text of Object.values(samples)) {
    for (const encoding of encodings) eq(Buffer.byteLength(text, encoding), Buffer.from(text, encoding).length, encoding);
  }
});
test("write truncates on a character boundary and slices decode a window", () => {
  const target = Buffer.alloc(8);
  eq(target.write("a€€", 1, 4, "utf8"), 4);
  eq(target.toString("utf8", 1, 5), "a€");
  eq(target.write("a€", 0, 3, "utf8"), 1);
  eq(Buffer.from("héllo").toString("utf8", 1, 3), "é");
});
test("pool", () => {
  eq(Buffer.poolSize, 65536);
  const a = Buffer.allocUnsafe(100), b = Buffer.allocUnsafe(100);
  eq(a.buffer === b.buffer, true, "shared");
  eq(a.buffer.byteLength, 65536);
  eq(Buffer.alloc(100).buffer.byteLength, 100, "alloc is never pooled");
  eq(Buffer.alloc(100).every((byte) => byte === 0), true);
});
finish();
"#;

#[test]
fn buffer_parity() {
    common::run_fixture("buffer_parity.js", PARITY, false);
}
