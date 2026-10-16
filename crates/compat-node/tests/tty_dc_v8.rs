//! `node:tty`, `node:diagnostics_channel` and `node:v8`, through `common::script` on V8.

mod common;

use common::script::{CHECKS, assert_ok, run};

const PRELUDE: &str = r#"
const req = (name) => globalThis[Symbol.for('velyr.node')].require(name);
const code = (fn, expected) => { try { fn(); } catch (e) { eq(e.code, expected, 'error code'); return e; } throw new Error('did not throw ' + expected); };
"#;

fn script(body: &str) {
    assert_ok(run(&format!("{CHECKS}{PRELUDE}{body}")));
}

#[test]
fn isatty_answers_for_the_standard_fds_and_false_for_anything_else() {
    script(
        r#"
        const tty = req('tty');
        for (const fd of [0, 1, 2]) eq(typeof tty.isatty(fd), 'boolean', `fd ${fd}`);
        for (const fd of [99, -1, 1.5, '1', null, undefined, NaN]) eq(tty.isatty(fd), false, String(fd));
    "#,
    );
}

#[test]
fn write_stream_reports_a_size_and_derives_color_depth_from_the_env() {
    script(
        r#"
        const { WriteStream, ReadStream } = req('tty');
        const out = new WriteStream(1);
        eq(out.isTTY, true);
        eq(out.fd, 1);
        eq(typeof out.columns, 'number');
        eq(typeof out.rows, 'number');
        same(out.getWindowSize(), [out.columns, out.rows]);
        const depth = (env) => out.getColorDepth(env);
        eq(depth({}), 1);
        eq(depth({ NO_COLOR: '1' }), 1);
        eq(depth({ TERM: 'dumb' }), 1);
        eq(depth({ FORCE_COLOR: '0' }), 1);
        eq(depth({ FORCE_COLOR: '1' }), 4);
        eq(depth({ FORCE_COLOR: '2' }), 8);
        eq(depth({ FORCE_COLOR: '3' }), 24);
        eq(depth({ FORCE_COLOR: '3', NO_COLOR: '1' }), 24);
        eq(depth({ COLORTERM: 'truecolor' }), 24);
        eq(depth({ TERM: 'xterm-256color' }), 8);
        eq(depth({ TERM: 'xterm' }), 4);
        eq(out.hasColors({ FORCE_COLOR: '3' }), true);
        eq(out.hasColors(256, { FORCE_COLOR: '2' }), true);
        eq(out.hasColors(256, { FORCE_COLOR: '1' }), false);
        eq(Object.getPrototypeOf(out).hasOwnProperty('getColorDepth'), true);
        eq(new WriteStream(2).fd, 2);
        code(() => new WriteStream(7), 'ERR_VELYR_UNSUPPORTED');
        code(() => new WriteStream('1'), 'ERR_INVALID_ARG_TYPE');
        const input = new ReadStream(0);
        eq(input.isTTY, true);
        eq(input.isRaw, false);
        eq(input.setRawMode(true), input);
        eq(input.isRaw, true);
        code(() => new ReadStream(1), 'ERR_VELYR_UNSUPPORTED');
    "#,
    );
}

#[test]
fn read_stream_ends_like_the_empty_stdin_and_write_stream_writes_through() {
    script(
        r#"
        const { ReadStream, WriteStream } = req('tty');
        let chunks = 0;
        for await (const _ of new ReadStream(0)) chunks++;
        eq(chunks, 0);
        const out = new WriteStream(1);
        await new Promise((resolve, reject) => out.write('', (err) => (err ? reject(err) : resolve())));
    "#,
    );
}

