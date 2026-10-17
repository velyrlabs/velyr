//! L16: the `CallDirectEval` instruction and configurable globals of indirect eval.

use std::rc::Rc;

use velyr_engine_native::bytecode_compiler::{compile_indirect_eval, compile_script};
use velyr_engine_vm::Value;
use velyr_engine_vm::bytecode::{ConstIndex, Constant, EvalCode, FunctionCode, Instruction};
use velyr_engine_vm::interpreter::{Abrupt, CompileGoal, Interpreter};

/// A VM with the dynamic-code hook, like `NativeIsolate`.
fn new_vm() -> Interpreter {
    let mut vm = Interpreter::new();
    vm.set_compile_hook(Rc::new(|source: &str, goal| {
        let code = match goal {
            CompileGoal::Function => compile_script(source),
            CompileGoal::IndirectEval => compile_indirect_eval(source),
        };
        code.map_err(|e| e.message)
    }));
    vm
}

/// `String(completion)`, or `throw <message>` / `terminated` for an abrupt completion.
fn show(vm: &mut Interpreter, result: Result<Value, Abrupt>) -> String {
    match result {
        Ok(value) => match vm.to_js_string(&value) {
            Ok(text) => text.to_string_lossy(),
            Err(_) => "<unprintable>".into(),
        },
        Err(Abrupt::Throw(value)) => format!("throw {}", vm.exception_message(&value)),
        Err(Abrupt::Terminated) => "terminated".into(),
        Err(Abrupt::Heap(error)) => format!("heap {error}"),
    }
}

fn run_in(vm: &mut Interpreter, source: &str) -> String {
    let code = compile_script(source).unwrap_or_else(|e| panic!("compile: {}", e.message));
    let result = vm.run_script(&code);
    show(vm, result)
}

fn run(source: &str) -> String {
    run_in(&mut new_vm(), source)
}

#[test]
fn indirect_eval_globals_are_configurable() {
    assert_eq!(
        run(
            "(0, eval)('var g = 1'); Object.getOwnPropertyDescriptor(globalThis, 'g').configurable"
        ),
        "true"
    );
    assert_eq!(run("(0, eval)('var h = 1'); delete globalThis.h"), "true");
    assert_eq!(
        run("(0, eval)('function k() {}'); delete globalThis.k"),
        "true"
    );
}

#[test]
fn script_globals_stay_non_configurable() {
    assert_eq!(run("var s = 1; delete globalThis.s"), "false");
}

#[test]
fn compile_errors_keep_their_kind() {
    assert_eq!(
        run(
            "try { (0, eval)('1 +') } catch (e) { e.name + (e.message.startsWith('SyntaxError') ? '!' : '') }"
        ),
        "SyntaxError"
    );
}

struct ScriptSite;

impl EvalCode for ScriptSite {
    fn compile(&self, source: &str) -> Result<FunctionCode, String> {
        compile_script(source)
            .map(Rc::unwrap_or_clone)
            .map_err(|e| e.message)
    }
}

/// Runs `prelude`, then `probe(<arg>)` with its call patched to `CallDirectEval`.
fn direct_probe(prelude: &str, arg: &str) -> String {
    let mut vm = new_vm();
    run_in(&mut vm, prelude);
    let mut code = Rc::unwrap_or_clone(compile_script(&format!("probe({arg})")).unwrap());
    let site = ConstIndex(code.constants.len() as u32);
    code.constants.push(Constant::EvalSite(Rc::new(ScriptSite)));
    let call = code
        .instructions
        .iter_mut()
        .find(|i| matches!(i, Instruction::CallUndefinedReceiver { .. }))
        .expect("a global call compiles to CallUndefinedReceiver");
    let Instruction::CallUndefinedReceiver { callee, args } = *call else {
        unreachable!()
    };
    *call = Instruction::CallDirectEval { callee, args, site };
    let result = vm.run_script(&Rc::new(code));
    show(&mut vm, result)
}

#[test]
fn call_direct_eval_dispatches_on_intrinsic() {
    assert_eq!(direct_probe("var probe = eval;", "'1 + 1'"), "2");
    assert_eq!(
        direct_probe("var probe = function () { return 'n'; };", "'1 + 1'"),
        "n"
    );
    assert_eq!(direct_probe("var probe = eval;", "7"), "7");
    assert!(direct_probe("var probe = eval;", "'1 +'").starts_with("throw SyntaxError"));
}

#[test]
fn direct_eval_sees_the_caller_this() {
    assert_eq!(direct_probe("var probe = eval;", "'typeof this'"), "object");
}
