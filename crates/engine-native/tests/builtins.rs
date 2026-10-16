//! Core built-ins on the bytecode VM (VELYR-107). Every script runs on the bytecode path
//! only (no tree-interpreter fallback).

use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

/// `String(completion)`, or `throw <message>` for an uncaught exception.
fn run(source: &str) -> String {
    let code = match compile_script(source) {
        Ok(code) => code,
        Err(error) => return format!("compile error: {}", error.message),
    };
    let mut vm = Interpreter::new();
    match vm.run_script(&code) {
        Ok(value) => match vm.to_js_string(&value) {
            Ok(text) => text.to_string_lossy(),
            Err(_) => "<unprintable completion>".into(),
        },
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(abrupt) => format!("{abrupt:?}"),
    }
}

#[test]
fn sloppy_arguments_without_parameter_writes_compile() {
    let source = "function f(a, b) { return arguments.length + a; } f(1, 2, 3)";
    assert_eq!(run(source), "4");
}

#[test]
fn object_define_property_and_freeze_follow_attributes() {
    let source = "'use strict'; const o = {}; Object.defineProperty(o, 'x', { value: 1 });
        const d = Object.getOwnPropertyDescriptor(o, 'x');
        const f = Object.freeze({ y: 2 });
        let threw = false; try { f.y = 3; } catch (e) { threw = e instanceof TypeError; }
        [d.writable, d.enumerable, d.configurable, Object.isFrozen(f), threw,
         o.hasOwnProperty('x'), Object.keys({ a: 1, b: 2 }).join('')].join()";
    assert_eq!(run(source), "false,false,false,true,true,true,ab");
}

#[test]
fn function_prototype_methods_bind_call_apply() {
    let source = "function f(a, b) { return this.k + a + b; }
        const g = f.bind({ k: 1 }, 2);
        [g(3), f.call({ k: 10 }, 1, 1), f.apply({ k: 0 }, [4, 5]), g.name, g.length].join()";
    assert_eq!(run(source), "6,12,9,bound f,1");
}

#[test]
fn array_methods_cover_es2025_surface() {
    let source = "const a = [3, 1, 2];
        [a.toSorted().join(''), a.join(''), a.with(0, 9).join(''), a.at(-1),
         a.toSpliced(1, 1, 'x', 'y').join(''), a.findLast(x => x > 1), a.findLastIndex(x => x > 5),
         [1, [2, [3, [4]]]].flat(Infinity).join(''), [1, 2].flatMap(x => [x, x * 2]).join(''),
         Array.from('abc').join('-'), Array.of(7, 8).length, Array.isArray(new Proxy([], {})),
         [1, 2, 3].reduce((s, x) => s + x), [, 1].includes(undefined), [NaN].indexOf(NaN)].join()";
    assert_eq!(
        run(source),
        "123,312,912,2,3xy2,2,-1,1234,1224,a-b-c,2,true,6,true,-1"
    );
}

#[test]
fn array_sort_is_stable_and_moves_holes_and_undefined_last() {
    let source = "const a = [{ k: 1, v: 'a' }, { k: 0, v: 'b' }, { k: 1, v: 'c' }];
        const b = [3, undefined, , 1];
        b.sort();
        a.sort((x, y) => x.k - y.k).map(x => x.v).join('') + b.length + (2 in b) + (3 in b)";
    assert_eq!(run(source), "bac4truefalse");
}

#[test]
fn array_length_writes_validate_range() {
    let source = "let r; try { new Array(-1); } catch (e) { r = e instanceof RangeError; }
        const a = [1, 2, 3]; a.length = 1; r + ',' + a.join()";
    assert_eq!(run(source), "true,1");
}

