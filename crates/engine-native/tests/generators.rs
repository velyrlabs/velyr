//! Generators, async functions, async generators, `for await` and the iterator helpers on
//! the bytecode VM (VELYR-109, VELYR-166), on the bytecode path only (no tree fallback).

use std::rc::Rc;

use velyr_engine_native::{bytecode_compiler::compile_script, iterator_helpers};
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

/// A VM with the iterator helpers prelude and the dynamic-code hook, like `NativeIsolate`.
fn new_vm() -> Interpreter {
    let mut vm = Interpreter::new();
    vm.set_compile_hook(Rc::new(|s: &str, _| compile_script(s).map_err(|e| e.message)));
    iterator_helpers::install(&mut vm).expect("iterator helpers install");
    vm
}

fn eval(vm: &mut Interpreter, source: &str) -> String {
    let code = match compile_script(source) {
        Ok(code) => code,
        Err(error) => return format!("compile error: {}", error.message),
    };
    match vm.run_script(&code) {
        Ok(value) => match vm.to_js_string(&value) {
            Ok(text) => text.to_string_lossy(),
            Err(_) => "<unprintable completion>".into(),
        },
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(abrupt) => format!("{abrupt:?}"),
    }
}

/// `String(completion)`, or `throw <message>` for an uncaught exception.
fn run(source: &str) -> String {
    eval(&mut new_vm(), source)
}

/// Runs `setup` after `var log = []`, drains the job queue and returns `String(log)`.
fn log_after(setup: &str) -> String {
    let mut vm = new_vm();
    let completion = eval(&mut vm, &format!("var log = []; {setup}"));
    let ok = !completion.starts_with("throw") && !completion.starts_with("compile error");
    assert!(ok, "{completion}");
    while vm.perform_microtask_checkpoint().expect("jobs run") {}
    eval(&mut vm, "String(log)")
}

#[test]
fn yield_suspends_and_resumes_with_the_sent_value() {
    let source = "
        function* g(a) { const b = yield a + 1; const c = yield b * 2; return c; }
        const it = g(1);
        const r = [it.next('ignored'), it.next(5), it.next('end'), it.next()];
        r.map(x => x.value + ':' + x.done).join();";
    assert_eq!(run(source), "2:false,10:false,end:true,undefined:true");
}

#[test]
fn return_and_throw_run_finally_blocks_and_complete_the_generator() {
    let source = "
        const log = [];
        function* g() {
            try { yield 1; yield 2; } finally { log.push('cleanup'); }
        }
        const a = g();
        a.next();
        const returned = a.return(9);
        const b = g();
        b.next();
        let caught;
        try { b.throw(new Error('boom')); } catch (e) { caught = e.message; }
        const c = g();
        const early = c.return(3);
        [returned.value, returned.done, a.next().done, caught, early.value, log.join('|')] + '';";
    assert_eq!(run(source), "9,true,true,boom,3,cleanup|cleanup");
}

#[test]
fn throw_is_catchable_inside_the_generator_and_finally_can_yield() {
    let source = "
        function* g() {
            try { yield 1; } catch (e) { yield 'caught ' + e; }
            try { yield 2; } finally { yield 'finally'; }
        }
        const it = g();
        const r = [it.next(), it.throw('x'), it.next(), it.return(7), it.next(), it.next()];
        r.map(x => x.value + ':' + x.done).join();";
    assert_eq!(
        run(source),
        "1:false,caught x:false,2:false,finally:false,7:true,undefined:true"
    );
}

#[test]
fn yield_star_delegates_values_and_returns_the_inner_result() {
    let source = "
        function* inner() { const x = yield 'a'; yield x; return 'inner-done'; }
        function* outer() { const r = yield* inner(); yield r; yield* [1, 2]; }
        const it = outer();
        const out = [it.next(), it.next('sent'), it.next(), it.next(), it.next(), it.next()];
        out.map(x => x.value + ':' + x.done).join();";
    assert_eq!(
        run(source),
        "a:false,sent:false,inner-done:false,1:false,2:false,undefined:true"
    );
}

