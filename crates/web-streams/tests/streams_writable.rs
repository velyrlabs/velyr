//! WritableStream, TransformStream, strategies, compression and the native hooks on V8.
mod common;
use common::expect;

#[test]
fn writable_stream_orders_writes_and_closes() {
    expect(
        "const log = [];
         const ws = new WritableStream({
           start() { log.push('start'); },
           async write(chunk) { await null; log.push('w' + chunk); },
           close() { log.push('close'); },
         });
         const w = ws.getWriter();
         w.write(1); w.write(2);
         await w.close();
         await w.closed;
         record(log.join());
         record(String(w.desiredSize));",
        &["start,w1,w2,close", "0"],
    );
}

#[test]
fn writable_stream_backpressure_and_ready() {
    expect(
        "let release;
         const ws = new WritableStream({ write() { return new Promise((r) => { release = r; }); } },
                                       { highWaterMark: 1, size: () => 1 });
         const w = ws.getWriter();
         await w.ready;
         record(String(w.desiredSize));
         const p = w.write('a');
         record(String(w.desiredSize));
         let readyDone = false;
         w.ready.then(() => { readyDone = true; });
         await new Promise((r) => setTimeout(r, 5));
         record(String(readyDone));
         release(); await p; await w.ready;
         record(String(w.desiredSize));",
        &["1", "0", "false", "1"],
    );
}

#[test]
fn writable_stream_abort_and_error_paths() {
    expect(
        "let seen;
         const ws = new WritableStream({ abort(r) { seen = r; } });
         const w = ws.getWriter();
         await w.abort('bye');
         record(String(seen));
         try { await w.write(1); } catch (e) { record(String(e)); }
         const bad = new WritableStream({ write() { throw new Error('nope'); } });
         const bw = bad.getWriter();
         try { await bw.write(1); } catch (e) { record(e.message); }
         try { await bw.closed; } catch (e) { record(e.message); }
         try { bw.releaseLock(); record(String(bad.locked)); } catch (e) { record('release ' + e.constructor.name); }
         try { new WritableStream({ type: 'x' }); } catch (e) { record(e.constructor.name); }",
        &["bye", "bye", "nope", "nope", "false", "RangeError"],
    );
}

#[test]
fn writable_controller_signal_aborts_when_stream_aborts() {
    expect(
        "let signal;
         const ws = new WritableStream({ start(c) { signal = c.signal; } });
         await ws.abort('r');
         record(signal.aborted + ' ' + signal.reason);",
        &["true r"],
    );
}

#[test]
fn transform_stream_maps_chunks_and_flushes() {
    expect(
        "const ts = new TransformStream({
           transform(chunk, c) { c.enqueue(chunk * 2); },
           flush(c) { c.enqueue('end'); },
         });
         const w = ts.writable.getWriter();
         w.write(1); w.write(2); w.close();
         const out = [];
         for await (const v of ts.readable) out.push(v);
         record(out.join());",
        &["2,4,end"],
    );
}

#[test]
fn transform_stream_defaults_to_identity_and_applies_backpressure() {
    expect(
        "const ts = new TransformStream();
         const w = ts.writable.getWriter();
         const r = ts.readable.getReader();
         w.write('a');
         record((await r.read()).value);
         record(String(ts.readable.constructor === ReadableStream));
         const slow = new TransformStream({}, { highWaterMark: 1 }, { highWaterMark: 0 });
         const sw = slow.writable.getWriter();
         const p = sw.write('x');
         let done = false;
         p.then(() => { done = true; });
         await new Promise((r) => setTimeout(r, 5));
         record(String(done));
         await slow.readable.getReader().read();
         await p;
         record(String(done));",
        &["a", "true", "false", "true"],
    );
}

#[test]
fn transform_stream_terminate_and_error() {
    expect(
        "const ts = new TransformStream({ transform(chunk, c) { c.terminate(); } });
         const w = ts.writable.getWriter();
         const r = ts.readable.getReader();
         w.write(1).catch(() => {});
         record(String((await r.read()).done));
         try { await w.closed; } catch (e) { record(e.constructor.name); }
         const bad = new TransformStream({ transform() { throw new Error('t'); } });
         const bw = bad.writable.getWriter();
         bw.write(1).catch(() => {});
         try { await bad.readable.getReader().read(); } catch (e) { record(e.message); }",
        &["true", "TypeError", "t"],
    );
}

#[test]
fn transform_stream_rejects_unsupported_types() {
    expect(
        "try { new TransformStream({ readableType: 'bytes' }); } catch (e) { record(e.constructor.name); }
         try { new TransformStream({ writableType: 'bytes' }); } catch (e) { record(e.constructor.name); }",
        &["RangeError", "RangeError"],
    );
}

#[test]
fn queuing_strategies() {
    expect(
        "const c = new CountQueuingStrategy({ highWaterMark: 3 });
         const b = new ByteLengthQueuingStrategy({ highWaterMark: 8 });
         record([c.highWaterMark, c.size('anything'), b.highWaterMark, b.size(new Uint8Array(5))].join());
         try { new CountQueuingStrategy({}); } catch (e) { record(e.constructor.name); }
         try { new CountQueuingStrategy(); } catch (e) { record(e.constructor.name); }
         record(String(CountQueuingStrategy.prototype.hasOwnProperty('size')));
         const ws = new WritableStream({}, b);
         record(String(ws.getWriter().desiredSize));",
        &["3,1,8,5", "TypeError", "TypeError", "true", "8"],
    );
}

