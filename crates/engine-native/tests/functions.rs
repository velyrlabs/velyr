//! Functions on the bytecode VM (VELYR-105): classes (fields, private names, static
//! blocks, `extends`, `super`), `new.target`, lexical arrow bindings and
//! `Function.prototype.call/apply/bind`. Every script runs on the bytecode path only (no
//! tree-interpreter fallback).

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

/// Name of the error constructor a script throws, or its completion when it does not.
fn thrown(source: &str) -> String {
    run(&format!(
        "try {{ {source}; 'no exception' }} catch (e) {{ e.constructor.name }}"
    ))
}

#[test]
fn class_with_constructor_methods_and_accessors() {
    let source = "
        class Point {
            constructor(x, y) { this.x = x; this.y = y; }
            sum() { return this.x + this.y; }
            get double() { return this.sum() * 2; }
            set both(v) { this.x = v; this.y = v; }
        }
        const p = new Point(1, 2);
        const before = p.double;
        p.both = 5;
        [before, p.sum(), p instanceof Point, Point.name, Point.length] + '';";
    assert_eq!(run(source), "6,10,true,Point,2");
}

#[test]
fn class_members_are_not_enumerable_and_prototype_is_fixed() {
    let source = "
        class A { m() {} get g() { return 1; } static s() {} }
        let keys = '';
        for (const k in A.prototype) keys += k;
        for (const k in A) keys += k;
        A.prototype = null;
        [keys === '', typeof A.prototype, A.prototype.constructor === A] + '';";
    assert_eq!(run(source), "true,object,true");
}

#[test]
fn class_constructor_requires_new() {
    assert_eq!(thrown("class A {} A()"), "TypeError");
}

#[test]
fn class_expression_names_and_inner_binding() {
    let source = "
        const Anonymous = class {};
        const Named = class Inner { static self() { return Inner; } };
        [Anonymous.name, Named.name, Named.self() === Named] + '';";
    assert_eq!(run(source), "Anonymous,Inner,true");
}

#[test]
fn unreferenced_class_expression_name_has_no_binding_storage() {
    assert_eq!(run("var C = class X { static f = this.name; }; C.f"), "X");
}

#[test]
fn class_expression_inner_binding_is_immutable() {
    assert_eq!(
        thrown("var C = class X { static m() { X = 1; } }; C.m()"),
        "TypeError"
    );
}

#[test]
fn class_declaration_binding_is_in_tdz_inside_its_heritage() {
    assert_eq!(thrown("class C extends C {}"), "ReferenceError");
}

#[test]
fn static_methods_fields_and_blocks_run_in_order_with_this_as_the_class() {
    let source = "
        const log = [];
        let outer = 'o';
        class C {
            static a = (log.push('a'), this.name);
            static { log.push('block:' + (this === C) + ':' + outer); var local = 1; }
            static b = this.a + '!';
            static m() { return this.b; }
        }
        [log, C.a, C.m()] + '';";
    assert_eq!(run(source), "a,block:true:o,C,C!");
}

#[test]
fn instance_fields_initialize_per_construction_with_computed_keys_evaluated_once() {
    let source = "
        let keyCalls = 0;
        let base = 10;
        const key = () => { keyCalls++; return 'k'; };
        class A {
            x = base + 1;
            [key()] = this.x * 2;
            arrow = () => this.x;
            empty;
        }
        const a = new A(), b = new A();
        base = 20;
        const c = new A();
        [keyCalls, a.x, a.k, a.arrow(), 'empty' in a, a.empty, c.x, a.arrow !== b.arrow] + '';";
    assert_eq!(run(source), "1,11,22,11,true,,21,true");
}