#[test]
fn yield_star_forwards_return_and_throw_to_the_delegate() {
    let source = "
        const log = [];
        function* inner() {
            try { yield 1; yield 2; }
            catch (e) { log.push('inner caught ' + e); yield 'recovered'; }
            finally { log.push('inner finally'); }
        }
        function* outer() { try { yield* inner(); } finally { log.push('outer finally'); } }
        const a = outer();
        a.next();
        const thrown = a.throw('t');
        const returned = a.return('r');
        const noThrow = { [Symbol.iterator]() { return this; },
            next() { return { value: 0, done: false }; },
            return() { log.push('closed'); return {}; } };
        function* plain() { yield* noThrow; }
        const b = plain();
        b.next();
        let error;
        try { b.throw('x'); } catch (e) { error = e.constructor.name; }
        [thrown.value, returned.value, returned.done, error, log.join('|')] + '';";
    assert_eq!(
        run(source),
        "recovered,r,true,TypeError,inner caught t|inner finally|outer finally|closed"
    );
}

#[test]
fn generators_are_iterable_and_close_on_early_exit() {
    let source = "
        const log = [];
        function* nums() { try { yield 1; yield 2; yield 3; } finally { log.push('closed'); } }
        const spread = [...nums()];
        let sum = 0;
        for (const n of nums()) { if (n === 2) break; sum += n; }
        const [first] = nums();
        [spread.join('-'), sum, first, log.length] + '';";
    assert_eq!(run(source), "1-2-3,1,1,3");
}

#[test]
fn generator_objects_have_the_spec_prototype_chain() {
    let source = "
        function* g() {}
        const GeneratorFunctionPrototype = Object.getPrototypeOf(g);
        const GeneratorPrototype = GeneratorFunctionPrototype.prototype;
        const IteratorPrototype = Object.getPrototypeOf(GeneratorPrototype);
        const it = g();
        const obj = { *m() { yield this.v; }, v: 4 };
        let notConstructor;
        try { new g(); } catch (e) { notConstructor = e.constructor.name; }
        [Object.getPrototypeOf(it) === g.prototype,
         Object.getPrototypeOf(g.prototype) === GeneratorPrototype,
         IteratorPrototype[Symbol.iterator].call(it) === it,
         Object.prototype.toString.call(it),
         g.hasOwnProperty('caller'),
         notConstructor,
         obj.m().next().value] + '';";
    assert_eq!(
        run(source),
        "true,true,true,[object Generator],false,TypeError,4"
    );
}

#[test]
fn generator_parameters_are_bound_at_call_time_and_running_generators_throw() {
    let source = "
        let count = 0;
        function* g(a = ++count) { yield a; }
        const it = g();
        const before = count;
        function* self() { yield it2.next(); }
        const it2 = self();
        let reentry;
        try { it2.next(); } catch (e) { reentry = e.constructor.name; }
        [before, it.next().value, reentry] + '';";
    assert_eq!(run(source), "1,1,TypeError");
}

#[test]
fn async_function_runs_synchronously_until_the_first_await() {
    let setup = "
        async function f(x) { log.push('start ' + x); const v = await x; log.push('got ' + v); return v * 2; }
        const p = f(21);
        log.push('sync ' + (p instanceof Promise));
        p.then(v => log.push('result ' + v));";
    assert_eq!(log_after(setup), "start 21,sync true,got 21,result 42");
}

#[test]
fn await_interleaves_with_other_jobs_in_spec_order() {
    let setup = "
        async function a() { log.push('a1'); await null; log.push('a2'); await null; log.push('a3'); }
        async function b() { log.push('b1'); await null; log.push('b2'); }
        a(); b();
        Promise.resolve().then(() => log.push('p'));";
    assert_eq!(log_after(setup), "a1,b1,a2,b2,p,a3");
}