#[test]
fn gzip_round_trip_restores_the_bytes() {
    expect(
        "const input = new Uint8Array(100000);
         for (let i = 0; i < input.length; i++) input[i] = (i * 31 + (i >> 8)) & 255;
         for (const format of ['gzip', 'deflate', 'deflate-raw']) {
           const packed = new ReadableStream({ start(c) {
             for (let i = 0; i < input.length; i += 7000) c.enqueue(input.subarray(i, i + 7000));
             c.close();
           } }).pipeThrough(new CompressionStream(format)).pipeThrough(new DecompressionStream(format));
           const parts = [];
           for await (const part of packed) parts.push(part);
           const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
           let at = 0;
           for (const p of parts) { out.set(p, at); at += p.length; }
           record(format + ' ' + (out.length === input.length && out.every((v, i) => v === input[i])));
         }",
        &["gzip true", "deflate true", "deflate-raw true"],
    );
}

#[test]
fn compression_emits_a_real_gzip_header_and_rejects_bad_input() {
    expect(
        "const w = new CompressionStream('gzip');
         const writer = w.writable.getWriter();
         writer.write(new TextEncoder().encode('hello hello hello'));
         writer.close();
         const head = (await w.readable.getReader().read()).value;
         record(head[0] + ',' + head[1]);
         const d = new DecompressionStream('gzip');
         const dw = d.writable.getWriter();
         dw.write(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])).catch(() => {});
         try { await d.readable.getReader().read(); } catch (e) { record(e.constructor.name); }
         try { new CompressionStream('br'); } catch (e) { record(e.constructor.name); }
         const t = new DecompressionStream('deflate');
         const tw = t.writable.getWriter();
         tw.write(new Uint8Array([0x78, 0x9c, 1])).catch(() => {});
         tw.close().catch(() => {});
         try { await t.readable.getReader().read(); record('no error'); } catch (e) { record(e.constructor.name); }
         for (const [format, bytes] of [
           ['gzip', [31, 139, 8, 0, 0, 0, 0, 0, 0, 3]],
           ['deflate-raw', [75, 76]],
           ['deflate', [120, 156, 75, 76]],
         ]) {
           const d2 = new DecompressionStream(format);
           const w2 = d2.writable.getWriter();
           w2.write(new Uint8Array(bytes)).catch(() => {});
           w2.close().catch(() => {});
           try { for await (const _ of d2.readable); record(format + ' no error'); } catch (e) { record(format + ' ' + e.constructor.name); }
         }
         const g = new CompressionStream('deflate');
         const gw = g.writable.getWriter();
         gw.write(new Uint8Array([1, 2, 3])); gw.close();
         const parts = [];
         for await (const part of g.readable) parts.push(...part);
         const trailing = new Uint8Array([...parts, 0, 0, 0]);
         const d3 = new DecompressionStream('deflate');
         const w3 = d3.writable.getWriter();
         w3.write(trailing).catch(() => {});
         w3.close().catch(() => {});
         try { for await (const _ of d3.readable); record('trailing no error'); } catch (e) { record('trailing ' + e.constructor.name); }",
        &["31,139", "TypeError", "TypeError", "TypeError", "gzip TypeError", "deflate-raw TypeError", "deflate TypeError", "trailing TypeError"],
    );
}

#[test]
fn compression_rejects_non_buffer_chunks() {
    expect(
        "const c = new CompressionStream('gzip');
         const w = c.writable.getWriter();
         c.readable.getReader().read().catch(() => {});
         try { await w.write('text'); } catch (e) { record(e.constructor.name); }",
        &["TypeError"],
    );
}

#[test]
fn native_readable_pulls_from_the_op_until_eof() {
    expect(
        "const internal = globalThis[Symbol.for('velyr.internal.streams')];
         record(String(Object.keys(globalThis).includes('createNativeReadable')) + ' ' + typeof internal.createNativeReadable);
         const chunks = [new Uint8Array([1, 2]), new Uint8Array([3]).buffer, null];
         const seen = [];
         const rs = internal.createNativeReadable(42, async (id) => { seen.push(id); return chunks.shift(); });
         const out = [];
         for await (const v of rs) out.push(...v);
         record(out.join() + ' ' + seen.join());
         let closed = 0;
         const rs2 = internal.createNativeReadable(7, async () => new Uint8Array(1), async (id) => { closed += id; });
         await rs2.cancel();
         record(String(closed));",
        &["false function", "1,2,3 42,42,42", "7"],
    );
}

#[test]
fn native_writable_forwards_chunks_to_the_op() {
    expect(
        "const internal = globalThis[Symbol.for('velyr.internal.streams')];
         const log = [];
         const ws = internal.createNativeWritable(9,
           async (id, chunk) => { log.push(id + ':' + [...chunk].join('.')); },
           async (id) => { log.push('close' + id); });
         const w = ws.getWriter();
         await w.write(new Uint8Array([1, 2]));
         await w.close();
         record(log.join());",
        &["9:1.2,close9"],
    );
}

#[test]
fn is_disturbed_tells_read_and_cancelled_streams_apart_and_stays_internal() {
    expect(
        "const internal = globalThis[Symbol.for('velyr.internal.streams')];
         const make = () => new ReadableStream({ pull(c) { c.enqueue(1); } });
         const fresh = make();
         const read = make();
         const reader = read.getReader();
         await reader.read();
         reader.releaseLock();
         const cancelled = make();
         await cancelled.cancel();
         record([fresh, read, cancelled, {}, null].map((s) => internal.isDisturbed(s)).join());
         record(String(Object.keys(globalThis).concat(Object.getOwnPropertyNames(ReadableStream.prototype))
           .some((name) => /disturbed/i.test(name))));",
        &["false,true,true,false,false", "false"],
    );
}
