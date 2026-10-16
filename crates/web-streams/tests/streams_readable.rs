//! ReadableStream, pipeTo, tee, async iteration, byte streams and BYOB readers on V8.
mod common;
use common::expect;

#[test]
fn globals_are_installed_and_hidden() {
    expect(
        "record([typeof ReadableStream, typeof WritableStream, typeof TransformStream,
                 typeof CountQueuingStrategy, typeof ByteLengthQueuingStrategy,
                 typeof TextEncoderStream, typeof TextDecoderStream,
                 typeof CompressionStream, typeof DecompressionStream,
                 typeof ReadableStreamDefaultReader, typeof ReadableStreamBYOBReader,
                 typeof op_streams_flate, Object.keys(globalThis).includes('ReadableStream')].join());",
        &["function,function,function,function,function,function,function,function,function,function,function,undefined,false"],
    );
}

#[test]
fn default_reader_reads_enqueued_chunks_then_done() {
    expect(
        "const rs = new ReadableStream({ start(c) { c.enqueue('a'); c.enqueue('b'); c.close(); } });
         const r = rs.getReader();
         const out = [];
         for (;;) { const { value, done } = await r.read(); if (done) break; out.push(value); }
         record(out.join() + ' ' + rs.locked);
         r.releaseLock();
         record(String(rs.locked));",
        &["a,b true", "false"],
    );
}

#[test]
fn pull_respects_high_water_mark_and_desired_size() {
    expect(
        "let pulls = 0, ctl;
         const rs = new ReadableStream({ start(c) { ctl = c; }, pull() { pulls++; } },
                                       new CountQueuingStrategy({ highWaterMark: 2 }));
         await new Promise((r) => setTimeout(r, 5));
         record(pulls + ' ' + ctl.desiredSize);
         ctl.enqueue(1);
         record(String(ctl.desiredSize));
         const r = rs.getReader();
         await r.read();
         await new Promise((r) => setTimeout(r, 5));
         record(String(pulls > 1));",
        &["1 2", "1", "true"],
    );
}

#[test]
fn cancel_calls_source_with_reason_and_closes_reader() {
    expect(
        "let seen;
         const rs = new ReadableStream({ cancel(r) { seen = r; } });
         const r = rs.getReader();
         await r.cancel('why');
         record(seen + ' ' + (await r.read()).done);
         await r.closed;
         record('closed');",
        &["why true", "closed"],
    );
}

#[test]
fn source_error_rejects_reads_and_closed() {
    expect(
        "const rs = new ReadableStream({ start(c) { c.error(new Error('boom')); } });
         const r = rs.getReader();
         try { await r.read(); } catch (e) { record(e.message); }
         try { await r.closed; } catch (e) { record(e.message); }",
        &["boom", "boom"],
    );
}

#[test]
fn locking_and_invalid_arguments() {
    expect(
        "const rs = new ReadableStream();
         rs.getReader();
         try { rs.getReader(); } catch (e) { record(e.constructor.name); }
         try { new ReadableStream({ type: 'nope' }); } catch (e) { record(e.constructor.name); }
         try { new ReadableStream(undefined, { highWaterMark: -1 }); } catch (e) { record(e.constructor.name); }
         try { ReadableStream.prototype.getReader.call({}); } catch (e) { record(e.constructor.name); }",
        &["TypeError", "TypeError", "RangeError", "TypeError"],
    );
}

#[test]
fn pipe_through_text_decoder_stream_decodes_split_multibyte_text() {
    expect(
        "const bytes = new TextEncoder().encode('h\u{e9}llo \u{20ac} w\u{f6}rld \u{1f600}!');
         const rs = new ReadableStream({ start(c) {
           for (let i = 0; i < bytes.length; i += 2) c.enqueue(bytes.slice(i, i + 2));
           c.close();
         } });
         const reader = rs.pipeThrough(new TextDecoderStream()).getReader();
         let text = '';
         for (;;) { const { value, done } = await reader.read(); if (done) break; text += value; }
         record(text);",
        &["h\u{e9}llo \u{20ac} w\u{f6}rld \u{1f600}!"],
    );
}

#[test]
fn text_encoder_stream_joins_split_surrogate_pairs() {
    expect(
        "const rs = new ReadableStream({ start(c) { c.enqueue('a\\ud83d'); c.enqueue('\\ude00b'); c.close(); } });
         const reader = rs.pipeThrough(new TextEncoderStream()).getReader();
         const all = [];
         for (;;) { const { value, done } = await reader.read(); if (done) break; all.push(...value); }
         record(all.join());",
        &["97,240,159,152,128,98"],
    );
}

#[test]
fn abort_during_pipe_to_cancels_both_sides_with_reason() {
    expect(
        "let cancelReason, abortReason, started = false;
         const rs = new ReadableStream({ pull(c) { c.enqueue(1); }, cancel(r) { cancelReason = r; } });
         const ws = new WritableStream({ write() { return new Promise((r) => setTimeout(r, 2)); }, abort(r) { abortReason = r; } });
         const ac = new AbortController();
         const p = rs.pipeTo(ws, { signal: ac.signal });
         await new Promise((r) => setTimeout(r, 5));
         const reason = new Error('stop');
         ac.abort(reason);
         try { await p; } catch (e) { record('rejected ' + (e === reason)); }
         record((cancelReason === reason) + ' ' + (abortReason === reason));
         record(rs.locked + ' ' + ws.locked);",
        &["rejected true", "true true", "false false"],
    );
}