#[test]
fn private_fields_methods_and_accessors_with_brand_checks() {
    let source = "
        class Counter {
            #count = 0;
            static #instances = 0;
            constructor() { Counter.#instances++; }
            #step() { return 1; }
            get #value() { return this.#count; }
            set #value(v) { this.#count = v; }
            increment() { this.#value += this.#step(); return this.#value; }
            static instances() { return Counter.#instances; }
            static isCounter(o) { return #count in o; }
            static peek(o) { return o.#count; }
        }
        const c = new Counter();
        c.increment(); c.increment();
        new Counter();
        let brand;
        try { Counter.peek({}); } catch (e) { brand = e.constructor.name; }
        [c.increment(), Counter.instances(), Counter.isCounter(c), Counter.isCounter({}), brand]
            + '';";
    assert_eq!(run(source), "3,2,true,false,TypeError");
}

#[test]
fn private_methods_are_not_writable_and_getters_without_setters_reject_writes() {
    assert_eq!(
        thrown("class A { #m() {} constructor() { this.#m = 1; } } new A()"),
        "TypeError"
    );
    assert_eq!(
        thrown("class A { get #g() { return 1; } constructor() { this.#g = 1; } } new A()"),
        "TypeError"
    );
}

#[test]
fn private_names_are_distinct_per_class_evaluation() {
    let source = "
        const make = () => class { #x = 1; static read(o) { return o.#x; } };
        const A = make(), B = make();
        let result;
        try { B.read(new A()); result = 'read'; } catch (e) { result = e.constructor.name; }
        [A.read(new A()), result] + '';";
    assert_eq!(run(source), "1,TypeError");
}

#[test]
fn derived_classes_call_super_and_super_properties() {
    let source = "
        class Animal {
            constructor(name) { this.name = name; }
            speak() { return this.name + ' makes a sound'; }
            static create(name) { return new this(name); }
        }
        class Dog extends Animal {
            tricks = this.name + '!';
            constructor(name) { super(name); }
            speak() { return super.speak() + ' (woof)'; }
            static create(name) { return super.create(name + ''); }
        }
        const d = Dog.create('Rex');
        [d.speak(), d.tricks, d instanceof Dog, d instanceof Animal] + '';";
    assert_eq!(run(source), "Rex makes a sound (woof),Rex!,true,true");
}

#[test]
fn default_derived_constructor_forwards_all_arguments() {
    let source = "
        class A { constructor(...args) { this.args = args; } }
        class B extends A {}
        new B(1, 2, 3).args + '';";
    assert_eq!(run(source), "1,2,3");
}

#[test]
fn derived_constructor_this_binding_rules() {
    assert_eq!(
        thrown("class A {} class B extends A { constructor() { this.x = 1; super(); } } new B()"),
        "ReferenceError"
    );
    assert_eq!(
        thrown("class A {} class B extends A { constructor() { super(); super(); } } new B()"),
        "ReferenceError"
    );
    assert_eq!(
        thrown("class A {} class B extends A { constructor() {} } new B()"),
        "ReferenceError"
    );
    assert_eq!(
        thrown("class A {} class B extends A { constructor() { super(); return 1; } } new B()"),
        "TypeError"
    );
    let source = "
        class A {}
        class B extends A { constructor() { return { custom: true }; } }
        new B().custom;";
    assert_eq!(run(source), "true");
}

#[test]
fn super_call_inside_an_arrow_binds_the_constructor_this() {
    let source = "
        class A { constructor() { this.a = 1; } }
        class B extends A {
            constructor() { const init = () => super(); const read = () => this.a; init();
                this.b = read() + 1; }
        }
        const b = new B();
        [b.a, b.b] + '';";
    assert_eq!(run(source), "1,2");
}

#[test]
fn extends_null_and_invalid_heritage() {
    assert_eq!(thrown("class A extends 1 {}"), "TypeError");
    assert_eq!(
        thrown("var F = function () {}; F.prototype = 1; class A extends F {}"),
        "TypeError"
    );
    let source = "
        class A extends null {}
        const proto = A.prototype;
        let threw;
        try { new A(); } catch (e) { threw = e.constructor.name; }
        [typeof proto, threw] + '';";
    assert_eq!(run(source), "object,TypeError");
}

#[test]
fn extends_a_builtin_constructor() {
    let source = "
        class MyError extends Error {
            constructor(message) { super(message); this.extra = 1; }
        }
        const e = new MyError('boom');
        [e instanceof MyError, e instanceof Error, e.message, e.extra] + '';";
    assert_eq!(run(source), "true,true,boom,1");
}

#[test]
fn super_property_assignment_sets_on_this() {
    let source = "
        class A { set v(x) { this.fromSetter = x; } }
        class B extends A { m() { super.v = 3; super.w = 4; return [this.fromSetter, this.w]; } }
        new B().m() + '';";
    assert_eq!(run(source), "3,4");
}

#[test]
fn object_literal_methods_reach_super() {
    let source = "
        const base = { hello() { return 'base ' + this.tag; } };
        const derived = { __proto__: base, tag: 'D', hello() { return super.hello() + '!'; } };
        derived.hello();";
    assert_eq!(run(source), "base D!");
}

#[test]
fn new_target_in_functions_classes_and_arrows() {
    let source = "
        function F() { return new.target === F; }
        function G() { const arrow = () => new.target; return arrow(); }
        class A { constructor() { this.target = new.target; } }
        class B extends A {}
        [F(), new F() instanceof F, G() === undefined, new B().target === B] + '';";
    assert_eq!(run(source), "false,true,true,true");
}

#[test]
fn arrows_see_the_enclosing_arguments_and_this() {
    let source = "
        function outer() {
            'use strict';
            const read = () => arguments[1] + this.base;
            return read();
        }
        outer.call({ base: 10 }, 1, 2);";
    assert_eq!(run(source), "12");
}

#[test]
fn function_prototype_call_and_apply() {
    let source = "
        function f(a, b) { return this.base + a + b; }
        const o = { base: 100 };
        [f.call(o, 1, 2), f.apply(o, [3, 4]), f.apply(o, { length: 2, 0: 5, 1: 6 })] + '';";
    assert_eq!(run(source), "103,107,111");
    assert_eq!(thrown("(function () {}).call.call(1)"), "TypeError");
}

#[test]
fn function_prototype_bind() {
    let source = "
        function Point(x, y) { this.x = x; this.y = y; }
        function add(a, b, c) { return this.base + a + b + c; }
        const bound = add.bind({ base: 1 }, 10);
        const BoundPoint = Point.bind(null, 7);
        const p = new BoundPoint(8);
        [bound(20, 30), bound.name, bound.length, p.x, p.y, p instanceof Point,
         p instanceof BoundPoint] + '';";
    assert_eq!(run(source), "61,bound add,2,7,8,true,true");
}

#[test]
fn deep_recursion_through_call_does_not_nest_the_rust_stack() {
    let source = "
        function down(n) { return n === 0 ? 'done' : down.call(null, n - 1); }
        down(500);";
    assert_eq!(run(source), "done");
}

#[test]
fn sloppy_function_caller_is_undefined() {
    assert_eq!(run("function f() {} f.caller === undefined"), "true");
}

#[test]
fn for_in_var_initializer_runs_before_head() {
    let source = "var log = ''; for (var x = (log += 'i', 1) in (log += 'o', {})) {} log + x";
    assert_eq!(run(source), "io1");
}