#[test]
fn channels_are_shared_by_name_and_deliver_message_and_name() {
    script(
        r#"
        const dc = req('diagnostics_channel');
        const ch = dc.channel('app:event');
        eq(dc.channel('app:event'), ch);
        eq(ch instanceof dc.Channel, true);
        eq(ch.name, 'app:event');
        eq(ch.hasSubscribers, false);
        eq(dc.hasSubscribers('app:event'), false);
        const seen = [];
        const fn = (message, name) => seen.push([message, name]);
        ch.subscribe(fn);
        eq(ch.hasSubscribers, true);
        eq(dc.hasSubscribers('app:event'), true);
        ch.publish({ a: 1 });
        const viaModule = (message) => seen.push(['module', message]);
        dc.subscribe('app:event', viaModule);
        ch.publish('x');
        same(seen, [[{ a: 1 }, 'app:event'], ['x', 'app:event'], ['module', 'x']]);
        eq(ch.unsubscribe(fn), true);
        eq(ch.unsubscribe(fn), false);
        eq(dc.unsubscribe('app:event', viaModule), true);
        eq(ch.hasSubscribers, false);
        eq(dc.hasSubscribers('never:created'), false);
        const sym = Symbol('s');
        eq(dc.channel(sym).name, sym);
        code(() => dc.channel(5), 'ERR_INVALID_ARG_TYPE');
        code(() => ch.subscribe('nope'), 'ERR_INVALID_ARG_TYPE');
        code(() => dc.subscribe('app:event', 1), 'ERR_INVALID_ARG_TYPE');
    "#,
    );
}

#[test]
fn a_throwing_subscriber_does_not_stop_the_others_or_the_publisher() {
    script(
        r#"
        const dc = req('diagnostics_channel');
        const ch = dc.channel('app:throws');
        const uncaught = new Promise((resolve) => process.once('uncaughtException', resolve));
        let after = 0;
        ch.subscribe(() => { throw new Error('subscriber failed'); });
        ch.subscribe(() => { after++; });
        ch.publish(1);
        eq(after, 1);
        eq((await uncaught).message, 'subscriber failed');
    "#,
    );
}

#[test]
fn bind_store_and_run_stores_use_async_local_storage() {
    script(
        r#"
        const dc = req('diagnostics_channel');
        const { AsyncLocalStorage } = req('async_hooks');
        const als = new AsyncLocalStorage();
        const plain = new AsyncLocalStorage();
        const ch = dc.channel('app:store');
        eq(ch.hasSubscribers, false);
        ch.bindStore(als, (data) => ({ wrapped: data }));
        ch.bindStore(plain);
        eq(ch.hasSubscribers, true);
        let inSubscriber;
        ch.subscribe(() => { inSubscriber = als.getStore(); });
        const result = ch.runStores('d', function (a, b) {
          same(als.getStore(), { wrapped: 'd' });
          eq(plain.getStore(), 'd');
          eq(this.tag, 'T');
          return a + b;
        }, { tag: 'T' }, 1, 2);
        eq(result, 3);
        eq(als.getStore(), undefined);
        same(inSubscriber, { wrapped: 'd' });
        await als.run('outer', async () => {
          await ch.runStores('x', async () => { await sleep(1); same(als.getStore(), { wrapped: 'x' }); });
          eq(als.getStore(), 'outer');
        });
        eq(ch.unbindStore(als), true);
        eq(ch.unbindStore(als), false);
        ch.runStores('y', () => eq(als.getStore(), undefined));
        code(() => ch.bindStore({}), 'ERR_INVALID_ARG_TYPE');
    "#,
    );
}

