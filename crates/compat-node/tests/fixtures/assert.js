const assert = node("assert");
const strict = node("assert/strict");
const fails = (fn, props) => {
  try { fn(); } catch (e) {
    eq(e instanceof assert.AssertionError, true, "AssertionError");
    eq(e.code, "ERR_ASSERTION");
    eq(e.name, "AssertionError");
    for (const k of Object.keys(props)) eq(e[k], props[k], k);
    return e;
  }
  throw new Error("did not throw");
};
test("shape", () => {
  eq(typeof assert, "function");
  eq(assert.ok, assert.ok);
  eq(assert.strict, strict);
  eq(strict.strict, strict);
  eq(strict.equal, assert.strictEqual);
  eq(strict.deepEqual, assert.deepStrictEqual);
  eq(assert.AssertionError.prototype instanceof Error, true);
});
test("ok", () => {
  assert(1);
  assert.ok("x");
  const e = fails(() => assert.ok(0), { operator: "==", generatedMessage: true, actual: 0, expected: true });
  eq(e.message.startsWith("The expression evaluated to a falsy value"), true);
  fails(() => assert.ok(), { message: "No value argument passed to `assert.ok()`" });
  fails(() => assert(false, "custom"), { message: "custom", generatedMessage: false });
  const own = new TypeError("mine");
  try { assert(false, own); } catch (x) { eq(x, own); }
});
test("equal and strictEqual", () => {
  assert.equal(1, "1");
  assert.equal(NaN, NaN);
  fails(() => assert.equal(1, 2), { message: "1 == 2", operator: "==", actual: 1, expected: 2 });
  assert.notEqual(1, 2);
  fails(() => assert.notEqual(1, "1"), { operator: "!=" });
  assert.strictEqual(1, 1);
  assert.strictEqual(NaN, NaN);
  fails(() => assert.strictEqual(1, 2), { message: "Expected values to be strictly equal:\n\n1 !== 2\n", operator: "strictEqual", generatedMessage: true });
  fails(() => assert.strictEqual(0, -0), { operator: "strictEqual" });
  fails(() => assert.strictEqual("a", "b", "nope"), { message: "nope", generatedMessage: false });
  assert.notStrictEqual(1, "1");
  fails(() => assert.notStrictEqual(1, 1), { operator: "notStrictEqual", message: 'Expected "actual" to be strictly unequal to: 1' });
  code(() => assert.strictEqual(1), "ERR_MISSING_ARGS", TypeError);
});
test("deepEqual and deepStrictEqual", () => {
  assert.deepEqual({ a: 1 }, { a: "1" });
  fails(() => assert.deepStrictEqual({ a: 1 }, { a: "1" }), { operator: "deepStrictEqual" });
  assert.deepStrictEqual({ a: [1, { b: new Map([[1, new Set([2])]]) }] }, { a: [1, { b: new Map([[1, new Set([2])]]) }] });
  assert.deepStrictEqual([1, 2, 3], [1, 2, 3]);
  fails(() => assert.deepStrictEqual([1, 2], [1, 3]), { operator: "deepStrictEqual" });
  assert.notDeepStrictEqual({ a: 1 }, { a: 2 });
  fails(() => assert.notDeepStrictEqual([1], [1]), { operator: "notDeepStrictEqual" });
  assert.notDeepEqual({ a: 1 }, { a: 2 });
  fails(() => assert.notDeepEqual({ a: 1 }, { a: "1" }), { operator: "notDeepEqual" });
  strict.deepEqual(new Date(5), new Date(5));
  fails(() => strict.deepEqual(/a/g, /a/i), {});
  const a = {}; a.me = a;
  const b = {}; b.me = b;
  assert.deepStrictEqual(a, b);
});
test("throws", () => {
  assert.throws(() => { throw new TypeError("bad"); });
  assert.throws(() => { throw new TypeError("bad"); }, TypeError);
  assert.throws(() => { throw new TypeError("bad"); }, /bad/);
  assert.throws(() => { throw new TypeError("bad"); }, { name: "TypeError", message: "bad" });
  assert.throws(() => { throw new TypeError("bad"); }, { message: /ba/ });
  assert.throws(() => { throw new Error("x"); }, (e) => e.message === "x");
  fails(() => assert.throws(() => {}), { message: "Missing expected exception.", operator: "throws" });
  fails(() => assert.throws(() => {}, TypeError), { message: "Missing expected exception (TypeError)." });
  fails(() => assert.throws(() => {}, "msg"), { message: "Missing expected exception: msg" });
  fails(() => assert.throws(() => { throw new Error("x"); }, { message: "y" }), { operator: "throws" });
  const other = new RangeError("r");
  try { assert.throws(() => { throw other; }, TypeError); } catch (e) { eq(e instanceof assert.AssertionError, true); }
  code(() => assert.throws(1), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("doesNotThrow", () => {
  assert.doesNotThrow(() => {});
  const e = fails(() => assert.doesNotThrow(() => { throw new Error("oops"); }), { operator: "doesNotThrow" });
  eq(e.message, 'Got unwanted exception.\nActual message: "oops"');
  const other = new RangeError("r");
  try { assert.doesNotThrow(() => { throw other; }, TypeError); } catch (x) { eq(x, other); }
});
atest("rejects and doesNotReject", async () => {
  await assert.rejects(Promise.reject(new Error("no")), /no/);
  await assert.rejects(async () => { throw new TypeError("t"); }, TypeError);
  let missing;
  await assert.rejects(Promise.resolve(1)).catch((e) => { missing = e; });
  eq(missing.message, "Missing expected rejection.");
  await assert.doesNotReject(Promise.resolve(1));
  let unwanted;
  await assert.doesNotReject(Promise.reject(new Error("u"))).catch((e) => { unwanted = e; });
  eq(unwanted.operator, "doesNotReject");
  let notFn;
  await assert.rejects(() => 1).catch((e) => { notFn = e; });
  eq(notFn.code, "ERR_INVALID_RETURN_VALUE");
});
test("fail, ifError, match", () => {
  fails(() => assert.fail(), { message: "Failed", operator: "fail", generatedMessage: true });
  fails(() => assert.fail("boom"), { message: "boom", generatedMessage: false });
  assert.ifError(null);
  assert.ifError(undefined);
  const e = fails(() => assert.ifError(new Error("inner")), { operator: "ifError" });
  eq(e.message, "ifError got unwanted exception: inner");
  assert.match("abc", /b/);
  fails(() => assert.match("abc", /z/), { operator: "match" });
  assert.doesNotMatch("abc", /z/);
  fails(() => assert.doesNotMatch("abc", /b/), { operator: "doesNotMatch" });
  code(() => assert.match("a", "a"), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("AssertionError", () => {
  const e = new assert.AssertionError({ actual: 1, expected: 2, operator: "strictEqual" });
  eq(e.generatedMessage, true);
  eq(e.toString().startsWith("AssertionError [ERR_ASSERTION]: Expected values"), true);
  const custom = new assert.AssertionError({ message: "m", actual: 1, expected: 2, operator: "x" });
  eq(custom.message, "m");
  eq(custom.generatedMessage, false);
  code(() => new assert.AssertionError(1), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("kernel errors keep their identity through throws checks", () => {
  const kernel = Object.assign(new Error("denied"), { code: "VELYR_PERMISSION_DENIED" });
  assert.throws(() => { throw kernel; }, { code: "VELYR_PERMISSION_DENIED" });
  try { assert.doesNotThrow(() => { throw kernel; }, TypeError); } catch (x) { eq(x, kernel); }
});
finish();
