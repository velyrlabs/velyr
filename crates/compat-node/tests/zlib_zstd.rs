//! `node:zlib` zstd decompression (ruzstd) and `worker_threads.markAsUncloneable`, through
//! `common::script` on V8. The frames below come from Node's own `zlib.zstdCompressSync`.
mod common;

use common::script::{CHECKS, assert_ok, run};

const PRELUDE: &str = r#"
const req = (name) => globalThis[Symbol.for('velyr.node')].require(name);
const { Buffer } = req('buffer');
const hex = (text) => Buffer.from(text, 'hex');
const HELLO = hex('28b52ffd20168500005068656c6c6f207a7374640100e14a11');
const TWO = hex('28b52ffd200631000066697273742028b52ffd20063100007365636f6e64');
const SKIP = hex('502a4d1803000000aabbcc28b52ffd200a510000616674657220736b6970');
const CHECKSUM = hex('28b52ffd240c610000636865636b65642064617461684588c4');
// 4 MiB of zeros in 147 bytes: a frame header, then 30 RLE blocks of 128 KiB and a last one.
const BOMB = hex('28b52ffd8058000040005400001000000100fbff39c002' + '02001000'.repeat(30) + '03001000');
const fails = (fn) => { try { fn(); } catch (e) { return e; } throw new Error('did not throw: ' + String(fn)); };
"#;

fn script(body: &str) {
    assert_ok(run(&format!("{CHECKS}{PRELUDE}{body}")));
}

#[test]
fn zstd_decompress_sync_reads_frames_skippable_frames_and_checksums() {
    script(
        r#"
        const zlib = req('zlib');
        eq(zlib.zstdDecompressSync(HELLO).toString(), 'hello hello hello zstd');
        // Node decodes the first frame only: a second frame, a leading skippable frame's successor
        // and trailing junk are all ignored.
        eq(zlib.zstdDecompressSync(TWO).toString(), 'first ');
        eq(zlib.zstdDecompressSync(SKIP).toString(), '');
        eq(zlib.zstdDecompressSync(Buffer.concat([HELLO, Buffer.from('junk')])).toString(), 'hello hello hello zstd');
        eq(zlib.zstdDecompressSync(CHECKSUM).toString(), 'checked data');
        eq(zlib.zstdDecompressSync(BOMB).length, 4 << 20);
        eq(zlib.constants.ZSTD_d_windowLogMax, 100);
        eq(zlib.constants.ZSTD_e_end, 2);
        eq(zlib.constants.ZSTD_c_compressionLevel, 100);
    "#,
    );
}

#[test]
fn zstd_decompress_streams_and_callbacks_work_chunk_by_chunk() {
    script(
        r#"
        const zlib = req('zlib');
        const stream = zlib.createZstdDecompress();
        const parts = [];
        stream.on('data', (chunk) => parts.push(chunk));
        for (let at = 0; at < TWO.length; at += 3) stream.write(TWO.subarray(at, at + 3));
        stream.end();
        await new Promise((resolve, reject) => { stream.on('end', resolve); stream.on('error', reject); });
        eq(Buffer.concat(parts).toString(), 'first ');
        const viaCallback = await new Promise((resolve, reject) => zlib.zstdDecompress(HELLO, (e, out) => (e ? reject(e) : resolve(out))));
        eq(viaCallback.toString(), 'hello hello hello zstd');
    "#,
    );
}

#[test]
fn zstd_rejects_damaged_truncated_and_bomb_input() {
    script(
        r#"
        const zlib = req('zlib');
        eq(typeof fails(() => zlib.zstdDecompressSync(Buffer.from('definitely not zstd data'))).code, 'string');
        eq(typeof fails(() => zlib.zstdDecompressSync(HELLO.subarray(0, HELLO.length - 6))).code, 'string');
        // Cut exactly at a block boundary: still an incomplete frame, not a short answer.
        eq(typeof fails(() => zlib.zstdDecompressSync(BOMB.subarray(0, BOMB.length - 4))).code, 'string');
        eq(typeof fails(() => zlib.zstdDecompressSync(Buffer.alloc(0))).code, 'string');
        const corrupt = Buffer.from(CHECKSUM);
        corrupt[corrupt.length - 1] ^= 1;
        eq(typeof fails(() => zlib.zstdDecompressSync(corrupt)).code, 'string');
        // The zip-bomb guard: the output ceiling applies to zstd like to every other decoder.
        eq(fails(() => zlib.zstdDecompressSync(BOMB, { maxOutputLength: 65536 })).code, 'ERR_BUFFER_TOO_LARGE');
        const streamed = zlib.createZstdDecompress({ maxOutputLength: 65536 });
        const error = await new Promise((resolve) => { streamed.on('error', resolve); streamed.on('data', () => {}); streamed.end(BOMB); });
        eq(error.code, 'ERR_BUFFER_TOO_LARGE');
        const viaCallback = await new Promise((resolve) => zlib.zstdDecompress(BOMB, { maxOutputLength: 65536 }, (e) => resolve(e)));
        eq(viaCallback.code, 'ERR_BUFFER_TOO_LARGE');
    "#,
    );
}

#[test]
fn zstd_compression_is_unsupported_and_loud() {
    script(
        r#"
        const zlib = req('zlib');
        for (const call of [() => zlib.createZstdCompress(), () => zlib.zstdCompressSync('x'), () => new zlib.ZstdCompress()]) {
          eq(fails(call).code, 'ERR_VELYR_UNSUPPORTED');
        }
        eq(typeof zlib.ZstdDecompress, 'function');
    "#,
    );
}

#[test]
fn buffer_encoding_slice_methods_decode_a_range() {
    script(
        r#"
        const b = Buffer.from('héllo');
        eq(b.utf8Slice(0, 3), 'hé');
        eq(b.utf8Slice(), 'héllo');
        eq(Buffer.from('abc').hexSlice(1), '6263');
        eq(Buffer.from('abc').latin1Slice(0, 2), 'ab');
        eq(Buffer.from('abc').base64Slice(), 'YWJj');
    "#,
    );
}

#[test]
fn mark_as_uncloneable_is_exported_and_leaves_the_object_alone() {
    // Node makes structured clone refuse a marked object; the engine's serializer has no hook for
    // that yet, so here it only accepts the call (what undici's webidl needs) and changes nothing.
    script(
        r#"
        const { markAsUncloneable } = req('worker_threads');
        eq(typeof markAsUncloneable, 'function');
        const marked = { kept: 1 };
        eq(markAsUncloneable(marked), undefined);
        eq(Object.keys(marked).join(), 'kept');
        eq(Object.getOwnPropertySymbols(marked).length, 0);
        for (const ignored of [1, null, undefined, 'text']) eq(markAsUncloneable(ignored), undefined);
    "#,
    );
}
