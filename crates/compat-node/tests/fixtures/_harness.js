// Tiny harness shared by the fixtures: `test`/`atest` register cases, `finish()` returns a promise
// of "ok N" or "FAIL <name> => <reason>" lines.
var node = (name) => globalThis[Symbol.for("velyr.node")].require(name);
var __cases = [];
function show(v) {
  if (typeof v === "string") return JSON.stringify(v);
  if (typeof v === "symbol") return String(v);
  if (typeof v === "bigint") return v + "n";
  try { return JSON.stringify(v) ?? String(v); } catch { return String(v); }
}
function test(name, fn) {
  try { fn(); __cases.push([name, null]); } catch (e) { __cases.push([name, String((e && e.stack) || e)]); }
}
function atest(name, fn) {
  __cases.push(Promise.resolve().then(fn).then(() => [name, null], (e) => [name, String((e && e.stack) || e)]));
}
function eq(actual, expected, label) {
  if (!Object.is(actual, expected)) throw new Error(`${label || "eq"}: got ${show(actual)}, expected ${show(expected)}`);
}
function same(actual, expected, label) { eq(JSON.stringify(actual), JSON.stringify(expected), label); }
function code(fn, expected, Type) {
  try { fn(); } catch (e) {
    eq(e.code, expected, "error code");
    if (Type) eq(e instanceof Type, true, "error type");
    return e;
  }
  throw new Error(`did not throw ${expected}`);
}
function finish() {
  return Promise.all(__cases).then((all) => {
    const bad = all.filter((r) => r[1]);
    return bad.length ? "FAIL " + bad.map((r) => `${r[0]} => ${r[1]}`).join("\n") : "ok " + all.length;
  });
}
