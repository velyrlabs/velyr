//! `process`, `global`, `setImmediate` and `node:process` end to end, through `common::script`.

mod common;

use common::script::{CHECKS, EXITS, assert_ok, check, grants, run, run_with};

#[test]
fn next_tick_runs_before_promise_microtasks() {
    assert_ok(check(
        r#"const log = [];
        Promise.resolve().then(() => log.push('p'));
        process.nextTick(() => log.push('t'));
        await sleep(5);
        same(log, ['t', 'p']);
        // Called from a microtask (after the await), the queue drains first, as in Node.
        const more = [];
        Promise.resolve().then(() => { more.push('p1'); process.nextTick(() => more.push('t2')); }).then(() => more.push('p2'));
        process.nextTick(() => { more.push('t1'); process.nextTick(() => more.push('t1b')); });
        await sleep(5);
        same(more, ['p1', 'p2', 't1', 't2', 't1b']);
        process.nextTick((a, b) => more.push(a + b), 'x', 'y');
        await sleep(5);
        eq(more[more.length - 1], 'xy');"#,
    ));
}

#[test]
fn env_without_permission_throws_permission_denied() {
    let result = run_with(
        r#"try { process.env.CARGO_PKG_NAME; } catch (e) {
            if (e.code !== 'VELYR_PERMISSION_DENIED') throw e;
            if (String(e.message).includes('CARGO_PKG_NAME') && String(e.message).includes(String(process.env.CARGO_MANIFEST_DIR))) throw new Error('leak');
            return;
        }
        throw new Error('env read was not denied');"#,
        grants(&["CARGO_MANIFEST_DIR"], &[]),
        "",
    );
    assert_ok(result);
    // No grants at all: HOME fails loudly instead of reading as undefined.
    let result = run(r#"let code;
        try { process.env.HOME; } catch (e) { code = e.code; }
        if (code !== 'VELYR_PERMISSION_DENIED') throw new Error('got ' + code);"#);
    assert_ok(result);
}

#[test]
fn env_proxy_reads_enumerates_and_writes_by_key() {
    let result = run_with(
        &format!(
            r#"{CHECKS}
        eq(typeof process.env.CARGO_MANIFEST_DIR, 'string');
        same(Object.keys(process.env), ['CARGO_MANIFEST_DIR']);
        eq('CARGO_MANIFEST_DIR' in process.env, true);
        eq(Object.keys(process.env).includes('PATH'), false);
        for (const denied of ['PATH', 'CARGO_PKG_NAME']) {{
          let code;
          try {{ denied in process.env; }} catch (e) {{ code = e.code; }}
          eq(code, 'VELYR_PERMISSION_DENIED', 'has');
          code = undefined;
          try {{ process.env[denied] = 'x'; }} catch (e) {{ code = e.code; }}
          eq(code, 'VELYR_PERMISSION_DENIED', 'set');
          code = undefined;
          try {{ delete process.env[denied]; }} catch (e) {{ code = e.code; }}
          eq(code, 'VELYR_PERMISSION_DENIED', 'delete');
        }}
        eq(process.env.VELYR_TEST_A, undefined);
        process.env.VELYR_TEST_A = 5;
        eq(process.env.VELYR_TEST_A, '5');
        same(Object.entries(process.env).filter(([k]) => k === 'VELYR_TEST_A'), [['VELYR_TEST_A', '5']]);
        delete process.env.VELYR_TEST_A;
        eq(process.env.VELYR_TEST_A, undefined);
        eq('VELYR_TEST_A' in process.env, false);
        eq(Object.keys(process.env).includes('VELYR_TEST_A'), false);
        // Deleting an inherited variable hides it for this isolate only.
        delete process.env.CARGO_MANIFEST_DIR;
        eq(process.env.CARGO_MANIFEST_DIR, undefined);
        eq(Object.keys(process.env).length, 0);
        eq(process.env[Symbol.iterator], undefined);"#
        ),
        grants(
            &["CARGO_MANIFEST_DIR", "VELYR_TEST_A"],
            &["CARGO_MANIFEST_DIR", "VELYR_TEST_A"],
        ),
        "",
    );
    assert_ok(result);
}