#[test]
fn reflect_and_proxy_traps_observe_operations() {
    let source = "const log = [];
        const p = new Proxy({}, { get(t, k, r) { log.push(k); return Reflect.get(t, k, r); },
          has(t, k) { return k === 'x'; } });
        p.a; ('x' in p);
        const { proxy, revoke } = Proxy.revocable({}, {}); revoke();
        let threw = false; try { proxy.x; } catch (e) { threw = e instanceof TypeError; }
        [log.join(), 'x' in p, threw, Reflect.ownKeys({ b: 1, [Symbol.iterator]: 0 }).length].join()";
    assert_eq!(run(source), "a,true,true,2");
}

#[test]
fn proxy_invariants_are_enforced() {
    let source = "const t = {}; Object.defineProperty(t, 'x', { value: 1 });
        const p = new Proxy(t, { get() { return 2; } });
        try { p.x; 'no error'; } catch (e) { e.constructor.name; }";
    assert_eq!(run(source), "TypeError");
}

#[test]
fn errors_carry_cause_and_prototype_chain() {
    let source = "const e = new RangeError('m', { cause: 1 });
        [e.cause, e.name, e.message, e instanceof Error, Object.prototype.toString.call(e),
         String(new TypeError('x'))].join()";
    assert_eq!(
        run(source),
        "1,RangeError,m,true,[object Error],TypeError: x"
    );
}

#[test]
fn symbol_registry_and_well_known_symbols() {
    let source = "const s = Symbol.for('k');
        [Symbol.keyFor(s), Symbol('d').description, typeof Symbol.iterator,
         Symbol.for('k') === s, String(Symbol('z'))].join()";
    assert_eq!(run(source), "k,d,symbol,true,Symbol(z)");
}

#[test]
fn boolean_wrappers_unwrap() {
    let source =
        "[new Boolean(false).valueOf(), typeof new Boolean(1), Boolean('').toString()].join()";
    assert_eq!(run(source), "false,object,false");
}

#[test]
fn string_methods_cover_es2025_surface() {
    let source = "const s = new String('ab');
        ['abc'.at(-1), 'a-b-c'.split('-').length, 'aXbX'.replaceAll('X', '[$&]'),
         'x'.padStart(3, 'ab'), '  t '.trim() + '|', '\\u0041\\u030A'.normalize('NFC').length,
         'ABC'.toLowerCase(), 'abc'.codePointAt(1), String.fromCodePoint(0x1F600).length,
         [...'a\\u{1F600}'].length, s.length, s[1], Object.keys(s).join(''), 'a'.localeCompare('b'),
         String.raw`x${1}y`, 'b'.anchor('\"'), 'ab'.substr(-1), String.fromCharCode(0xD800).isWellFormed()].join()";
    assert_eq!(
        run(source),
        "c,3,a[X]b[X],abx,t|,1,abc,98,2,2,2,b,01,-1,x1y,<a name=\"&quot;\">b</a>,b,false"
    );
}

#[test]
fn number_formatting_is_exact() {
    let source = "[(1.005).toFixed(2), (1.45).toFixed(1), (0.000001).toFixed(7), (-1.5).toFixed(0),
         (123.456).toExponential(2), (0.00001).toExponential(), (123.456).toPrecision(4),
         (0.000123).toPrecision(2), (1e21).toPrecision(3), (255).toString(16), (0.5).toString(2),
         (1e21).toFixed(2), Number('0x10'), Number(10n), parseInt('  -0x1F'), parseFloat('3.5e2x'),
         Number.isSafeInteger(2 ** 53), Number.parseInt === parseInt, (25).toPrecision(1)].join()";
    assert_eq!(
        run(source),
        "1.00,1.4,0.0000010,-2,1.23e+2,1e-5,123.5,0.00012,1.00e+21,ff,0.1,1e+21,16,10,-31,350,false,true,3e+1"
    );
}