#[test]
fn trace_sync_publishes_start_end_and_error_around_the_call() {
    script(
        r#"
        const dc = req('diagnostics_channel');
        const tc = dc.tracingChannel('app:sync');
        eq(dc.channel('tracing:app:sync:start'), tc.start);
        eq(dc.channel('tracing:app:sync:asyncEnd'), tc.asyncEnd);
        eq(tc.hasSubscribers, false);
        eq(tc.traceSync(() => 5), 5);
        const log = [];
        const handlers = {};
        for (const name of ['start', 'end', 'asyncStart', 'asyncEnd', 'error']) {
          handlers[name] = (ctx) => log.push([name, ctx.result, ctx.error && ctx.error.message]);
        }
        tc.subscribe(handlers);
        eq(tc.hasSubscribers, true);
        eq(tc.traceSync(function (a) { return this.k + a; }, {}, { k: 1 }, 2), 3);
        same(log, [['start', undefined, undefined], ['end', 3, undefined]]);
        log.length = 0;
        let thrown;
        try { tc.traceSync(() => { throw new Error('boom'); }, {}); } catch (e) { thrown = e; }
        eq(thrown.message, 'boom');
        same(log.map((e) => e[0]), ['start', 'error', 'end']);
        eq(tc.unsubscribe(handlers), true);
        eq(tc.hasSubscribers, false);
        same(dc.tracingChannel({ start: tc.start, end: tc.end, asyncStart: tc.asyncStart, asyncEnd: tc.asyncEnd, error: tc.error }).start === tc.start, true);
        code(() => dc.tracingChannel({ start: tc.start }), 'ERR_INVALID_ARG_TYPE');
    "#,
    );
}

#[test]
fn trace_promise_and_trace_callback_publish_the_async_events() {
    script(
        r#"
        const dc = req('diagnostics_channel');
        const tc = dc.tracingChannel('app:async');
        const log = [];
        const handlers = {};
        for (const name of ['start', 'end', 'asyncStart', 'asyncEnd', 'error']) {
          handlers[name] = (ctx) => log.push(name + ':' + (ctx.error ? ctx.error.message : ctx.result));
        }
        tc.subscribe(handlers);
        const value = await tc.tracePromise(async () => { await sleep(1); return 'r'; }, {});
        eq(value, 'r');
        same(log, ['start:undefined', 'end:undefined', 'asyncStart:r', 'asyncEnd:r']);
        log.length = 0;
        await tc.tracePromise(() => Promise.reject(new Error('no')), {}).then(
          () => { throw new Error('should reject'); },
          (e) => eq(e.message, 'no'));
        same(log, ['start:undefined', 'end:undefined', 'error:no', 'asyncStart:no', 'asyncEnd:no']);
        log.length = 0;
        const done = new Promise((resolve) => {
          tc.traceCallback((a, cb) => setTimeout(() => cb(null, a * 2), 1), -1, {}, null, 21, (err, res) => resolve([err, res]));
        });
        same(await done, [null, 42]);
        same(log, ['start:undefined', 'end:undefined', 'asyncStart:42', 'asyncEnd:42']);
        code(() => tc.traceCallback((cb) => cb(), 0, {}, null, 'not a function'), 'ERR_INVALID_ARG_TYPE');
    "#,
    );
}

#[test]
fn get_heap_statistics_returns_node_shaped_numbers() {
    script(
        r#"
        const v8 = req('v8');
        const stats = v8.getHeapStatistics();
        for (const key of ['total_heap_size', 'total_heap_size_executable', 'total_physical_size',
          'total_available_size', 'used_heap_size', 'heap_size_limit', 'malloced_memory',
          'peak_malloced_memory', 'does_zap_garbage', 'number_of_native_contexts',
          'number_of_detached_contexts', 'total_global_handles_size', 'used_global_handles_size',
          'external_memory']) {
          eq(typeof stats[key], 'number', key);
          eq(stats[key] >= 0, true, key);
        }
        eq(stats.heap_size_limit > 0, true, 'limit');
        eq(typeof v8.cachedDataVersionTag(), 'number');
    "#,
    );
}