#[test]
fn pipe_to_already_aborted_signal_rejects_with_abort_error() {
    expect(
        "const rs = new ReadableStream({ cancel() { record('cancel'); } });
         const ws = new WritableStream({ abort() { record('abort'); } });
         try { await rs.pipeTo(ws, { signal: AbortSignal.abort() }); } catch (e) { record(e.name); }",
        &["abort", "cancel", "AbortError"],
    );
}

#[test]
fn pipe_to_moves_chunks_and_closes_destination() {
    expect(
        "const got = [];
         const rs = new ReadableStream({ start(c) { c.enqueue(1); c.enqueue(2); c.enqueue(3); c.close(); } });
         const ws = new WritableStream({ write(x) { got.push(x); }, close() { got.push('closed'); } });
         await rs.pipeTo(ws);
         record(got.join());",
        &["1,2,3,closed"],
    );
}

#[test]
fn pipe_to_propagates_source_error_and_respects_prevent_abort() {
    expect(
        "const src = () => new ReadableStream({ start(c) { c.error(new Error('src')); } });
         let aborted = 0;
         const dst = () => new WritableStream({ abort() { aborted++; } });
         try { await src().pipeTo(dst()); } catch (e) { record(e.message + ' ' + aborted); }
         try { await src().pipeTo(dst(), { preventAbort: true }); } catch (e) { record(e.message + ' ' + aborted); }",
        &["src 1", "src 1"],
    );
}

#[test]
fn pipe_to_propagates_destination_error_by_cancelling_source() {
    expect(
        "let cancelled;
         const rs = new ReadableStream({ pull(c) { c.enqueue(1); }, cancel(r) { cancelled = r.message; } });
         const ws = new WritableStream({ write() { throw new Error('sink'); } });
         try { await rs.pipeTo(ws); } catch (e) { record(e.message + ' ' + cancelled); }",
        &["sink sink"],
    );
}

#[test]
fn async_iteration_reads_all_and_releases_lock_on_exhaustion() {
    expect(
        "const rs = new ReadableStream({ start(c) { c.enqueue('x'); c.enqueue('y'); c.close(); } });
         const out = [];
         for await (const chunk of rs) out.push(chunk);
         record(out.join() + ' ' + rs.locked);",
        &["x,y false"],
    );
}

#[test]
fn async_iteration_break_cancels_and_releases_lock() {
    expect(
        "let cancelled = false;
         const rs = new ReadableStream({ pull(c) { c.enqueue(1); }, cancel() { cancelled = true; } });
         for await (const chunk of rs) { break; }
         record(cancelled + ' ' + rs.locked);",
        &["true false"],
    );
}

#[test]
fn async_iteration_break_with_prevent_cancel_keeps_stream_usable() {
    expect(
        "let cancelled = false, n = 0;
         const rs = new ReadableStream({ pull(c) { c.enqueue(++n); }, cancel() { cancelled = true; } });
         for await (const chunk of rs.values({ preventCancel: true })) { break; }
         record(cancelled + ' ' + rs.locked);
         const { value } = await rs.getReader().read();
         record(String(value > 0));",
        &["false false", "true"],
    );
}

#[test]
fn async_iterator_inherits_the_async_iterator_prototype() {
    expect(
        "const it = new ReadableStream()[Symbol.asyncIterator]();
         const proto = Object.getPrototypeOf(Object.getPrototypeOf(async function* () {}).prototype);
         record(String(proto.isPrototypeOf(it)) + ' ' + (ReadableStream.prototype.values === ReadableStream.prototype[Symbol.asyncIterator]));",
        &["true true"],
    );
}

#[test]
fn readable_stream_from_async_and_sync_iterables() {
    expect(
        "async function* gen() { yield 1; yield 2; }
         const out = [];
         for await (const v of ReadableStream.from(gen())) out.push(v);
         for await (const v of ReadableStream.from([3, Promise.resolve(4)])) out.push(v);
         record(out.join());
         let returned = false;
         const it = { [Symbol.asyncIterator]() { return { next: async () => ({ value: 1, done: false }), return: async () => { returned = true; return {}; } }; } };
         const r = ReadableStream.from(it).getReader();
         await r.read();
         await r.cancel('x');
         record(String(returned));
         try { ReadableStream.from(5); } catch (e) { record(e.constructor.name); }",
        &["1,2,3,4", "true", "TypeError"],
    );
}

