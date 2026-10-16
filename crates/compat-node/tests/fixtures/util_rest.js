const util = node("util");
test("format", () => {
  eq(util.format("%s:%d:%i:%f", "a", "42", "7.9", "1.5"), "a:42:7:1.5");
  eq(util.format("%j %%", { a: 1 }), '{"a":1} %');
  eq(util.format("%o", [1]), "[ 1, [length]: 1 ]");
  eq(util.format("%O", { a: { b: 1 } }), "{ a: { b: 1 } }");
  eq(util.format("a", "b", 1, { x: 1 }), "a b 1 { x: 1 }");
  eq(util.format("%s"), "%s");
  eq(util.format("%s", 5n), "5n");
  eq(util.format("%c styled", "color: red"), " styled");
  eq(util.format(1, 2), "1 2");
  eq(util.formatWithOptions({ colors: true }, "%O", 1), "\u001b[33m1\u001b[39m");
});
test("stripVTControlCharacters", () => {
  eq(util.stripVTControlCharacters("\u001b[31mred\u001b[39m"), "red");
});
atest("promisify", async () => {
  const ok = util.promisify((x, cb) => cb(null, x + 1));
  eq(await ok(1), 2);
  const bad = util.promisify((cb) => cb(new Error("no")));
  let caught;
  await bad().catch((e) => { caught = e; });
  eq(caught.message, "no");
  const custom = () => 1;
  custom[util.promisify.custom] = () => Promise.resolve("c");
  eq(await util.promisify(custom)(), "c");
  code(() => util.promisify(1), "ERR_INVALID_ARG_TYPE", TypeError);
});
atest("promisify keeps kernel error identity", async () => {
  const kernel = Object.assign(new Error("denied"), { code: "VELYR_PERMISSION_DENIED" });
  const fn = util.promisify((cb) => cb(kernel));
  let caught;
  await fn().catch((e) => { caught = e; });
  eq(caught, kernel);
  eq(caught.code, "VELYR_PERMISSION_DENIED");
});
atest("callbackify", async () => {
  const cb = util.callbackify(async (x) => x * 2);
  eq(await new Promise((res) => cb(4, (err, v) => res([err, v]))).then((r) => r[1]), 8);
  const falsy = util.callbackify(() => Promise.reject(null));
  const err = await new Promise((res) => falsy((e) => res(e)));
  eq(err.code, "ERR_FALSY_VALUE_REJECTION");
  eq(err.reason, null);
});
test("inherits", () => {
  function A() {}
  A.prototype.hi = () => "hi";
  function B() {}
  util.inherits(B, A);
  eq(new B().hi(), "hi");
  eq(B.super_, A);
  code(() => util.inherits(1, A), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("deprecate warns once and forwards", () => {
  const seen = [];
  const original = globalThis.process;
  globalThis.process = { emitWarning: (m, t, c) => seen.push([m, t, c]) };
  try {
    const d = util.deprecate((a, b) => a + b, "old", "DEP_X");
    eq(d(1, 2), 3);
    d(1, 2);
    same(seen, [["old", "DeprecationWarning", "DEP_X"]]);
  } finally {
    globalThis.process = original;
  }
});
test("isDeepStrictEqual", () => {
  eq(util.isDeepStrictEqual({ a: [1, { b: 2 }] }, { a: [1, { b: 2 }] }), true);
  eq(util.isDeepStrictEqual([1], ["1"]), false);
  eq(util.isDeepStrictEqual(0, -0), false);
  eq(util.isDeepStrictEqual(NaN, NaN), true);
  eq(util.isDeepStrictEqual(new Map([[{ k: 1 }, 1]]), new Map([[{ k: 1 }, 1]])), true);
  eq(util.isDeepStrictEqual(new Set([{ a: 1 }]), new Set([{ a: 2 }])), false);
  eq(util.isDeepStrictEqual(new Date(1), new Date(1)), true);
  eq(util.isDeepStrictEqual({}, Object.create(null)), false);
  const a = { x: 1 }; a.self = a;
  const b = { x: 1 }; b.self = b;
  eq(util.isDeepStrictEqual(a, b), true);
  eq(util.isDeepStrictEqual(new Uint8Array([1]), new Uint8Array([1])), true);
});
test("parseArgs", () => {
  const r = util.parseArgs({
    args: ["-f", "--bar=b", "pos", "-vv", "--n", "x", "--n", "y"],
    options: { foo: { type: "boolean", short: "f" }, bar: { type: "string" }, v: { type: "boolean", short: "v", multiple: true }, n: { type: "string", multiple: true }, d: { type: "string", default: "dd" } },
    allowPositionals: true,
  });
  same(r.values, { foo: true, bar: "b", v: [true, true], n: ["x", "y"], d: "dd" });
  same(r.positionals, ["pos"]);
  code(() => util.parseArgs({ args: ["--nope"], options: {} }), "ERR_PARSE_ARGS_UNKNOWN_OPTION", TypeError);
  code(() => util.parseArgs({ args: ["x"], options: {} }), "ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL", TypeError);
  code(() => util.parseArgs({ args: ["--s"], options: { s: { type: "string" } } }), "ERR_PARSE_ARGS_INVALID_OPTION_VALUE", TypeError);
  const t = util.parseArgs({ args: ["--a", "--", "z"], options: { a: { type: "boolean" } }, allowPositionals: true, tokens: true });
  same(t.tokens.map((x) => x.kind), ["option", "option-terminator", "positional"]);
});
test("styleText", () => {
  eq(util.styleText("red", "x"), "x");
  eq(util.styleText("red", "x", { validateStream: false }), "\u001b[31mx\u001b[39m");
  eq(util.styleText(["bold", "red"], "x", { validateStream: false }), "\u001b[1m\u001b[31mx\u001b[39m\u001b[22m");
  code(() => util.styleText("nope", "x"), "ERR_INVALID_ARG_VALUE", TypeError);
  code(() => util.styleText("red", 1), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("types", () => {
  const t = util.types;
  eq(t.isPromise(Promise.resolve()), true);
  eq(t.isPromise({ then() {} }), false);
  eq(t.isRegExp(/a/), true);
  eq(t.isDate(new Date()), true);
  eq(t.isMap(new Map()), true);
  eq(t.isSet(new Map()), false);
  eq(t.isTypedArray(new Uint8Array()), true);
  eq(t.isUint8Array(new Uint8Array()), true);
  eq(t.isAsyncFunction(async () => {}), true);
  eq(t.isGeneratorFunction(function* () {}), true);
  eq(t.isNativeError(new TypeError("x")), true);
  eq(t.isBoxedPrimitive(Object(1n)), true);
  eq(t.isArrayBuffer(new ArrayBuffer(1)), true);
  eq(t.isWeakMap(new WeakMap()), true);
  eq(node("util/types"), util.types);
});
test("describe errors", () => {
  const e = code(() => util.inherits(1, function () {}), "ERR_INVALID_ARG_TYPE", TypeError);
  eq(e.message, 'The "ctor" argument must be of type function. Received type number (1)');
});
finish();
