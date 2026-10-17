//! Default stream high-water marks (RT-08): 64 KiB for byte streams, 16 for object mode.

mod common;

use common::script::{assert_ok, check};

const PRELUDE: &str = r#"
const stream = globalThis[Symbol.for('velyr.node')].require('stream');
const { Readable, Writable, PassThrough, pipeline } = stream;
const { Buffer } = globalThis[Symbol.for('velyr.node')].require('buffer');
"#;

fn run(body: &str) -> String {
    check(&format!("{PRELUDE}{body}"))
}

#[test]
fn default_high_water_marks_follow_node() {
    assert_ok(run(
        r#"same(new Readable({ read() {} }).readableHighWaterMark, 65536);
        same(new Writable({ write() {} }).writableHighWaterMark, 65536);
        same(new Readable({ objectMode: true, read() {} }).readableHighWaterMark, 16);"#,
    ));
}

#[test]
fn one_mib_pipeline_copies_identical_bytes() {
    assert_ok(run(r#"const src = Buffer.alloc(1 << 20);
        for (let i = 0; i < src.length; i++) src[i] = (i * 31 + 7) & 0xff;
        const chunks = [];
        const sink = new Writable({ write(c, _e, cb) { chunks.push(c); cb(); } });
        await new Promise((resolve, reject) =>
          pipeline(Readable.from([src.subarray(0, 300000), src.subarray(300000)]), new PassThrough(), sink,
            (e) => (e ? reject(e) : resolve())));
        same(Buffer.concat(chunks).equals(src), true);"#));
}