#[test]
fn rejections_throw_at_the_await_and_throws_reject_the_promise() {
    let setup = "
        async function boom() { await null; throw new Error('bad'); }
        async function catcher() {
            try { await boom(); } catch (e) { log.push('caught ' + e.message); }
            finally { log.push('finally'); }
            return 'ok';
        }
        catcher().then(v => log.push(v));
        async function param(a = (() => { throw new TypeError('param'); })()) {}
        let sync = 'no throw';
        try { param().catch(e => log.push('rejected ' + e.message)); } catch (e) { sync = 'threw'; }
        log.push(sync);";
    assert_eq!(
        log_after(setup),
        "no throw,rejected param,caught bad,finally,ok"
    );
}

#[test]
fn async_arrows_and_methods_keep_this_and_await_thenables() {
    let setup = "
        const obj = {
            v: 5,
            async m() { const arrow = async () => this.v + await { then(r) { r(1); } }; return arrow(); },
        };
        class C { async get() { return await Promise.resolve('class'); } }
        obj.m().then(v => log.push(v));
        new C().get().then(v => log.push(v));
        const AsyncFunctionPrototype = Object.getPrototypeOf(async function () {});
        log.push(Object.prototype.toString.call(AsyncFunctionPrototype));
        log.push((async () => {}).hasOwnProperty('prototype'));";
    assert_eq!(log_after(setup), "[object AsyncFunction],false,class,6");
}

#[test]
fn async_functions_await_inside_loops() {
    let setup = "
        async function sum(values) {
            let total = 0;
            for (const v of values) total += await v;
            let i = 0;
            while (i < 3) { total += await Promise.resolve(i); i++; }
            return total;
        }
        sum([1, Promise.resolve(2), 3]).then(v => log.push(v));";
    assert_eq!(log_after(setup), "9");
}

#[test]
fn async_generator_queues_requests_and_awaits_yielded_values() {
    let setup = "
        async function* g() {
            const x = yield Promise.resolve(1);
            log.push('sent ' + x);
            yield 2;
            return 'end';
        }
        const it = g();
        const show = r => log.push(r.value + ':' + r.done);
        it.next('lost').then(show);
        it.next('x').then(show);
        it.next().then(show);
        it.next().then(show);
        log.push('sync ' + (Symbol.asyncIterator in it));";
    assert_eq!(
        log_after(setup),
        // AsyncGeneratorYield resumes at once while requests are queued.
        "sync true,sent x,1:false,2:false,end:true,undefined:true"
    );
}

#[test]
fn async_generator_return_and_throw_requests() {
    let setup = "
        async function* g() {
            try { yield 1; yield 2; } finally { log.push('finally'); }
        }
        const a = g();
        a.next().then(r => log.push('a ' + r.value));
        a.return(Promise.resolve('ret')).then(r => log.push('a ' + r.value + ':' + r.done));
        const b = g();
        b.throw(new Error('early')).catch(e => log.push('b ' + e.message));
        const c = g();
        c.return('fresh').then(r => log.push('c ' + r.value + ':' + r.done));
        g.prototype.next.call({}).catch(e => log.push(e.constructor.name));";
    assert_eq!(
        log_after(setup),
        "b early,TypeError,a 1,finally,c fresh:true,a ret:true"
    );
}

#[test]
fn for_await_consumes_async_and_sync_iterables() {
    let setup = "
        async function* source() { yield 'a'; yield 'b'; }
        async function run() {
            for await (const v of source()) log.push(v);
            for await (const v of [Promise.resolve(1), 2]) log.push(v);
            async function* delegating() { yield* source(); yield* [3]; }
            for await (const v of delegating()) log.push(v);
            return 'done';
        }
        run().then(v => log.push(v));";
    assert_eq!(log_after(setup), "a,b,1,2,a,b,3,done");
}

