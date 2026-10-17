//! `RegExp.prototype.compile` (ECMA-262 Annex B.2.4.1).

use velyr_engine_native::bytecode_compiler::compile_script;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter};

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

#[test]
fn compile_reinitializes_and_resets_last_index() {
    let source = "const r = /a/g; r.lastIndex = 3;
        [r.compile('b', 'i') === r, r.source, r.flags, r.lastIndex].join()";
    assert_eq!(run(source), "true,b,i,0");
}

#[test]
fn compile_copies_from_a_regexp_pattern() {
    assert_eq!(run("/a/.compile(/b/g).flags"), "g");
}

#[test]
fn compile_rejects_flags_with_a_regexp_pattern() {
    let source = "try { /a/.compile(/b/, 'g'); 'no' } catch (e) { e instanceof TypeError }";
    assert_eq!(run(source), "true");
}

#[test]
fn compile_requires_a_regexp_receiver() {
    let source =
        "try { RegExp.prototype.compile.call({}); 'no' } catch (e) { e instanceof TypeError }";
    assert_eq!(run(source), "true");
}

#[test]
fn compile_has_length_two() {
    assert_eq!(run("RegExp.prototype.compile.length"), "2");
}