#[test]
fn math_functions_follow_number_semantics() {
    let source = "[Math.round(-0.5), 1 / Math.round(-0.2), Math.round(0.49999999999999994), Math.max(),
         Math.min(0, -0) === 0 && 1 / Math.min(0, -0), Math.hypot(3, 4), Math.clz32(1), Math.imul(-1, 8),
         Math.fround(5.5), Math.f16round(1.337), Math.sign(-3), Math.cbrt(27), Math.trunc(-4.7),
         Math.sumPrecise([1e20, 0.1, -1e20]), Math.sumPrecise([]) === 0 && 1 / Math.sumPrecise([]),
         Math.pow(1, Infinity), typeof Math.random(), Object.prototype.toString.call(Math)].join()";
    assert_eq!(
        run(source),
        "0,-Infinity,0,-Infinity,-Infinity,5,31,-8,5.5,1.3369140625,-1,3,-4,0.1,-Infinity,NaN,number,[object Math]"
    );
}

#[test]
fn json_round_trips_with_reviver_replacer_and_indent() {
    let source = "const o = JSON.parse('{\"a\":[1,2,{\"b\":null}],\"c\":\"\\\\u0041\"}', (k, v) => k === 'b' ? 7 : v);
        let cyc = {}; cyc.self = cyc;
        let threw; try { JSON.stringify(cyc); } catch (e) { threw = e instanceof TypeError; }
        let bad; try { JSON.parse('{a:1}'); } catch (e) { bad = e instanceof SyntaxError; }
        [JSON.stringify(o), JSON.stringify({ x: 1, y: [2] }, null, 2), JSON.stringify({ x: 1, y: 2 }, ['y']),
         JSON.stringify({ toJSON() { return 'j'; } }), JSON.stringify(new Number(3)), JSON.stringify(undefined),
         JSON.stringify('\\u2028\\ud800'.slice(0, 1) + String.fromCharCode(0xdfff)), threw, bad].join('|')";
    assert_eq!(
        run(source),
        "{\"a\":[1,2,{\"b\":7}],\"c\":\"A\"}|{\n  \"x\": 1,\n  \"y\": [\n    2\n  ]\n}|{\"y\":2}|\"j\"|3||\"\u{2028}\\udfff\"|true|true"
    );
}

#[test]
fn keyed_collections_keep_insertion_order_and_weak_keys() {
    let source = "const m = new Map([[1, 'a'], [-0, 'z']]); m.set(NaN, 'n');
        const s = new Set([1, 2, 2, 3]);
        const seen = []; for (const x of s) { seen.push(x); if (x === 1) { s.delete(2); s.add(4); } }
        const wm = new WeakMap(); const k = {}; wm.set(k, 5);
        let bad; try { wm.set(1, 1); } catch (e) { bad = e instanceof TypeError; }
        const g = Map.groupBy([1, 2, 3], x => x % 2);
        [m.get(0), m.get(NaN), m.size, [...m.keys()].join(':'), seen.join(''),
         [...new Set([1, 2]).union(new Set([2, 3]))].join(''), new Set([1, 2]).isSubsetOf(new Set([1, 2, 3])),
         [...new Set([1, 2, 3]).difference(new Set([2]))].join(''), wm.get(k), wm.has({}), bad,
         new WeakSet([k]).has(k), g.get(1).join(''), Object.prototype.toString.call(m)].join()";
    assert_eq!(
        run(source),
        "z,n,3,1:0:NaN,134,123,true,13,5,false,true,true,13,[object Map]"
    );
}

#[test]
fn string_literals_keep_lone_surrogates() {
    let source = "['\\uD800'.charCodeAt(0), `\\uDC00x`.charCodeAt(0), '\\uFFFD'.charCodeAt(0),
         '\\uD83D\\uDE00'.length, String.raw`\\uD800`.length].join()";
    assert_eq!(run(source), "55296,56320,65533,2,6");
}

#[test]
fn error_is_error_checks_the_error_data_slot() {
    let source = "class E extends Error {}
        [Error.isError(new TypeError()), Error.isError(new E()), Error.isError({ __proto__: Error.prototype }),
         Error.isError(Object.create(null)), 'abc'.replace({ [Symbol.replace]: null }, 'x')].join()";
    assert_eq!(run(source), "true,true,false,false,abc");
}

