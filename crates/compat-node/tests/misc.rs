//! `node:os`, `node:timers` (+ promises), `node:querystring`, `node:zlib`, `node:perf_hooks` and
//! `node:module`, through `common::script` (and an ESM host for `createRequire`).

mod common;

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

use common::script::{CHECKS, assert_ok, check, run_with};
use velyr_compat_commonjs::{commonjs_host_functions, install_commonjs};
use velyr_compat_node::{NodeBuiltinResolver, compat_node_host_functions, install_compat_node};
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_modules::FsResolver;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_web_core::{install_url, url_host_functions};

const PRELUDE: &str = r#"
const { Buffer } = globalThis[Symbol.for('velyr.node')].require('buffer');
const req = (name) => globalThis[Symbol.for('velyr.node')].require(name);
const hex = (b) => Buffer.from(b).toString('hex');
const code = (fn, expected) => { try { fn(); } catch (e) { eq(e.code, expected, 'error code'); return e; } throw new Error('did not throw ' + expected); };
const rejects = async (promise, expected) => { try { await promise; } catch (e) { eq(e.code, expected, 'error code'); return e; } throw new Error('did not reject ' + expected); };
"#;

fn run(body: &str) -> String {
    check(&format!("{PRELUDE}{body}"))
}

/// Pipes `input` through the system `gzip` with `args`.
fn system_gzip(args: &[&str], input: &[u8]) -> Vec<u8> {
    let mut child = Command::new("gzip")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("system gzip");
    let mut stdin = child.stdin.take().unwrap();
    let data = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&data).unwrap());
    let output = child.wait_with_output().unwrap();
    writer.join().unwrap();
    assert!(output.status.success(), "gzip {args:?} failed");
    output.stdout
}

