//! `node:async_hooks`: `AsyncLocalStorage`, `AsyncResource` and the `createHook` stub, end to end
//! under the real event loop on both engines, which carry the store across `await` through
//! their continuation context. Each case runs as `v8::<case>` and `native::<case>`.

mod common;

use common::script::{CHECKS, EngineKind, Net, assert_ok, run_on};

const PRELUDE: &str = "const { AsyncLocalStorage, AsyncResource, createHook } = \
    globalThis[Symbol.for('velyr.node')].require('async_hooks');\n";

fn als(engine: EngineKind, body: &str, net: Net) {
    assert_ok(run_on(engine, &format!("{CHECKS}{PRELUDE}{body}"), net));
}

/// Each case is a script body; this runs it on V8 and on the native engine.
macro_rules! on_both_engines {
    ($($case:ident: $net:ident),* $(,)?) => {
        mod v8 {
            $(#[test] fn $case() { super::als(super::EngineKind::V8, super::$case(), super::Net::$net) })*
        }
        mod native {
            $(#[test] fn $case() { super::als(super::EngineKind::Native, super::$case(), super::Net::$net) })*
        }
    };
}

on_both_engines!(
    run_scopes_the_store_and_restores_it_even_when_the_callback_throws: Off,
    the_store_survives_await_and_reaches_a_timer_set_afterwards: Off,
    the_store_follows_timers_intervals_ticks_immediates_and_microtasks: Off,
    the_store_follows_an_op_completion: Off,
    concurrent_runs_keep_their_own_store: Off,
    enter_with_lasts_for_the_rest_of_the_execution: Off,
    snapshot_bind_and_async_resource_capture_the_current_store: Off,
    create_hook_is_a_stub_that_warns: Off,
    two_concurrent_http_requests_do_not_share_a_store: Loopback,
);

fn run_scopes_the_store_and_restores_it_even_when_the_callback_throws() -> &'static str {
    r#"
        const als = new AsyncLocalStorage();
        eq(als.getStore(), undefined);
        const result = als.run('a', () => {
          eq(als.getStore(), 'a');
          als.run('b', () => eq(als.getStore(), 'b'));
          eq(als.getStore(), 'a');
          als.exit(() => eq(als.getStore(), undefined));
          eq(als.getStore(), 'a');
          return 42;
        });
        eq(result, 42);
        eq(als.getStore(), undefined);
        eq(als.run(1, (a, b) => a + b, 2, 3), 5);
        let thrown;
        try { als.run(1, () => { throw new Error('x'); }); } catch (e) { thrown = e; }
        eq(thrown && thrown.message, 'x');
        eq(als.getStore(), undefined);
        const other = new AsyncLocalStorage();
        als.run(1, () => other.run(2, () => { eq(als.getStore(), 1); eq(other.getStore(), 2); }));
        als.run(1, () => { als.disable(); eq(als.getStore(), undefined); });
    "#
}

fn the_store_survives_await_and_reaches_a_timer_set_afterwards() -> &'static str {
    r#"
        const als = new AsyncLocalStorage();
        const seen = await new Promise((resolve) => {
          als.run(1, async () => { await sleep(5); setTimeout(() => resolve(als.getStore()), 1); });
        });
        eq(seen, 1);
        eq(als.getStore(), undefined);
    "#
}

fn the_store_follows_timers_intervals_ticks_immediates_and_microtasks() -> &'static str {
    r#"
        const als = new AsyncLocalStorage();
        const got = {};
        await als.run('S', () => new Promise((resolve) => {
          let pending = 5;
          const done = (name) => { got[name] = als.getStore(); if (--pending === 0) resolve(); };
          setTimeout(() => done('timeout'), 1);
          const interval = setInterval(() => { clearInterval(interval); done('interval'); }, 1);
          setImmediate(() => done('immediate'));
          process.nextTick(() => done('tick'));
          queueMicrotask(() => done('microtask'));
        }));
        for (const name of ['timeout', 'interval', 'immediate', 'tick', 'microtask']) eq(got[name], 'S', name);
        eq(als.getStore(), undefined);
    "#
}

fn the_store_follows_an_op_completion() -> &'static str {
    r#"
        const als = new AsyncLocalStorage();
        const seen = await als.run('io', async () => { await test_io(); return als.getStore(); });
        eq(seen, 'io');
    "#
}

fn concurrent_runs_keep_their_own_store() -> &'static str {
    r#"
        const als = new AsyncLocalStorage();
        const out = await Promise.all([1, 2, 3].map((id) => als.run(id, async () => {
          await sleep(10 - id * 2);
          const before = als.getStore();
          await sleep(1);
          return [before, als.getStore()];
        })));
        same(out, [[1, 1], [2, 2], [3, 3]]);
    "#
}

fn enter_with_lasts_for_the_rest_of_the_execution() -> &'static str {
    r#"
        const als = new AsyncLocalStorage();
        async function enter() { als.enterWith('E'); await sleep(1); return als.getStore(); }
        eq(await enter(), 'E');
        eq(als.getStore(), 'E');
    "#
}

fn snapshot_bind_and_async_resource_capture_the_current_store() -> &'static str {
    r#"
        const als = new AsyncLocalStorage();
        const snapshot = als.run('snap', () => AsyncLocalStorage.snapshot());
        eq(snapshot(() => als.getStore()), 'snap');
        eq(als.getStore(), undefined);
        const bound = als.run('bound', () => AsyncLocalStorage.bind(() => als.getStore()));
        eq(bound(), 'bound');
        const resource = als.run('res', () => new AsyncResource('X'));
        eq(resource.runInAsyncScope(() => als.getStore()), 'res');
        const fromResource = als.run('rb', () => AsyncResource.bind(() => als.getStore()));
        eq(fromResource(), 'rb');
        eq(als.getStore(), undefined);
    "#
}

fn create_hook_is_a_stub_that_warns() -> &'static str {
    r#"
        const warnings = [];
        process.on('warning', (warning) => warnings.push(warning.message));
        const hook = createHook({ init() {} });
        eq(hook.enable(), hook);
        eq(hook.disable(), hook);
        await sleep(5);
        eq(warnings.some((message) => message.includes('createHook')), true);
    "#
}

fn two_concurrent_http_requests_do_not_share_a_store() -> &'static str {
    r#"
        const http = globalThis[Symbol.for('velyr.node')].require('http');
        const als = new AsyncLocalStorage();
        const server = http.createServer((request, res) => {
          const id = request.url.slice(1);
          als.run(id, async () => {
            await sleep(id === 'a' ? 30 : 5);
            await sleep(1);
            res.end(als.getStore() + ':' + request.url);
          });
        });
        server.listen(0, '127.0.0.1');
        await new Promise((resolve) => server.once('listening', resolve));
        const { port } = server.address();
        const get = (path) => new Promise((resolve, reject) => {
          const request = http.request({ port, host: '127.0.0.1', path }, (res) => {
            let body = '';
            res.on('data', (chunk) => { body += chunk; });
            res.on('end', () => resolve(body));
          });
          request.on('error', reject);
          request.end();
        });
        // An open server keeps the loop alive, so close it even when a check fails.
        try {
          const [a, b] = await Promise.all([get('/a'), get('/b')]);
          eq(a, 'a:/a');
          eq(b, 'b:/b');
          eq(als.getStore(), undefined);
        } finally {
          await new Promise((resolve) => server.close(resolve));
        }
    "#
}