#[test]
fn serialize_round_trips_through_the_structured_clone() {
    script(
        r#"
        const v8 = req('v8');
        const { Buffer } = req('buffer');
        const value = { a: 1, list: [1, 'two', { c: new Date(5) }], m: new Map([[1, 'x']]), s: new Set([1]), n: null, big: 10n };
        const bytes = v8.serialize(value);
        eq(Buffer.isBuffer(bytes), true);
        const back = v8.deserialize(bytes);
        eq(back.a, 1);
        eq(back.list[2].c.getTime(), 5);
        eq(back.m.get(1), 'x');
        eq(back.s.has(1), true);
        eq(back.big, 10n);
        eq(v8.deserialize(new Uint8Array(bytes)).a, 1);
        let failed;
        try { v8.serialize(() => {}); } catch (e) { failed = e; }
        eq(failed instanceof Error, true);
        code(() => v8.deserialize('text'), 'ERR_INVALID_ARG_TYPE');
    "#,
    );
}

#[test]
fn unsupported_parts_throw_and_set_flags_only_warns() {
    script(
        r#"
        const v8 = req('v8');
        for (const name of ['getHeapSnapshot', 'writeHeapSnapshot', 'getHeapSpaceStatistics',
          'getHeapCodeStatistics', 'setHeapSnapshotNearHeapLimit']) {
          code(() => v8[name](), 'ERR_VELYR_UNSUPPORTED');
        }
        for (const name of ['Serializer', 'Deserializer', 'DefaultSerializer', 'DefaultDeserializer']) {
          code(() => new v8[name](), 'ERR_VELYR_UNSUPPORTED');
        }
        eq(v8.startupSnapshot.isBuildingSnapshot(), false);
        code(() => v8.startupSnapshot.addSerializeCallback(() => {}), 'ERR_VELYR_UNSUPPORTED');
        const warned = new Promise((resolve) => process.once('warning', resolve));
        eq(v8.setFlagsFromString('--max-old-space-size=100'), undefined);
        eq((await warned).name, 'V8FlagsWarning');
        code(() => v8.setFlagsFromString(1), 'ERR_INVALID_ARG_TYPE');
    "#,
    );
}

#[test]
fn the_new_builtins_are_listed_and_resolvable() {
    script(
        r#"
        const node = globalThis[Symbol.for('velyr.node')];
        for (const name of ['tty', 'diagnostics_channel', 'v8', 'console']) eq(node.has(name), true, name);
        const names = req('module').builtinModules;
        for (const name of ['tty', 'diagnostics_channel', 'v8', 'console']) eq(names.includes(name), true, `builtinModules ${name}`);
    "#,
    );
}

#[test]
fn console_module_has_a_console_class_that_writes_to_the_given_streams() {
    script(
        r#"
        const { Console } = req('console');
        eq(typeof Console, 'function');
        eq(req('console').log, console.log);
        const out = [];
        const err = [];
        const sink = (lines) => ({ write(chunk) { lines.push(String(chunk)); return true; } });
        const c = new Console({ stdout: sink(out), stderr: sink(err) });
        const { log } = c;
        log('a %s', 'b', 3);
        c.info('i');
        c.debug('d');
        c.error('e');
        c.warn('w');
        c.assert(false, 'nope %d', 1);
        c.assert(true, 'silent');
        c.group('g');
        c.log('inner\nline');
        c.groupEnd();
        c.log('outer');
        c.count();
        c.count('x');
        c.count();
        c.countReset();
        c.count();
        c.dir({ a: { b: { c: { d: 1 } } } }, { depth: 0 });
        same(out, ['a b 3\n', 'i\n', 'd\n', 'g\n', '  inner\n  line\n', 'outer\n', 'default: 1\n',
          'x: 1\n', 'default: 2\n', 'default: 1\n', "{ a: [Object] }\n"]);
        same(err, ['e\n', 'w\n', 'Assertion failed: nope 1\n']);
        const both = new Console(sink(out));
        both.error('to stdout');
        eq(out[out.length - 1], 'to stdout\n');
        code(() => new Console({}), 'ERR_INVALID_ARG_TYPE');
        code(() => new Console({ stdout: sink(out) }).table([]), 'ERR_VELYR_UNSUPPORTED');
    "#,
    );
}