#[test]
fn tee_default_streams_delivers_to_both_branches_and_cancels_source_once() {
    expect(
        "let cancelled;
         const rs = new ReadableStream({ start(c) { c.enqueue(1); c.enqueue(2); c.close(); } });
         const [a, b] = rs.tee();
         const drain = async (s) => { const out = []; for await (const v of s) out.push(v); return out.join(); };
         record(await drain(a) + '|' + await drain(b));
         const rs2 = new ReadableStream({ pull(c) { c.enqueue(1); }, cancel(r) { cancelled = r; } });
         const [c1, c2] = rs2.tee();
         const first = c1.cancel('one');
         record(String(cancelled));
         await c2.cancel('two');
         await first;
         record(cancelled.join());",
        &["1,2|1,2", "undefined", "one,two"],
    );
}

#[test]
fn byte_stream_default_reader_gets_uint8_arrays() {
    expect(
        "const rs = new ReadableStream({ type: 'bytes', start(c) { c.enqueue(new Uint8Array([1, 2, 3])); c.close(); } });
         const r = rs.getReader();
         const { value, done } = await r.read();
         record(value.constructor.name + ' ' + [...value].join() + ' ' + done);
         record(String((await r.read()).done));",
        &["Uint8Array 1,2,3 false", "true"],
    );
}

#[test]
fn byob_reader_reads_into_view_and_detaches_the_buffer() {
    expect(
        "const rs = new ReadableStream({ type: 'bytes', start(c) { c.enqueue(new Uint8Array([1, 2, 3, 4, 5])); c.close(); } });
         const r = rs.getReader({ mode: 'byob' });
         const buf = new ArrayBuffer(4);
         const first = await r.read(new Uint8Array(buf, 0, 4));
         record([...first.value].join() + ' ' + first.done + ' ' + buf.byteLength);
         const second = await r.read(new Uint8Array(4));
         record([...second.value].join() + ' ' + second.done);
         const third = await r.read(new Uint8Array(4));
         record(third.value.byteLength + ' ' + third.done);",
        &["1,2,3,4 false 0", "5 false", "0 true"],
    );
}

#[test]
fn byob_request_respond_fills_the_pending_view() {
    expect(
        "const rs = new ReadableStream({ type: 'bytes', pull(c) {
           const v = c.byobRequest.view;
           v[0] = 7; v[1] = 8;
           c.byobRequest.respond(2);
         } });
         const r = rs.getReader({ mode: 'byob' });
         const { value } = await r.read(new Uint8Array(8));
         record([...value].join() + ' ' + value.byteLength);
         const u16 = await r.read(new Uint16Array(2));
         record(u16.value.constructor.name + ' ' + u16.value.length);",
        &["7,8 2", "Uint16Array 1"],
    );
}

#[test]
fn byob_read_honors_min_and_rejects_bad_views() {
    expect(
        "const rs = new ReadableStream({ type: 'bytes' });
         const r = rs.getReader({ mode: 'byob' });
         try { await r.read(new Uint8Array(0)); } catch (e) { record(e.constructor.name); }
         try { await r.read(new Uint8Array(2), { min: 3 }); } catch (e) { record(e.constructor.name); }
         try { await r.read({}); } catch (e) { record(e.constructor.name); }
         try { new ReadableStream({}).getReader({ mode: 'byob' }); } catch (e) { record(e.constructor.name); }",
        &["TypeError", "RangeError", "TypeError", "TypeError"],
    );
}

#[test]
fn byte_stream_auto_allocate_serves_default_reads_through_byob_request() {
    expect(
        "const rs = new ReadableStream({ type: 'bytes', autoAllocateChunkSize: 4, pull(c) {
           c.byobRequest.view[0] = 9;
           c.byobRequest.respond(1);
         } });
         const { value } = await rs.getReader().read();
         record(value.constructor.name + ' ' + value.length + ' ' + value[0] + ' ' + value.buffer.byteLength);",
        &["Uint8Array 1 9 4"],
    );
}

#[test]
fn releasing_a_byob_reader_with_pending_read_rejects_it() {
    expect(
        "const rs = new ReadableStream({ type: 'bytes' });
         const r = rs.getReader({ mode: 'byob' });
         const p = r.read(new Uint8Array(2));
         r.releaseLock();
         try { await p; } catch (e) { record(e.constructor.name); }
         const r2 = rs.getReader();
         record(String(rs.locked));",
        &["TypeError", "true"],
    );
}

#[test]
fn byte_stream_tee_branches_receive_the_same_bytes() {
    expect(
        "const rs = new ReadableStream({ type: 'bytes', start(c) { c.enqueue(new Uint8Array([1, 2])); c.enqueue(new Uint8Array([3])); c.close(); } });
         const [a, b] = rs.tee();
         const drain = async (s) => { const out = []; for await (const v of s) out.push(...v); return out.join(); };
         record(await drain(a) + '|' + await drain(b));
         const rs2 = new ReadableStream({ type: 'bytes', start(c) { c.enqueue(new Uint8Array([5, 6, 7])); c.close(); } });
         const [c, d] = rs2.tee();
         const rr = c.getReader({ mode: 'byob' });
         const { value } = await rr.read(new Uint8Array(3));
         record([...value].join() + ' ' + [...(await d.getReader().read()).value].join());",
        &["1,2,3|1,2,3", "5,6,7 5,6,7"],
    );
}
