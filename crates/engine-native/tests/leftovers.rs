//! Small leftovers: huge `maxByteLength` limits and `toISOString` at the time-value extremes.

use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

/// `String(completion)`, or `throw <message>` for an uncaught exception.
fn run(source: &str) -> String {
    let code = match compile_script(source) {
        Ok(code) => code,
        Err(error) => return format!("compile error: {}", error.message),
    };
    let mut vm = Interpreter::new();
    match vm.run_script(&code) {
        Ok(value) => match vm.to_js_string(&value) {
            Ok(text) => text.to_string_lossy(),
            Err(_) => "<unprintable completion>".into(),
        },
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(abrupt) => format!("{abrupt:?}"),
    }
}

fn throws_range(expression: &str) -> String {
    run(&format!(
        "try {{ {expression}; 'no error' }} catch (e) {{ e instanceof RangeError ? 'RangeError' : String(e) }}"
    ))
}

#[test]
fn array_buffer_rejects_huge_max_byte_length() {
    for max in ["7 * 1125899906842624", "9007199254740992 - 1"] {
        let expression = format!("new ArrayBuffer(0, {{ maxByteLength: {max} }})");
        assert_eq!(throws_range(&expression), "RangeError", "{max}");
    }
}

#[test]
fn shared_array_buffer_rejects_huge_max_byte_length() {
    for max in ["7 * 1125899906842624", "9007199254740992 - 1"] {
        let expression = format!("new SharedArrayBuffer(0, {{ maxByteLength: {max} }})");
        assert_eq!(throws_range(&expression), "RangeError", "{max}");
    }
}

#[test]
fn modest_max_byte_length_still_works() {
    let source = "new ArrayBuffer(0, { maxByteLength: 1024 }).maxByteLength";
    assert_eq!(run(source), "1024");
}

#[test]
fn to_iso_string_handles_the_extreme_time_values() {
    let source = "[new Date(-8.64e15).toISOString(), new Date(8.64e15).toISOString()].join()";
    assert_eq!(
        run(source),
        "-271821-04-20T00:00:00.000Z,+275760-09-13T00:00:00.000Z"
    );
}

#[test]
fn to_iso_string_near_the_minimum_in_local_construction() {
    let source = "new Date(1970, 0, -99999999, 0, 60, 0, 0).toISOString().slice(-1)";
    assert_eq!(run(source), "Z");
}
