//! URI built-ins and Annex B escape/unescape on the bare VM.

use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

fn run(source: &str) -> String {
    let code = compile_script(source).expect("compiles");
    let mut vm = Interpreter::new();
    match vm.run_script(&code) {
        Ok(value) => vm
            .to_js_string(&value)
            .expect("printable")
            .to_string_lossy(),
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(abrupt) => format!("{abrupt:?}"),
    }
}

#[test]
fn encode_uri_component_escapes_utf8() {
    let source = "encodeURIComponent('a b&\\u00fc\\u20ac\\u{1F600}')";
    assert_eq!(run(source), "a%20b%26%C3%BC%E2%82%AC%F0%9F%98%80");
}

#[test]
fn encode_uri_keeps_reserved() {
    assert_eq!(run("encodeURI('a b;/?#')"), "a%20b;/?#");
}

#[test]
fn decode_preserves_reserved_only_for_decode_uri() {
    assert_eq!(run("decodeURI('%3B%2F%41')"), "%3B%2FA");
    assert_eq!(run("decodeURIComponent('%3B%41')"), ";A");
    assert_eq!(run("decodeURI('%E2%82%AC')"), "\u{20ac}");
}

#[test]
fn malformed_input_throws_uri_error() {
    for call in [
        "encodeURI('\\ud800')",
        "encodeURI('\\udc00a')",
        "decodeURI('%C0%80')",
        "decodeURI('%ED%A0%80')",
        "decodeURI('%F4%90%80%80')",
        "decodeURI('%E2%82')",
        "decodeURI('%')",
        "decodeURI('%zz')",
    ] {
        let source = format!("try {{ {call}; 'no' }} catch (e) {{ e instanceof URIError }}");
        assert_eq!(run(&source), "true", "{call}");
    }
}

#[test]
fn escape_and_unescape() {
    assert_eq!(run("escape('\\u00e4 b\\u20ac')"), "%E4%20b%u20AC");
    assert_eq!(run("unescape('%u20AC%E4%zz%u12')"), "\u{20ac}\u{e4}%zz%u12");
}

#[test]
fn function_shape() {
    assert_eq!(run("encodeURI.length"), "1");
    let source = "Object.getOwnPropertyDescriptor(globalThis, 'escape').enumerable";
    assert_eq!(run(source), "false");
}