#[test]
fn sloppy_arguments_written_through_alias_their_parameters() {
    let escaping =
        "function g(a) { Object.defineProperty(arguments, '0', { value: 2 }); return a; } g(1)";
    assert_eq!(run(escaping), "2");
    let written = "function g(a) { arguments[0] = 2; return a; } g(1)";
    assert_eq!(run(written), "2");
    let no_params = "function h() { return Array.prototype.slice.call(arguments).length; } h(1, 2)";
    assert_eq!(run(no_params), "2");
    let unread_params =
        "function k(a, b) { arguments[0] = 5; return [].slice.call(arguments).join(); } k(1, 2)";
    assert_eq!(run(unread_params), "5,2");
}

#[test]
fn method_to_string_starts_at_the_definition() {
    let source =
        "class A { constructor() {} static /* c */ m() {} } const o = { get g() { return 1; } };
        [A.m.toString(), Object.getOwnPropertyDescriptor(o, 'g').get.toString(),
         A.toString() === 'class A { constructor() {} static /* c */ m() {} }'].join('|')";
    assert_eq!(run(source), "m() {}|get g() { return 1; }|true");
}

#[test]
fn proxy_on_the_prototype_chain_runs_its_traps() {
    let source = "const target = { get self() { return this; } };
        const child = Object.create(new Proxy(target, {}));
        let receiver;
        const writer = Object.create(new Proxy({}, { set(t, k, v, r) { receiver = r; return true; } }));
        writer.x = 1;
        [child.self === child, receiver === writer, writer.hasOwnProperty('x')].join()";
    assert_eq!(run(source), "true,true,false");
}

#[test]
fn bind_length_uses_to_integer_or_infinity() {
    let source = "function f(a, b) {}
        Object.defineProperty(f, 'length', { value: Infinity });
        const big = function () {}; Object.defineProperty(big, 'length', { value: 2 ** 53 });
        const g = function () {}; Object.setPrototypeOf(g, { length: 5 }); delete g.length;
        [f.bind().length, f.bind(null, 1).length, big.bind().length,
         Function.prototype.bind.call(g).length].join()";
    assert_eq!(run(source), "Infinity,Infinity,9007199254740992,0");
}

#[test]
fn iterator_prototype_to_string_tag_is_an_accessor() {
    let source = "const proto = Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]()));
        const d = Object.getOwnPropertyDescriptor(proto, Symbol.toStringTag);
        const it = Object.create(proto); it[Symbol.toStringTag] = 'Mine';
        let threw = false; try { proto[Symbol.toStringTag] = 'x'; } catch (e) { threw = e instanceof TypeError; }
        [typeof d.get, Object.prototype.toString.call(Object.create(proto)),
         it.hasOwnProperty(Symbol.toStringTag), threw].join()";
    assert_eq!(run(source), "function,[object Iterator],true,true");
}

#[test]
fn sloppy_functions_box_a_primitive_this() {
    let source = "function sloppy() { return typeof this; }
        function strict() { 'use strict'; return typeof this; }
        Number.prototype.kind = sloppy;
        [sloppy.call(1), strict.call(1), (2).kind(), [0].map(sloppy, 'x')[0],
         Object.getOwnPropertyNames(sloppy).includes('caller'), sloppy.caller].join()";
    assert_eq!(run(source), "object,number,object,object,true,");
}

#[test]
fn keyed_store_and_array_length_coerce_objects() {
    let source = "const key = { valueOf() { return 1; }, toString() { return 0; } };
        const x = []; x[key] = 5;
        const y = [1, 2, 3]; y.length = new Number(1);
        const z = []; z.length = { valueOf() { return 2; } };
        [x[0], y.length, y.join(), z.length].join('|')";
    assert_eq!(run(source), "5|1|1|2");
}

#[test]
fn construct_through_a_revoked_proxy_new_target_throws() {
    let source = "const handle = Proxy.revocable(function () {}, { get() { handle.revoke(); } });
        let name; try { new handle.proxy(); } catch (e) { name = e.constructor.name; } name";
    assert_eq!(run(source), "TypeError");
}
