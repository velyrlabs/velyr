//! `Buffer`, `node:buffer` and `node:string_decoder`, as JS fixtures with asserts in a V8 isolate.

mod common;

use velyr_engine_api::{Isolate, Source, Value};

macro_rules! fixture {
    ($test:ident, $file:literal, $global:literal) => {
        #[test]
        fn $test() {
            common::run_fixture($file, include_str!(concat!("fixtures/", $file)), $global);
        }
    };
}

fixture!(buffer_core, "buffer.js", true);
fixture!(buffer_encodings, "buffer_encodings.js", false);
fixture!(buffer_read_write, "buffer_rw.js", false);
fixture!(string_decoder, "string_decoder.js", false);

fn eval_string(iso: &mut impl Isolate, text: &str) -> String {
    let source = Source {
        name: "probe.js".into(),
        text: text.into(),
    };
    match iso.eval_script(&source) {
        Ok(Value::String(s)) => s.to_string_lossy(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn buffer_global_is_opt_in() {
    let mut iso = common::new_isolate();
    assert_eq!(eval_string(&mut iso, "typeof Buffer"), "undefined");
    velyr_compat_node::install_buffer_global(&mut iso).unwrap();
    let probe = "[typeof Buffer, Buffer === globalThis[Symbol.for('velyr.node')].require('buffer').Buffer, Object.keys(globalThis).includes('Buffer')].join()";
    assert_eq!(eval_string(&mut iso, probe), "function,true,false");
}