#[test]
fn for_await_break_closes_and_rejections_propagate() {
    let setup = "
        async function* source() {
            try { yield 1; yield 2; } finally { log.push('closed'); }
        }
        async function early() {
            for await (const v of source()) { log.push(v); break; }
        }
        async function failing() {
            try { for await (const v of [Promise.reject(new Error('no'))]) log.push(v); }
            catch (e) { log.push('caught ' + e.message); }
        }
        early().then(() => failing());";
    assert_eq!(log_after(setup), "1,closed,caught no");
}

#[test]
fn deep_async_recursion_does_not_nest_the_rust_stack() {
    let setup = "
        async function depth(n) { return n === 0 ? 0 : 1 + await depth(n - 1); }
        depth(500).then(v => log.push(v));";
    assert_eq!(log_after(setup), "500");
}

#[test]
fn suspended_frames_survive_garbage_collection() {
    let setup = "
        function* keep() { const held = { v: 'kept' }; yield 1; yield held.v; }
        const it = keep();
        it.next();
        async function waiter() { const held = ['alive']; await null; return held[0]; }
        const p = waiter();
        let junk = [];
        for (let i = 0; i < 200000; i++) junk.push({ i });
        junk = null;
        log.push(it.next().value);
        p.then(v => log.push(v));";
    assert_eq!(log_after(setup), "kept,alive");
}

#[test]
fn iterator_helpers_are_lazy_and_chainable() {
    let source = "
        const log = [];
        function* nat() { let i = 0; while (true) { log.push('pull ' + i); yield i++; } }
        const it = nat().filter(n => n % 2 === 0).map((n, i) => n * 10 + i).drop(1).take(2);
        const before = log.length;
        const out = it.toArray();
        [before, out.join('-'), log.length, Object.prototype.toString.call(nat().map(x => x))] + '';";
    assert_eq!(run(source), "0,21-42,5,[object Iterator Helper]");
}

#[test]
fn iterator_helper_consumers() {
    let source = "
        const it = () => [1, 2, 3, 4].values();
        const seen = [];
        it().forEach((v, i) => seen.push(v + '@' + i));
        [it().reduce((a, b) => a + b),
         it().reduce((a, b) => a + b, 10),
         it().some(v => v > 3), it().every(v => v > 3),
         it().find(v => v > 2),
         it().flatMap(v => [v, v]).toArray().join(''),
         seen.join(' ')] + '';";
    assert_eq!(run(source), "10,20,true,false,3,11223344,1@0 2@1 3@2 4@3");
}

#[test]
fn iterator_helpers_close_the_underlying_iterator() {
    let source = "
        const log = [];
        function* src() { try { yield 1; yield 2; yield 3; } finally { log.push('closed'); } }
        src().some(v => v === 1);
        const mapped = src().map(v => v);
        mapped.next();
        mapped.return();
        src().take(1).toArray();
        let error;
        try { src().map(v => { throw new Error('x'); }).next(); } catch (e) { error = e.message; }
        const observed = Iterator.from({ next() { return { done: false }; },
            return() { log.push('closed'); return {}; } });
        observed.filter(Boolean).return();
        [log.length, error] + '';";
    assert_eq!(run(source), "5,x");
}

#[test]
fn iterator_from_and_the_iterator_constructor() {
    let source = "
        const plain = { i: 0, next() { return { value: this.i++, done: this.i > 3 }; } };
        const wrapped = Iterator.from(plain);
        const gen = (function* () { yield 1; })();
        let abstract;
        try { new Iterator(); } catch (e) { abstract = e.constructor.name; }
        class Counter extends Iterator { next() { return { value: 1, done: false }; } }
        let badLimit;
        try { [].values().take(-1); } catch (e) { badLimit = e.constructor.name; }
        [wrapped.map(x => x * 2).toArray().join(),
         Iterator.from(gen) === gen,
         Iterator.from('ab').toArray().join('|'),
         Iterator.prototype === Object.getPrototypeOf(Object.getPrototypeOf([].values())),
         typeof Iterator.prototype.map,
         Iterator.prototype.map.length,
         abstract,
         new Counter().take(2).toArray().join(),
         badLimit] + '';";
    assert_eq!(
        run(source),
        "0,2,4,true,a|b,true,function,1,TypeError,1,1,RangeError"
    );
}