#[test]
fn immediate_runs_before_timeout_zero_inside_an_io_callback() {
    assert_ok(check(
        r#"await test_io();
        const order = [];
        setTimeout(() => order.push('timeout'), 0);
        setImmediate(() => order.push('immediate'));
        await sleep(30);
        same(order, ['immediate', 'timeout']);"#,
    ));
}

#[test]
fn immediate_semantics() {
    assert_ok(check(
        r#"const order = [];
        const a = setImmediate((x, y) => order.push('a' + x + y), 1, 2);
        const cleared = setImmediate(() => order.push('cleared'));
        clearImmediate(cleared);
        setImmediate(() => {
          order.push('b');
          process.nextTick(() => order.push('tick'));
          setImmediate(() => order.push('nested'));
        });
        setImmediate(() => order.push('c'));
        eq(a.hasRef(), true);
        eq(a.unref().hasRef(), false);
        eq(a.ref().hasRef(), true);
        await sleep(20);
        same(order, ['a12', 'b', 'tick', 'c', 'nested']);
        eq(await new Promise((r) => setImmediate(r, 'v')), 'v');
        const util = globalThis[Symbol.for('velyr.node')].require('util');
        eq(await util.promisify(setImmediate)('w'), 'w');
        let code;
        try { setImmediate('nope'); } catch (e) { code = e.code; }
        eq(code, 'ERR_INVALID_ARG_TYPE');
        clearImmediate(undefined);
        const t = globalThis[Symbol.for('velyr.node')].require('timers');
        eq(t.setImmediate, setImmediate);
        eq(t.setTimeout, setTimeout);"#,
    ));
}

#[test]
fn unref_immediate_does_not_keep_the_process_alive() {
    // The unref'd immediate still runs in the first check phase; the run must end.
    assert_ok(check("setImmediate(() => {}).unref();"));
}

#[test]
fn argv_exec_path_and_basic_info() {
    assert_ok(check(
        r#"same(process.argv, ['/opt/velyr/bin/velyr', '/app/main.js', '--flag', 'value']);
        eq(process.execPath, '/opt/velyr/bin/velyr');
        eq(process.argv0, 'node');
        same(process.execArgv, []);
        eq(process.version, 'v24.11.0');
        eq(process.versions.node, '24.11.0');
        eq(typeof process.versions.velyr, 'string');
        eq(process.release.name, 'node');
        eq(process.title, 'node');
        eq(['darwin', 'linux', 'win32'].includes(process.platform), true);
        eq(['x64', 'arm64'].includes(process.arch), true);
        eq(Number.isInteger(process.pid) && process.pid > 0, true);
        eq(Object.prototype.toString.call(process), '[object process]');
        eq(typeof process.on, 'function');
        eq(global, globalThis);
        eq(Object.keys(globalThis).includes('global'), true);
        eq(Object.keys(globalThis).includes('process'), false);
        eq(globalThis[Symbol.for('velyr.node')].require('process'), process);
        eq(typeof process.cwd(), 'string');
        eq(typeof process.uptime(), 'number');"#,
    ));
}

#[test]
fn hrtime_memory_and_uptime() {
    assert_ok(check(
        r#"const t0 = process.hrtime();
        eq(t0.length, 2);
        await sleep(5);
        const d = process.hrtime(t0);
        eq(d[0] === 0 && d[1] > 1e6, true, 'diff ' + d);
        const b0 = process.hrtime.bigint();
        await sleep(2);
        eq(typeof b0, 'bigint');
        eq(process.hrtime.bigint() > b0, true);
        const m = process.memoryUsage();
        eq(m.rss > 0, true);
        for (const k of ['heapTotal', 'heapUsed', 'external', 'arrayBuffers']) eq(typeof m[k], 'number', k);
        eq(process.memoryUsage.rss() > 0, true);
        const u = process.uptime();
        await sleep(3);
        eq(process.uptime() > u, true);
        let code;
        try { process.hrtime('x'); } catch (e) { code = e.code; }
        eq(code, 'ERR_INVALID_ARG_TYPE');"#,
    ));
}

