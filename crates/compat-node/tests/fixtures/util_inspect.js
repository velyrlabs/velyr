const { inspect } = node("util");
test("primitives", () => {
  eq(inspect("a"), "'a'");
  eq(inspect("it's"), '"it\'s"');
  eq(inspect(`it's "q"`), "`it's \"q\"`");
  eq(inspect("a\nb"), "'a\\nb'");
  eq(inspect(-0), "-0");
  eq(inspect(10n), "10n");
  eq(inspect(Symbol("s")), "Symbol(s)");
  eq(inspect(undefined), "undefined");
  eq(inspect(null), "null");
  eq(inspect(true), "true");
});
test("objects and arrays", () => {
  eq(inspect({}), "{}");
  eq(inspect([]), "[]");
  eq(inspect({ a: 1, "b-c": "x" }), "{ a: 1, 'b-c': 'x' }");
  eq(inspect([1, [2, [3, [4]]]]), "[ 1, [ 2, [ 3, [Array] ] ] ]");
  eq(inspect([1, , 3]), "[ 1, <1 empty item>, 3 ]");
  eq(inspect({ [Symbol("k")]: 1 }), "{ [Symbol(k)]: 1 }");
  eq(inspect(Object.create(null)), "[Object: null prototype] {}");
  class Foo { constructor() { this.x = 1; } }
  eq(inspect(new Foo()), "Foo { x: 1 }");
  eq(inspect({ get a() { return 1; }, set b(v) {} }), "{ a: [Getter], b: [Setter] }");
});
test("nested depth", () => {
  const value = { a: { b: { c: { d: 1 } } } };
  eq(inspect(value), "{ a: { b: { c: [Object] } } }");
  eq(inspect(value, { depth: null }), "{\n  a: { b: { c: { d: 1 } } }\n}");
  eq(inspect(value, { depth: 0 }), "{ a: [Object] }");
  eq(inspect(value, { compact: false, depth: 0 }), "{\n  a: [Object]\n}");
});
test("circular", () => {
  const o = { name: "o" };
  o.self = o;
  eq(inspect(o), "<ref *1> { name: 'o', self: [Circular *1] }");
  const a = [];
  a.push(a);
  eq(inspect(a), "<ref *1> [ [Circular *1] ]");
  const p = { q: {} };
  p.q.back = p;
  eq(inspect(p), "<ref *1> { q: { back: [Circular *1] } }");
});
test("line breaking and grouping", () => {
  eq(inspect([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]), "[\n  0, 1, 2, 3, 4,\n  5, 6, 7, 8, 9\n]");
  const long = { aaaaaaaaaaaaaaaa: "bbbbbbbbbbbbbbbbbbbb", cccccccccccccccc: "dddddddddddddddddddd", eeeeeeeeeeeee: 1 };
  eq(inspect(long), "{\n  aaaaaaaaaaaaaaaa: 'bbbbbbbbbbbbbbbbbbbb',\n  cccccccccccccccc: 'dddddddddddddddddddd',\n  eeeeeeeeeeeee: 1\n}");
  eq(inspect(Array.from({ length: 101 }, (_, i) => i)).includes("... 1 more item\n]"), true);
});
test("collections and builtins", () => {
  eq(inspect(new Map([[1, "a"], ["k", { z: 1 }]])), "Map(2) { 1 => 'a', 'k' => { z: 1 } }");
  eq(inspect(new Set([1, 2])), "Set(2) { 1, 2 }");
  eq(inspect(new Map()), "Map(0) {}");
  eq(inspect(new Date(0)), "1970-01-01T00:00:00.000Z");
  eq(inspect(/a+/gi), "/a+/gi");
  eq(inspect(new Uint8Array([1, 2])), "Uint8Array(2) [ 1, 2 ]");
  eq(inspect(new ArrayBuffer(2)), "ArrayBuffer { [Uint8Contents]: <00 00>, byteLength: 2 }");
  eq(inspect(new Number(3)), "[Number: 3]");
  eq(inspect(new String("s")), "[String: 's']");
  eq(inspect(new WeakMap()), "WeakMap { <items unknown> }");
  eq(inspect(function foo() {}), "[Function: foo]");
  eq(inspect(() => {}), "[Function (anonymous)]");
  eq(inspect(class A {}), "[class A]");
  eq(inspect(class B extends Array {}), "[class B extends Array]");
  eq(inspect(async function f() {}), "[AsyncFunction: f]");
  eq(inspect(function* g() {}), "[GeneratorFunction: g]");
  const f = function bar() {};
  f.x = 1;
  eq(inspect(f), "[Function: bar] { x: 1 }");
});
test("errors", () => {
  const e = new Error("boom");
  e.stack = "Error: boom\n    at x (y:1:1)";
  eq(inspect(e), "Error: boom\n    at x (y:1:1)");
  const noStack = new RangeError("r");
  noStack.stack = undefined;
  eq(inspect(noStack), "[RangeError: r]");
  const withProps = new Error("p");
  withProps.stack = "Error: p\n    at z (q:1:1)";
  withProps.code = "E_X";
  eq(inspect(withProps), "Error: p\n    at z (q:1:1) {\n  code: 'E_X'\n}");
});
test("options", () => {
  eq(inspect({ b: 1, a: 2 }, { sorted: true }), "{ a: 2, b: 1 }");
  eq(inspect([1, 2, 3], { maxArrayLength: 1 }), "[ 1, ... 2 more items ]");
  eq(inspect("abcdef", { maxStringLength: 3 }), "'abc'... 3 more characters");
  eq(inspect({ a: 1 }, { colors: true }), "{ a: \u001b[33m1\u001b[39m }");
  eq(inspect([1], { showHidden: true }), "[ 1, [length]: 1 ]");
  eq(inspect({ a: { b: 1 } }, { compact: true }), "{ a: { b: 1 } }");
  eq(inspect({ a: 1 }, false, 0), "{ a: 1 }");
});
test("custom inspect and toStringTag", () => {
  eq(inspect({ [inspect.custom]: () => "CUSTOM" }), "CUSTOM");
  eq(inspect({ [inspect.custom](depth, opts) { return { d: depth, t: typeof opts.stylize }; } }), "{ d: 2, t: 'function' }");
  eq(inspect({ [inspect.custom]: () => "x" }, { customInspect: false }), "{\n  [Symbol(nodejs.util.inspect.custom)]: [Function: [nodejs.util.inspect.custom]]\n}");
  class Tagged { get [Symbol.toStringTag]() { return "T"; } }
  eq(inspect(new Tagged()), "Tagged [T] {}");
});
finish();
