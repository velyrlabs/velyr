//! Language fixes measured against test262 `test/language` (VELYR-154): mapped arguments,
//! global declarations on a non-extensible global and classes nested in class scopes.

use velyr_engine_api::{Engine, Isolate, IsolateOptions, Source, Value};
use velyr_engine_native::NativeEngine;
use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

/// `String(completion)` on the bytecode path only (no tree-interpreter fallback).
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
fn sloppy_arguments_alias_simple_parameters() {
    let source =
        "function f(a, b) { arguments[0] = 9; b = 7; return [a, arguments[1], arguments.length]; }
        function late(a, b) { b = 2; return arguments[1] === undefined && arguments.length === 1; }
        function captured(a) { const read = () => arguments[0]; a = 5; return read(); }
        function escaped(a) { return [arguments, () => a]; }
        const [args, get] = escaped(1); args[0] = 6;
        function dup(a, a) { arguments[1] = 5; arguments[0] = 4; return a; }
        function desc(a) { a = 3; return Object.getOwnPropertyDescriptor(arguments, '0').value; }
        function callee(a) { return arguments.callee === callee; }
        [f(1, 2).join('|'), late(1), captured(1), get(), dup(1, 2), desc(1), callee(0),
         Object.prototype.toString.call(escaped(0)[0])].join()";
    assert_eq!(run(source), "9|7|2,true,5,6,5,3,true,[object Arguments]");
}

#[test]
fn deleting_or_redefining_an_argument_breaks_the_mapping() {
    let source = "function del(a) { delete arguments[0]; arguments[0] = 3; return a; }
        function frozen(a) { Object.defineProperty(arguments, '0', { value: 4, writable: false });
            a = 8; return [a, arguments[0]].join('|'); }
        function accessor(a) { Object.defineProperty(arguments, '0', { get() { return 'g'; } });
            a = 2; return arguments[0] + a; }
        function strict(a) { 'use strict'; arguments[0] = 2; return a; }
        function freeze(a) { Object.freeze(arguments); a = 9; return arguments[0]; }
        [del(1), frozen(1), accessor(1), strict(1), freeze(1)].join()";
    assert_eq!(run(source), "1,8|4,g2,1,1");
}

#[test]
fn classes_nested_in_class_scopes_compile() {
    let source = "class A extends class B { m() { return 1; } } {}
        class C { x = class D {}; static y = class E extends class F {} {}; }
        [String(A), new A().m(), new C().x.name, C.y.name].join('|')";
    assert_eq!(
        run(source),
        "class A extends class B { m() { return 1; } } {}|1|D|E"
    );
}

#[test]
fn global_declarations_need_an_extensible_global() {
    let mut isolate = NativeEngine
        .create_isolate(IsolateOptions::default())
        .expect("isolate");
    let source = Source {
        name: "language.js".into(),
        text: "var log = [];
            Object.preventExtensions(this);
            for (const code of ['var fresh;', 'function freshFn() {}', 'var log;']) {
                try { (0, eval)(code); log.push('ok'); } catch (e) { log.push(e.constructor.name); }
            }
            String(log)"
            .into(),
    };
    let result = isolate.eval_script(&source).expect("script runs");
    assert_eq!(result, Value::String("TypeError,TypeError,ok".into()));
}

#[test]
fn class_declaration_inner_name_binding_is_immutable() {
    let source =
        "class C { m() { try { C = 42; } catch (e) { return e.constructor.name; } return 'no'; } }
        const outer = C; C = null;
        [new outer().m(), typeof outer.prototype.m, C].join()";
    assert_eq!(run(source), "TypeError,function,");
}

#[test]
fn class_declaration_outer_binding_stays_mutable_and_separate() {
    let source = "class C { m() { return C; } }
        const cls = C; C = null;
        [C, cls.prototype.m() === cls].join()";
    assert_eq!(run(source), ",true");
}