#[test]
fn exit_and_exit_code() {
    assert_ok(check(
        r#"const seen = [];
        process.on('exit', (c) => seen.push(c));
        process.exitCode = '41';
        eq(process.exitCode, 41);
        let code;
        try { process.exitCode = 'abc'; } catch (e) { code = e.code; }
        eq(code, 'ERR_INVALID_ARG_TYPE');
        eq(process.exitCode, 41);
        let err;
        try { process.exit(); } catch (e) { err = e; }
        eq(err.name, 'ProcessExit');
        eq(err.exitCode, 41);
        same(seen, [41]);
        try { process.exit(43); } catch (e) { err = e; }
        eq(err.exitCode, 43);
        same(seen, [41], 'exit emitted once');"#,
    ));
    let exits = EXITS.lock().unwrap();
    assert!(exits.contains(&41) && exits.contains(&43), "{exits:?}");
}

#[test]
fn chdir_errors_and_cwd() {
    assert_ok(check(
        r#"const cwd = process.cwd();
        let err;
        try { process.chdir('/definitely/not/here'); } catch (e) { err = e; }
        eq(err.code, 'ENOENT');
        eq(err.syscall, 'chdir');
        eq(process.cwd(), cwd);
        try { process.chdir(5); } catch (e) { err = e; }
        eq(err.code, 'ERR_INVALID_ARG_TYPE');"#,
    ));
}

#[test]
fn warnings_stdio_and_uncaught_exceptions() {
    assert_ok(check(
        r#"const warnings = [];
        process.on('warning', (w) => warnings.push(`${w.name}|${w.message}|${w.code}`));
        process.emitWarning('careful', 'CustomWarning', 'W001');
        process.emitWarning('old', { type: 'DeprecationWarning', code: 'DEP1' });
        eq(warnings.length, 0, 'warning is async');
        await sleep(3);
        same(warnings, ['CustomWarning|careful|W001', 'DeprecationWarning|old|DEP1']);
        process.noDeprecation = true;
        process.emitWarning('hidden', 'DeprecationWarning');
        await sleep(3);
        eq(warnings.length, 2);

        eq(process.stdout.write(''), true);
        eq(process.stderr.fd, 2);
        eq(process.stdin.fd, 0);
        eq(process.stdout.isTTY, false);
        let called = false;
        process.stdout.write('', () => { called = true; });
        eq(called, false);
        await sleep(3);
        eq(called, true);
        let code;
        try { process.stdout.write(5); } catch (e) { code = e.code; }
        eq(code, 'ERR_INVALID_ARG_TYPE');

        const caught = [];
        process.on('uncaughtException', (e, origin) => caught.push(e.message + '|' + origin));
        process.nextTick(() => { throw new Error('from tick'); });
        setImmediate(() => { throw new Error('from immediate'); });
        await sleep(10);
        same(caught, ['from tick|uncaughtException', 'from immediate|uncaughtException']);"#,
    ));
}

#[test]
fn uncaught_tick_without_listener_fails_the_run_readably() {
    let result =
        check("process.nextTick(() => { throw new Error('tick exploded'); }); await sleep(5);");
    assert!(result.contains("tick exploded"), "{result}");
}

#[test]
fn host_emits_lifecycle_events() {
    let result = run_with(
        "if (globalThis.__seen.join() !== 'beforeExit:0,exit:7,SIGINT,rejection:boom') throw new Error(globalThis.__seen.join());",
        grants(&[], &[]),
        r#"// EMIT
        globalThis.__seen = [];
        process.on('beforeExit', (c) => __seen.push('beforeExit:' + c));
        process.on('exit', (c) => __seen.push('exit:' + c));
        process.on('SIGINT', () => __seen.push('SIGINT'));
        process.on('unhandledRejection', (r) => __seen.push('rejection:' + r));"#,
    );
    assert_ok(result);
}
