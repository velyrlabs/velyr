// 200000 Buffer.from + toString round trips (utf8, base64, hex); prints nothing.
const text = "héllo wörld ".repeat(8);
let sink = 0;
for (let i = 0; i < 200000; i++) {
  const buf = Buffer.from(text);
  sink += buf.toString("utf8").length + buf.toString("base64").length + buf.toString("hex").length;
}
if (sink === 0) throw new Error("unreachable");
