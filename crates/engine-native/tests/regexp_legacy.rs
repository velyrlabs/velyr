//! RegExp legacy static properties (proposal-regexp-legacy-features).

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
fn statics_follow_the_last_successful_exec() {
    let source = "/(a)(b)/.exec('xaby');
        [RegExp.$1, RegExp.$2, RegExp.$3, RegExp.lastMatch, RegExp.leftContext,
         RegExp.rightContext, RegExp.lastParen, RegExp.input].join('|')";
    assert_eq!(run(source), "a|b||ab|x|y|b|xaby");
}

#[test]
fn short_aliases_match_the_long_names() {
    let source = "/(a)(b)/.exec('xaby');
        [RegExp['$&'], RegExp['$`'], RegExp[\"$'\"], RegExp['$+'], RegExp.$_].join('|')";
    assert_eq!(run(source), "ab|x|y|b|xaby");
}

#[test]
fn failed_match_keeps_previous_values() {
    assert_eq!(run("/(a)/.exec('a'); /z/.exec('a'); RegExp.$1"), "a");
}

#[test]
fn initial_values_are_empty_strings() {
    assert_eq!(
        run("[RegExp.$1, RegExp.lastMatch, RegExp.input].join('|')"),
        "||"
    );
}

#[test]
fn descriptors_are_accessors() {
    let source = "const d = Object.getOwnPropertyDescriptor(RegExp, '$1');
        [typeof d.get, d.set, d.enumerable, d.configurable].join()";
    assert_eq!(run(source), "function,,false,true");
}

#[test]
fn input_has_a_setter() {
    let source = "RegExp.input = 42; [RegExp.input, RegExp.$_,
        typeof Object.getOwnPropertyDescriptor(RegExp, '$_').set].join()";
    assert_eq!(run(source), "42,42,function");
}

#[test]
fn getters_reject_other_receivers() {
    let source = "class R extends RegExp {}
        const get = Object.getOwnPropertyDescriptor(RegExp, '$1').get;
        try { get.call(R); 'no' } catch (e) { e instanceof TypeError }";
    assert_eq!(run(source), "true");
}

#[test]
fn subclass_exec_invalidates_the_statics() {
    let source = "class R extends RegExp {}
        /(a)/.exec('a'); new R('a').exec('a');
        try { RegExp.$1; 'no' } catch (e) { e instanceof TypeError }";
    assert_eq!(run(source), "true");
}

#[test]
fn a_later_legacy_exec_revalidates_the_statics() {
    let source = "class R extends RegExp {}
        new R('a').exec('a'); /(b)/.exec('b'); RegExp.$1";
    assert_eq!(run(source), "b");
}

#[test]
fn compile_throws_for_subclass_instances() {
    let source = "class R extends RegExp {}
        try { new R('a').compile('b'); 'no' } catch (e) { e instanceof TypeError }";
    assert_eq!(run(source), "true");
}

#[test]
fn compile_keeps_working_for_plain_regexps() {
    assert_eq!(run("/a/.compile('b').source"), "b");
}