// The expected EOL, devNull and os.type() values are the unix ones.
#[cfg(unix)]
#[test]
fn os_answers_from_the_host_without_env_permission() {
    assert_ok(run(r#"const os = req('os'), proc = req('process');
        eq(os.platform(), proc.platform); eq(os.arch(), proc.arch);
        eq(os.EOL, '\n'); eq(os.endianness(), 'LE'); eq(os.devNull, '/dev/null');
        eq(os.type(), proc.platform === 'darwin' ? 'Darwin' : 'Linux');
        if (os.hostname().length === 0) throw new Error('hostname');
        const cpus = os.cpus();
        if (cpus.length < 1 || typeof cpus[0].model !== 'string') throw new Error('cpus');
        const t = cpus[0].times;
        if ([t.user, t.nice, t.sys, t.idle, t.irq].some((v) => !(v >= 0))) throw new Error('times ' + JSON.stringify(t));
        if (proc.platform === 'linux' && !(t.user + t.sys + t.idle > 0)) throw new Error('linux times are real: ' + JSON.stringify(t));
        if (os.availableParallelism() < 1) throw new Error('parallelism');
        const total = os.totalmem(), free = os.freemem();
        if (!(total > free && free > 0)) throw new Error(`memory ${total} ${free}`);
        if (!(os.uptime() > 0)) throw new Error('uptime');
        if (os.tmpdir().length === 0 || os.homedir().length === 0) throw new Error('dirs');
        eq(typeof os.release(), 'string'); eq(typeof os.version(), 'string'); eq(typeof os.machine(), 'string');
        eq(os.constants.signals.SIGINT, 2); eq(os.constants.signals.SIGKILL, 9); eq(os.constants.errno.ENOENT, 2);
        eq(Object.isFrozen(os.constants), true);"#));
}

#[test]
fn timers_return_timeout_objects() {
    assert_ok(run(r#"const timers = req('timers');
        const log = [];
        const t = timers.setTimeout((a, b) => log.push(a + b), 5, 'x', 'y');
        eq(t.constructor.name, 'Timeout'); eq(t.hasRef(), true); eq(t.unref(), t); eq(t.hasRef(), false); eq(t.ref().hasRef(), true);
        eq(typeof +t, 'number'); eq(globalThis.setTimeout, timers.setTimeout, 'node mode swaps the global');
        await sleep(30);
        same(log, ['xy']);
        // clearTimeout takes the object and its numeric id.
        const gone = timers.setTimeout(() => log.push('gone'), 5);
        timers.clearTimeout(+gone);
        const gone2 = timers.setTimeout(() => log.push('gone2'), 5);
        gone2.close();
        // refresh() re-arms a timer that already fired.
        const again = timers.setTimeout(() => log.push('again'), 5);
        await sleep(30);
        again.refresh();
        await sleep(30);
        same(log, ['xy', 'again', 'again']);
        let ticks = 0;
        const every = timers.setInterval(() => ticks++, 5);
        await sleep(60);
        timers.clearInterval(every);
        if (ticks < 3) throw new Error('interval ticked ' + ticks);
        const seen = ticks; await sleep(30); eq(ticks, seen, 'cleared interval stays quiet');
        code(() => timers.setTimeout('nope'), 'ERR_INVALID_ARG_TYPE');"#));
}

#[test]
fn unref_timers_do_not_keep_the_loop_alive() {
    // A 60s ref'd timer would hang this test, so finishing at all is the assertion.
    assert_ok(run(
        r#"const timers = req('timers'), tp = req('timers/promises');
        const every = timers.setInterval(() => {}, 60000).unref();
        eq(every.hasRef(), false);
        timers.setTimeout(() => {}, 60000).unref();
        const late = timers.setTimeout(() => {}, 60000);
        late.unref().ref().unref();
        tp.setTimeout(60000, 'x', { ref: false });
        tp.setImmediate('x', { ref: false });
        (async () => { for await (const _ of tp.setInterval(60000, 1, { ref: false })) break; })();"#,
    ));
}

#[test]
fn timers_promises_resolve_iterate_and_abort() {
    assert_ok(run(
        r#"const tp = req('timers/promises'), timers = req('timers');
        eq(await tp.setTimeout(5, 'v'), 'v');
        eq(await tp.setImmediate('i'), 'i');
        eq(timers.promises, tp);
        const got = [];
        for await (const value of tp.setInterval(5, 'tick')) { got.push(value); if (got.length === 3) break; }
        same(got, ['tick', 'tick', 'tick']);
        const controller = new AbortController();
        const pending = tp.setTimeout(10000, 'late', { signal: controller.signal });
        controller.abort();
        await rejects(pending, 'ABORT_ERR');
        const aborted = new AbortController(); aborted.abort();
        await rejects(tp.setTimeout(1, 'x', { signal: aborted.signal }), 'ABORT_ERR');
        await tp.scheduler.wait(5);
        await tp.scheduler.yield();
        const { promisify } = req('util');
        eq(await promisify(timers.setTimeout)(5, 'p'), 'p');"#,
    ));
}

#[test]
fn querystring_matches_the_legacy_parser() {
    assert_ok(run(r#"const qs = req('querystring');
        same(qs.parse('a=1&b=2&a=3&c'), { a: ['1', '3'].length ? ['1', '3'] : 0, b: '2', c: '' });
        same(qs.parse('x=%20y+z&&=empty&k=%E4%BD%A0'), { x: ' y z', '': 'empty', k: '你' });
        same(qs.parse('a:1;b:2', ';', ':'), { a: '1', b: '2' });
        same(qs.parse('a=1&b=2&c=3', null, null, { maxKeys: 2 }), { a: '1', b: '2' });
        eq(Object.getPrototypeOf(qs.parse('a=1')), null);
        eq(qs.stringify({ a: [1, 2], b: 'x y', c: true, d: null, e: {}, f: 2n, g: NaN }), 'a=1&a=2&b=x%20y&c=true&d=&e=&f=2&g=');
        eq(qs.stringify({ a: 1, b: 2 }, ';', ':'), 'a:1;b:2');
        eq(qs.stringify('nope'), '');
        eq(qs.escape("a b&c=d!'()*~"), "a%20b%26c%3Dd!'()*~");
        code(() => qs.escape('\ud800'), 'ERR_INVALID_URI');
        eq(qs.unescape('%41%zz'), 'A%zz');
        eq(qs.unescapeBuffer('a+b%41', true).toString(), 'a bA');
        eq(qs.encode, qs.stringify); eq(qs.decode, qs.parse);"#));
}

#[test]
fn zlib_round_trips_every_format_sync_callback_and_stream() {
    assert_ok(run(
        r#"const zlib = req('zlib'), { pipeline } = req('stream');
        const data = Buffer.from('velyr zlib round trip '.repeat(2000));
        const pairs = [['gzip', 'gunzip'], ['deflate', 'inflate'], ['deflateRaw', 'inflateRaw'], ['brotliCompress', 'brotliDecompress']];
        const cap = (s) => s[0].toUpperCase() + s.slice(1);
        for (const [pack, unpack] of pairs) {
          const packed = zlib[pack + 'Sync'](data, pack === 'brotliCompress' ? { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 4 } } : undefined);
          if (!(packed.length < data.length / 4)) throw new Error(pack + ' did not shrink');
          eq(zlib[unpack + 'Sync'](packed).equals(data), true, pack + ' sync');
          // callback form (blocking pool)
          const viaCallback = await new Promise((resolve, reject) => zlib[pack](data, (e, out) => (e ? reject(e) : resolve(out))));
          eq(zlib[unpack + 'Sync'](viaCallback).equals(data), true, pack + ' callback');
          const viaUnpack = await new Promise((resolve, reject) => zlib[unpack](packed, (e, out) => (e ? reject(e) : resolve(out))));
          eq(viaUnpack.equals(data), true, unpack + ' callback');
          // stream form, 777-byte writes through pipeline
          const { Readable, Writable } = req('stream');
          const chunks = [];
          const source = Readable.from((function* () { for (let at = 0; at < data.length; at += 777) yield data.subarray(at, at + 777); })());
          const sink = new Writable({ write(chunk, enc, cb) { chunks.push(chunk); cb(); } });
          await new Promise((resolve, reject) => pipeline(source, zlib['create' + cap(pack)](), zlib['create' + cap(unpack)](), sink, (e) => (e ? reject(e) : resolve())));
          eq(Buffer.concat(chunks).equals(data), true, pack + ' stream');
        }
        // A compressing stream emits output before the input ends (no whole-input buffering).
        const gz = zlib.createGzip();
        let early = 0;
        gz.on('data', (c) => { early += c.length; });
        gz.write(Buffer.alloc(200000, 7));
        gz.flush();
        await sleep(20);
        if (early === 0) throw new Error('gzip stream buffered everything');
        gz.end(); await new Promise((r) => gz.on('end', r).resume());
        eq(zlib.crc32('123456789'), 0xcbf43926); eq(zlib.crc32('56789', zlib.crc32('1234')), 0xcbf43926);
        eq(zlib.crc32(Buffer.from('')), 0);
        eq(zlib.constants.Z_BEST_COMPRESSION, 9);"#,
    ));
}

#[test]
fn zlib_unzip_reset_and_default_brotli_quality() {
    assert_ok(run(r#"const zlib = req('zlib');
        const data = Buffer.from('unzip and reset '.repeat(500));
        for (const pack of ['gzipSync', 'deflateSync']) {
          const packed = zlib[pack](data);
          eq(zlib.unzipSync(packed).equals(data), true, pack + ' unzipSync');
          const viaCallback = await new Promise((resolve, reject) => zlib.unzip(packed, (e, out) => (e ? reject(e) : resolve(out))));
          eq(viaCallback.equals(data), true, pack + ' unzip callback');
          const stream = zlib.createUnzip(); const parts = [];
          stream.on('data', (c) => parts.push(c));
          for (const byte of packed.subarray(0, 5)) stream.write(Buffer.from([byte]));
          stream.end(packed.subarray(5)); await new Promise((r) => stream.on('end', r));
          eq(Buffer.concat(parts).equals(data), true, pack + ' unzip stream');
        }
        code(() => zlib.unzipSync(Buffer.alloc(0)), 'Z_BUF_ERROR');
        code(() => zlib.unzipSync(Buffer.from('not compressed at all')), 'Z_DATA_ERROR');
        // reset(): a gunzip stream that finished a member starts over; a half-fed one forgets its input.
        const gunzip = zlib.createGunzip(); const out = [];
        gunzip.on('data', (c) => out.push(c));
        gunzip.write(zlib.gzipSync('lost').subarray(0, 8));
        gunzip.reset();
        gunzip.end(zlib.gzipSync('kept'));
        await new Promise((r) => gunzip.on('end', r));
        eq(Buffer.concat(out).toString(), 'kept');
        // The default brotli quality still round-trips and shrinks.
        const brotli = zlib.brotliCompressSync(data);
        eq(zlib.brotliDecompressSync(brotli).equals(data), true); if (!(brotli.length < data.length / 4)) throw new Error('brotli');"#));
}

#[test]
fn zlib_dictionary_window_bits_and_params() {
    assert_ok(run(r#"const zlib = req('zlib');
        const dictionary = Buffer.from('shared words that both sides know already');
        const data = Buffer.from('shared words that both sides know already, then news');
        for (const [pack, unpack] of [['deflate', 'inflate'], ['deflateRaw', 'inflateRaw']]) {
          const plain = zlib[pack + 'Sync'](data), withDict = zlib[pack + 'Sync'](data, { dictionary });
          if (!(withDict.length < plain.length)) throw new Error(pack + ' dictionary did not help');
          eq(zlib[unpack + 'Sync'](withDict, { dictionary }).equals(data), true, unpack);
          const viaCallback = await new Promise((resolve, reject) => zlib[unpack](withDict, { dictionary }, (e, out) => (e ? reject(e) : resolve(out))));
          eq(viaCallback.equals(data), true, unpack + ' callback');
          const stream = zlib['create' + pack[0].toUpperCase() + pack.slice(1)]({ dictionary }), parts = [];
          stream.on('data', (c) => parts.push(c)); stream.end(data); await new Promise((r) => stream.on('end', r));
          eq(zlib[unpack + 'Sync'](Buffer.concat(parts), { dictionary }).equals(data), true, pack + ' stream');
        }
        const missing = code(() => zlib.inflateSync(zlib.deflateSync(data, { dictionary })), 'Z_NEED_DICT');
        eq(missing.errno, 2); eq(missing.message, 'Missing dictionary');
        code(() => zlib.inflateSync(zlib.deflateSync(data, { dictionary }), { dictionary: Buffer.from('wrong') }), 'Z_DATA_ERROR');
        code(() => zlib.gzipSync('x', { dictionary }), 'Z_STREAM_ERROR');
        // windowBits: a small window still round-trips, 0 lets decoders take the header's size.
        const big = Buffer.alloc(100000, 'abc');
        const small = zlib.gzipSync(big, { windowBits: 9 });
        eq(zlib.gunzipSync(small).equals(big), true); eq(zlib.gunzipSync(small, { windowBits: 0 }).equals(big), true);
        eq(zlib.inflateSync(zlib.deflateSync(big, { windowBits: 10 }), { windowBits: 0 }).equals(big), true);
        code(() => zlib.deflateSync('x', { windowBits: 7 }), 'ERR_OUT_OF_RANGE');
        // params(): any level before data, then within an algorithm family; across families it fails clearly.
        const gz = zlib.createGzip(), out = [];
        gz.on('data', (c) => out.push(c));
        const params = (...args) => new Promise((resolve) => gz.params(...args, (e) => resolve(e)));
        eq(await params(1, 0), undefined); eq(await params(3, 0), undefined);
        gz.write(Buffer.alloc(5000, 1));
        eq(await params(5, 0), undefined);
        eq((await params(9, 0)).code, 'Z_STREAM_ERROR');
        gz.end(Buffer.alloc(5000, 2)); await new Promise((r) => gz.on('end', r));
        eq(zlib.gunzipSync(Buffer.concat(out)).length, 10000);
        code(() => zlib.createGzip().params(10, 0, () => {}), 'ERR_OUT_OF_RANGE');
        code(() => zlib.createGunzip().params(1, 0, () => {}), 'ERR_VELYR_UNSUPPORTED');"#));
}

#[test]
fn zlib_interoperates_with_the_system_gzip() {
    let plain: Vec<u8> = b"interop with the system gzip\n".repeat(500);
    let from_system = system_gzip(&["-c", "-6"], &plain);
    let multi = [system_gzip(&["-c"], b"one "), system_gzip(&["-c"], b"two")].concat();
    let dir = tempfile::tempdir().unwrap();
    let canonical = dunce::canonicalize(dir.path()).unwrap();
    let dir_text = canonical.to_str().unwrap();
    std::fs::write(canonical.join("system.gz"), &from_system).unwrap();
    std::fs::write(canonical.join("multi.gz"), &multi).unwrap();
    std::fs::write(canonical.join("plain.txt"), &plain).unwrap();
    let body = format!(
        r#"const zlib = req('zlib'), fs = req('fs'), at = (n) => {dir_text:?} + '/' + n;
        const plain = fs.readFileSync(at('plain.txt'));
        eq(zlib.gunzipSync(fs.readFileSync(at('system.gz'))).equals(plain), true, 'system gzip -> gunzip');
        eq(zlib.gunzipSync(fs.readFileSync(at('multi.gz'))).toString(), 'one two', 'concatenated members');
        fs.writeFileSync(at('ours.gz'), zlib.gzipSync(plain));"#
    );
    let capabilities = CapabilitySet::builder()
        .allow(Capability::FsRead(PathPattern::new(dir_text)))
        .allow(Capability::FsWrite(PathPattern::new(dir_text)))
        .build();
    let script = format!("{CHECKS}{PRELUDE}{body}");
    assert_ok(run_with(&script, capabilities, ""));
    let ours = std::fs::read(canonical.join("ours.gz")).unwrap();
    assert_eq!(
        system_gzip(&["-dc"], &ours),
        plain,
        "gzipSync -> system gunzip"
    );
}

#[test]
fn zlib_limits_output_and_reports_damaged_input() {
    assert_ok(run(r#"const zlib = req('zlib');
        const bomb = zlib.gzipSync(Buffer.alloc(4 * 1024 * 1024));
        if (bomb.length > 10000) throw new Error('bomb is not small: ' + bomb.length);
        code(() => zlib.gunzipSync(bomb, { maxOutputLength: 65536 }), 'ERR_BUFFER_TOO_LARGE');
        const viaCallback = await new Promise((resolve) => zlib.gunzip(bomb, { maxOutputLength: 65536 }, (e) => resolve(e)));
        eq(viaCallback.code, 'ERR_BUFFER_TOO_LARGE');
        eq(zlib.gunzipSync(bomb).length, 4 * 1024 * 1024, 'within the default ceiling');
        const stream = zlib.createGunzip({ maxOutputLength: 65536 });
        const failed = new Promise((resolve) => stream.on('error', resolve));
        stream.write(bomb);
        eq((await failed).code, 'ERR_BUFFER_TOO_LARGE', 'stream bomb');
        const short = zlib.deflateSync(Buffer.from('truncated input'));
        const e1 = code(() => zlib.inflateSync(short.subarray(0, short.length - 3)), 'Z_BUF_ERROR');
        eq(e1.errno, -5); eq(e1.message, 'unexpected end of file');
        const e2 = code(() => zlib.gunzipSync(Buffer.from('this is not gzip data')), 'Z_DATA_ERROR');
        eq(e2.errno, -3);
        code(() => zlib.brotliDecompressSync(Buffer.from('plain text is not brotli')), 'Z_DATA_ERROR');
        const flipped = Buffer.from(zlib.gzipSync('checksum')); flipped[flipped.length - 6] ^= 0xff;
        code(() => zlib.gunzipSync(flipped), 'Z_DATA_ERROR');
        code(() => zlib.gzipSync(12), 'ERR_INVALID_ARG_TYPE');
        code(() => zlib.gzipSync('x', { level: 12 }), 'ERR_OUT_OF_RANGE');
        code(() => zlib.deflateSync('x', { dictionary: 'text' }), 'ERR_INVALID_ARG_TYPE');"#));
}

#[test]
fn perf_hooks_expose_performance_and_loop_delay() {
    assert_ok(run(r#"const ph = req('perf_hooks');
        // The script harness does not install web-core's performance: the getters say so clearly.
        eq(globalThis.performance, undefined);
        try { ph.performance; throw new Error('no error'); } catch (e) { eq(e.message.includes('did not install performance'), true); }
        const monitor = ph.monitorEventLoopDelay({ resolution: 5 });
        eq(monitor.enable(), true); eq(monitor.enable(), false);
        await sleep(80);
        eq(monitor.disable(), true);
        if (monitor.count < 3) throw new Error('count ' + monitor.count);
        if (!(monitor.min > 0 && monitor.max >= monitor.min && monitor.mean >= monitor.min && monitor.stddev >= 0)) throw new Error('stats');
        if (!(monitor.percentile(50) <= monitor.percentile(99))) throw new Error('percentiles');
        monitor.reset(); eq(monitor.count, 0);
        const histogram = ph.createHistogram();
        for (const v of [1000, 2000, 3000, 4000]) histogram.record(v);
        eq(histogram.count, 4); eq(histogram.min, 1000); eq(histogram.max, 4000); eq(histogram.mean, 2500);
        eq(histogram.percentile(50), 2000); eq(histogram.percentile(100), 4000);
        code(() => ph.monitorEventLoopDelay({ resolution: 0 }), 'ERR_OUT_OF_RANGE');
        // eventLoopUtilization: cumulative shape, and a diff against an earlier sample.
        const before = ph.eventLoopUtilization();
        let spin = 0; for (let i = 0; i < 3e6; i++) spin += i;
        const diff = ph.eventLoopUtilization(ph.eventLoopUtilization(), before); // later minus earlier
        const now = ph.eventLoopUtilization();
        if (!(now.utilization >= 0 && now.utilization <= 1 && now.active >= before.active)) throw new Error('elu ' + JSON.stringify(now));
        if (!(diff.active >= 0 && diff.utilization >= 0)) throw new Error('elu diff ' + JSON.stringify(diff));
        code(() => ph.eventLoopUtilization(1), 'ERR_INVALID_ARG_TYPE');
        // timerify wraps sync, async and constructor calls and records into the histogram.
        const timed = ph.createHistogram();
        const add = ph.timerify((a, b) => a + b, { histogram: timed });
        eq(add(1, 2), 3); eq(add.name, 'timerified '); eq(add.length, 2);
        eq(await ph.timerify(async () => 'v', { histogram: timed })(), 'v');
        const Box = ph.timerify(class Box { constructor(v) { this.v = v; } }, { histogram: timed });
        eq(new Box(7).v, 7); eq(timed.count, 3);
        code(() => ph.timerify('nope'), 'ERR_INVALID_ARG_TYPE');
        // An enabled monitor left running must not keep the process alive: finishing is the check.
        ph.monitorEventLoopDelay({ resolution: 60000 }).enable();"#));
}

#[test]
fn module_lists_builtins_and_refuses_register() {
    assert_ok(run(r#"const mod = req('module');
        eq(mod.isBuiltin('node:fs'), true); eq(mod.isBuiltin('fs/promises'), true); eq(mod.isBuiltin('zlib'), true);
        eq(mod.isBuiltin('node:nope'), false); eq(mod.isBuiltin(3), false);
        for (const name of ['os', 'timers/promises', 'querystring', 'zlib', 'perf_hooks', 'module', 'path']) {
          eq(mod.builtinModules.includes(name), true, name);
        }
        eq(mod.Module, mod);
        const e = code(() => mod.register('./hooks.mjs'), 'ERR_VELYR_UNSUPPORTED');
        eq(e.message.includes('register'), true);
        code(() => mod.createRequire('relative/path.js'), 'ERR_INVALID_ARG_VALUE');
        // Without the CommonJS loader there is nothing to build a require from: a clear error.
        code(() => mod.createRequire('/tmp/x.js'), 'ERR_VELYR_UNSUPPORTED');"#));
}

#[test]
fn create_require_works_from_esm() {
    let work = tempfile::tempdir().unwrap();
    let facades = tempfile::tempdir().unwrap();
    let (work, facades) = (
        dunce::canonicalize(work.path()).unwrap(),
        dunce::canonicalize(facades.path()).unwrap(),
    );
    std::fs::write(
        work.join("dep.cjs"),
        "module.exports = { answer: 42, here: __filename };",
    )
    .unwrap();
    std::fs::write(
        work.join("main.mjs"),
        r#"import { createRequire, builtinModules, isBuiltin } from 'node:module';
import mod from 'node:module';
import { gzipSync, gunzipSync } from 'node:zlib';
import { parse } from 'node:querystring';
const require = createRequire(import.meta.url);
if (require('node:path').join('a', 'b') !== 'a/b') throw new Error('builtin through require');
if (require('./dep.cjs').answer !== 42) throw new Error('relative cjs through require');
if (createRequire(new URL('./dep.cjs', import.meta.url))('./dep.cjs').answer !== 42) throw new Error('URL form');
if (require('os').EOL !== (require('os').platform() === 'win32' ? '\r\n' : '\n') || !isBuiltin('node:os') || !builtinModules.includes('zlib')) throw new Error('lists');
if (mod.createRequire !== createRequire) throw new Error('default export');
if (gunzipSync(gzipSync('esm')).toString() !== 'esm' || parse('a=1').a !== '1') throw new Error('facades');
"#,
    )
    .unwrap();
    let resolver = NodeBuiltinResolver::new(Arc::new(FsResolver), &facades).unwrap();
    let capabilities = CapabilitySet::builder()
        .allow(Capability::ImportLocal(PathPattern::new(
            work.to_str().unwrap(),
        )))
        .allow(Capability::ImportLocal(PathPattern::new(
            resolver.facade_dir().to_str().unwrap(),
        )))
        .build();
    let config = RuntimeConfig {
        capabilities: Some(capabilities.clone()),
        platform_init: Some(V8Engine::init_platform),
        module_resolver: Some(Arc::new(resolver)),
        ..RuntimeConfig::default()
    };
    let mut host_functions = commonjs_host_functions();
    host_functions.extend(url_host_functions());
    host_functions.extend(compat_node_host_functions());
    run_module_file(
        V8Engine::new(),
        config,
        host_functions,
        "",
        move |isolate, _, _| {
            install_url(isolate).expect("url");
            install_commonjs(isolate, capabilities.clone()).expect("commonjs");
            install_compat_node(isolate).expect("install");
        },
        work.join("main.mjs"),
    )
    .unwrap_or_else(|e| panic!("{e:?}"));
}