#[test]
fn coroutine_function_constructors() {
    let setup = "
        const [GF, AF, AGF] = [function* () {}, async function () {}, async function* () {}]
            .map(f => Object.getPrototypeOf(f).constructor);
        const g = GF('a', 'yield a; yield a * 2;'), p = Object.getOwnPropertyDescriptor;
        log.push(GF.name, AF.name, AGF.name, GF.length, Object.getPrototypeOf(AGF) === Function,
            [...g(3)].join('|'), g instanceof GF, new GF('yield 1')().next().value,
            p(GF.prototype, 'constructor').writable, p(AF, 'prototype').writable);
        AF('x', 'return await x')(5).then(v => log.push(v));
        AGF('yield 7')().next().then(r => log.push(r.value));";
    let expected = "GeneratorFunction,AsyncFunction,AsyncGeneratorFunction,1,true,3|6,true,1";
    assert_eq!(log_after(setup), format!("{expected},false,false,5,7"));
}

#[test]
fn for_await_and_async_yield_star_await_the_return_promise() {
    let setup = "
        const iterable = () => ({ [Symbol.asyncIterator]: () => ({ next: () => ({ done: false }),
            return() { log.push('return'); return Promise.resolve().then(() => 0)
                .then(() => { log.push('settled'); return {}; }); } }) });
        (async () => { for await (const x of iterable()) break; log.push('break'); })()
        .then(async () => { try { for await (const x of iterable()) throw 'e'; }
            catch (e) { log.push('caught ' + e); }
            const it = (async function* () { yield* iterable(); })(); await it.next();
            await it.throw('t').catch(e => log.push(e.constructor.name)); });";
    let expected = "return,settled,break,return,settled,caught e,return,settled,TypeError";
    assert_eq!(log_after(setup), expected);
}

#[test]
fn async_from_sync_without_throw_closes_and_rejects() {
    let setup = "
        const sync = { [Symbol.iterator]: () => ({ next: () => ({ done: false }),
            return: (...a) => (log.push('return ' + a.length), {}) }) };
        const it = (async function* () { yield* sync; })();
        it.next().then(() => it.throw('x')).catch(e => log.push(e.constructor.name));";
    assert_eq!(log_after(setup), "return 0,TypeError");
}

#[test]
fn sync_yield_star_forwards_the_inner_result_object() {
    let source = "
        let reads = 0, result = { done: false, get value() { return ++reads; } };
        const r = (function* () { yield* { [Symbol.iterator]: () => ({ next: () => result }) }; })();
        [r.next() === result, reads] + '';";
    assert_eq!(run(source), "true,0");
}

#[test]
fn async_yield_star_awaits_a_return_resumption() {
    let setup = "
        const inner = { [Symbol.asyncIterator]() { return this; }, next: () => ({ done: false }),
            return: v => (log.push('return ' + v), { done: true }),
            throw: e => (log.push('throw ' + e), { done: true }) };
        const g = async function* () { yield* inner; }, a = g(), b = g();
        a.next().then(() => a.return(Promise.resolve('x'))).then(() => b.next())
            .then(() => b.return(Promise.reject('y')));";
    assert_eq!(log_after(setup), "return x,throw y");
}

#[test]
fn iterator_helpers_look_native_and_constructor_is_an_accessor() {
    let source = "
        const d = Object.getOwnPropertyDescriptor(Iterator.prototype, 'constructor');
        let o = Object.create(Iterator.prototype), error = (o.constructor = 1);
        try { d.set.call(Iterator.prototype, 2); } catch (e) { error = e.constructor.name; }
        [String(Iterator.prototype.map).includes('[native code]'), String(Iterator).includes('native'),
         typeof d.get, d.get.call(1) === Iterator, o.constructor, error] + '';";
    assert_eq!(run(source), "true,true,function,true,1,TypeError");
}
