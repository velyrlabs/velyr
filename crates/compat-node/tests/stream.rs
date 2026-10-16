//! `node:stream`, `stream/promises`, `stream/web` and the Web Streams interop, through
//! `common::script` (real event loop and tick hook: streams lean on `process.nextTick`).

mod common;

use common::script::{assert_ok, check};

const PRELUDE: &str = r#"
const node = (n) => globalThis[Symbol.for('velyr.node')].require(n);
const stream = node('stream');
const { Readable, Writable, Duplex, Transform, PassThrough, pipeline, finished } = stream;
const collect = async (readable) => { const out = []; for await (const c of readable) out.push(c); return out; };
"#;

fn run(body: &str) -> String {
    check(&format!("{PRELUDE}{body}"))
}

#[test]
fn pipeline_with_transform_propagates_the_error_and_destroys_the_source() {
    assert_ok(run(r#"const source = Readable.from(['a', 'b', 'c']);
        const boom = new Error('transform failed');
        const failing = new Transform({ transform(chunk, enc, cb) { cb(boom); } });
        const sink = new Writable({ write(chunk, enc, cb) { cb(); } });
        const err = await new Promise((resolve) => pipeline(source, failing, sink, resolve));
        eq(err, boom);
        eq(source.destroyed, true, 'source destroyed');
        eq(sink.destroyed, true, 'sink destroyed');

        // stream/promises rejects with the same error.
        const { pipeline: pipelineP, finished: finishedP } = node('stream/promises');
        const other = new Error('again');
        let caught;
        try {
          await pipelineP(Readable.from(['x']), new Transform({ transform(c, e, cb) { cb(other); } }), new PassThrough());
        } catch (e) { caught = e; }
        eq(caught, other);
        eq(stream.promises, node('stream/promises'));

        // Happy path: chunks flow through, `finished` resolves.
        const chunks = [];
        const upper = new Transform({ transform(c, e, cb) { cb(null, String(c).toUpperCase()); } });
        const out = new Writable({ write(c, e, cb) { chunks.push(String(c)); cb(); } });
        await pipelineP(Readable.from(['a', 'b']), upper, out);
        same(chunks, ['A', 'B']);
        await finishedP(out);"#));
}

#[test]
fn backpressure_respects_the_high_water_mark() {
    assert_ok(run(r#"const writes = [];
        const w = new Writable({ highWaterMark: 2, objectMode: true, write(c, e, cb) { writes.push(c); setTimeout(cb, 1); } });
        eq(w.write(1), true);
        eq(w.write(2), false, 'at the mark');
        eq(w.writableNeedDrain, true);
        await new Promise((r) => w.once('drain', r));
        eq(w.writableNeedDrain, false);
        eq(w.write(3), true);
        w.end();
        await new Promise((r) => finished(w, r));
        same(writes, [1, 2, 3]);

        let reads = 0;
        const r = new Readable({ highWaterMark: 3, objectMode: true, read() { reads++; this.push(reads); } });
        r.read(0);
        await new Promise((res) => setTimeout(res, 5));
        eq(reads, 3, 'stops pulling at the mark');
        eq(r.readableLength, 3);
        eq(r.read(), 1);
        await new Promise((res) => setTimeout(res, 5));
        eq(reads, 4, 'pulls again after a read');

        const sizes = new Writable({ highWaterMark: 4, write(c, e, cb) { setTimeout(cb, 5); } });
        eq(sizes.write('abc'), true);
        eq(sizes.write('de'), false, 'bytes count, not chunks');"#));
}

#[test]
fn to_web_and_from_web_round_trip() {
    assert_ok(run(
        r#"const web = Readable.toWeb(Readable.from(['a', 'b', 'c']));
        eq(web instanceof ReadableStream, true);
        const back = Readable.fromWeb(web, { objectMode: true });
        same(await collect(back), ['a', 'b', 'c']);

        const objects = Readable.fromWeb(Readable.toWeb(Readable.from([{ n: 1 }, { n: 2 }])), { objectMode: true });
        same(await collect(objects), [{ n: 1 }, { n: 2 }]);

        const seen = [];
        const nodeSide = new Writable({ objectMode: true, write(c, e, cb) { seen.push(c); cb(); } });
        const webWriter = Writable.toWeb(nodeSide);
        eq(webWriter instanceof WritableStream, true);
        const writer = webWriter.getWriter();
        await writer.write('x');
        await writer.write('y');
        await writer.close();
        same(seen, ['x', 'y']);

        const sunk = [];
        const fromWeb = Writable.fromWeb(new WritableStream({ write(c) { sunk.push(c); } }), { objectMode: true });
        await new Promise((res) => { fromWeb.write('p'); fromWeb.write('q'); fromWeb.end(res); });
        same(sunk, ['p', 'q']);

        // Errors cross the bridge.
        const failing = new ReadableStream({ start(c) { c.error(new Error('web boom')); } });
        let message;
        try { await collect(Readable.fromWeb(failing)); } catch (e) { message = e.message; }
        eq(message, 'web boom');"#,
    ));
}

#[test]
fn stream_classes_and_helpers() {
    assert_ok(run(r#"const web = node('stream/web');
        for (const name of ['ReadableStream', 'WritableStream', 'TransformStream', 'TextDecoderStream', 'CountQueuingStrategy']) eq(web[name], globalThis[name], name);
        eq(new PassThrough() instanceof Transform, true);
        eq(new Transform() instanceof Duplex, true);
        eq(node('events').prototype.isPrototypeOf(Readable.prototype), true);
        same(await collect(Readable.from(['a', 'b']).map((x) => x + '!')), ['a!', 'b!']);

        const ac = new AbortController();
        const aborted = stream.addAbortSignal(ac.signal, new PassThrough());
        const done = new Promise((r) => aborted.once('error', r));
        ac.abort();
        eq((await done).name, 'AbortError');

        const composed = stream.compose(new Transform({ transform(c, e, cb) { cb(null, c + '1'); } }), new Transform({ transform(c, e, cb) { cb(null, c + '2'); } }));
        composed.end('z');
        same((await collect(composed)).map(String), ['z12']);

        const err = await new Promise((r) => { const d = new Duplex({ read() {}, write(c, e, cb) { cb(); } }); finished(d, r); d.destroy(new Error('d')); });
        eq(err.message, 'd');
        let code;
        try { new Writable().write(5); } catch (e) { code = e.code; }
        eq(code, 'ERR_INVALID_ARG_TYPE');"#));
}

#[test]
fn process_stdio_are_real_streams() {
    assert_ok(run(r#"eq(process.stdout instanceof Writable, true);
        eq(process.stderr instanceof Writable, true);
        eq(process.stdin instanceof Readable, true);
        eq(process.stdout, process.stdout, 'cached');
        eq(process.stdout.fd, 1);
        eq(process.stderr.fd, 2);
        eq(process.stdin.fd, 0);
        eq(process.stdout.isTTY, false);
        eq(process.stdout.write(''), true);
        await new Promise((r) => process.stdout.write('', r));
        const sink = new Writable({ write(c, e, cb) { cb(); } });
        await new Promise((r) => pipeline(Readable.from([]), sink, r));
        // stdin has no data source: it ends right away.
        same(await collect(process.stdin), []);"#));
}