#[test]
fn default_parameters_are_in_tdz_for_later_and_self_references() {
    let source = "function later(a = b, b) {} function self(a = a) {}
        const o = { m(a = b, b) {}, *g(a = b, b) {}, async am(a = b, b) {} };
        const k = class { static s(a = a) {} };
        const names = [];
        for (const f of [later, self, o.m, o.g, k.s]) {
            try { f(); names.push('no'); } catch (e) { names.push(e.constructor.name); }
        }
        names.join()";
    assert_eq!(
        run(source),
        "ReferenceError,ReferenceError,ReferenceError,ReferenceError,ReferenceError"
    );
}

#[test]
fn property_keys_honor_symbol_to_primitive() {
    let source =
        "const key = { [Symbol.toPrimitive](hint) { return hint === 'string' ? 'k' : 1; } };
        const holder = { k: 'string-hint', 1: 'default-hint' };
        class C { [key]() { return 'method'; } static [key] = 'field'; }
        const lone = { [key]: 1 };
        [holder[key], Object.keys(lone), typeof C.prototype.k, C.k, Reflect.has(holder, key),
         delete holder[key], 'k' in holder].join()";
    assert_eq!(run(source), "string-hint,k,function,field,true,true,false");
}

#[test]
fn private_names_do_not_leak_as_symbols() {
    let source = "class C { #a = 1; #m() {} static #s = 2; get #g() { return 1; }
            static probe(o) { return #a in o; } }
        const c = new C();
        [Object.getOwnPropertySymbols(c).length, Reflect.ownKeys(c).length,
         Object.getOwnPropertySymbols(C).length, Object.getOwnPropertyNames(c).length,
         Object.getOwnPropertySymbols(Object.getOwnPropertyDescriptors(c)).length,
         Object.getOwnPropertySymbols(C.prototype).length].join()";
    assert_eq!(run(source), "0,0,0,0,0,0");
}

#[test]
fn private_fields_reject_non_extensible_objects() {
    let source = "class Base { constructor(o) { return o; } }
        class C extends Base { #x = 7; static has(o) { return #x in o; } }
        const attempt = (o) => { try { new C(o); return 'added'; } catch (e) { return e.constructor.name; } };
        const open = {};
        [attempt(Object.preventExtensions({})), attempt(Object.freeze({})), attempt(open),
         C.has(open)].join()";
    assert_eq!(run(source), "TypeError,TypeError,added,true");
}

#[test]
fn anonymous_functions_in_computed_key_fields_take_the_key_as_name() {
    let source = "const sym = Symbol('desc'); let n = 0; const k = () => 'f' + n++;
        class C { [k()] = function() {}; static [k()] = () => {}; static [sym] = class {};
            [k()] = class { static name = 'own'; }; }
        const c = new C();
        [c.f0.name, C.f1.name, C[sym].name, c.f2.name].join()";
    assert_eq!(run(source), "f0,f1,[desc],own");
}

#[test]
fn primitive_this_is_boxed_in_sloppy_functions_only() {
    let source = "function sloppy() { return typeof this; }
        function strict() { 'use strict'; return typeof this; }
        const arrowHolder = { m() { return () => typeof this; } };
        const o = { sloppy, strict, getter: Object.getOwnPropertyDescriptor(
            { get g() { return typeof this; } }, 'g').get };
        class K { m() { return typeof this; } static s() { return typeof this; } }
        [sloppy.call(1), sloppy.call('s'), sloppy.call(true), sloppy.call(Symbol()), sloppy.call(1n),
         strict.call(1), sloppy.call(null) , sloppy.call(undefined),
         o.getter.call(5), K.prototype.m.call(1), K.s.call(2),
         sloppy.apply(3), [4].map(function() { return typeof this; }, 4)[0]].join()";
    assert_eq!(
        run(source),
        "object,object,object,object,object,number,object,object,object,number,number,object,object"
    );
}

#[test]
fn assignment_value_survives_a_setter_call() {
    let source = "const o = { set a(v) { return 9; } }; const k = 'a';
        class C { static set [1 + 1](v) { return 9; } }
        [o.a = 2, o[k] = 3, C[2] = 5, C[1 + 1] = 6].join()";
    assert_eq!(run(source), "2,3,5,6");
}
