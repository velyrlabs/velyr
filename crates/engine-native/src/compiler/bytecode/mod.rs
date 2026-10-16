//! Oxc AST -> register bytecode (`velyr_engine_vm::bytecode`), with scopes from
//! `oxc_semantic`.
//!
//! Bindings live in one of three places: script-level declarations are global names,
//! bindings captured by a nested function are context slots, everything else is a
//! register. Constructs outside the supported subset fail with
//! `SyntaxError: unsupported: <construct>` instead of compiling to wrong code.

mod assignment;
mod call;
mod class;
mod control;
mod emitter;
mod expression;
mod function;
#[path = "../generators.rs"]
mod generators;
mod lazy;
mod literal;
mod loops;
#[path = "../../modules/compile.rs"]
mod module;
mod outer;
mod pattern;
mod scope;
mod statement;
mod variable;

use std::rc::Rc;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::{AstNodes, SemanticBuilder};
use oxc_span::SourceType;
use velyr_engine_api::JsException;
use velyr_engine_vm::bytecode::FunctionCode;
use velyr_engine_vm::exception::Script;

use crate::compiler::{JsResult, throw};
pub use module::compile_module;

/// Compiles a classic script into its top-level `FunctionCode`.
pub fn compile_script(text: &str) -> JsResult<Rc<FunctionCode>> {
    compile_named_script("<anonymous>", text)
}

/// `compile_script` with the script name stack traces report.
pub fn compile_named_script(name: &str, text: &str) -> JsResult<Rc<FunctionCode>> {
    analyze(text, false, true, |program, _, analysis| {
        let script = Rc::new(Script::new(name, text));
        function::compile_program(program, analysis, script)
    })
}

/// Parses `text` (as a module with `module`), reports its early errors when `check`, and
/// hands the program, its nodes and its scope analysis to `run`. Without `check` (a lazy
/// body, out of already checked text) parser errors are ignored too: its wrapper puts
/// `new.target` of an arrow outside a function, which the parser reports, and a fatal one
/// leaves an empty program, which the caller rejects.
fn analyze<R>(
    text: &str,
    module: bool,
    check: bool,
    run: impl FnOnce(&Program, &AstNodes, &scope::ScopeAnalysis) -> JsResult<R>,
) -> JsResult<R> {
    let allocator = Allocator::default();
    let options = ParseOptions {
        preserve_parens: false,
        ..ParseOptions::default()
    };
    let source_type = match module {
        true => SourceType::mjs(),
        false => SourceType::script(),
    };
    let parsed = Parser::new(&allocator, text, source_type)
        .with_options(options)
        .parse();
    if let Some(error) = parsed.diagnostics.first()
        && check
    {
        return Err(throw("SyntaxError", &error.message));
    }
    // Node store needed by `collect_classes` (private names per class) and lazy bodies.
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .with_check_syntax_error(check)
        .build(&parsed.program);
    if let Some(error) = semantic.diagnostics.first() {
        return Err(throw("SyntaxError", &error.message));
    }
    let classes = class::collect_classes(semantic.semantic.nodes());
    let parameters = scope::simple_parameters(semantic.semantic.nodes());
    let tdz_parameters = scope::tdz_parameters(semantic.semantic.nodes());
    let (scoping, nodes) = semantic.semantic.into_scoping_and_nodes();
    let analysis =
        scope::ScopeAnalysis::new(&scoping, classes, &parameters, tdz_parameters, module)?;
    run(&parsed.program, &nodes, &analysis)
}

fn unsupported(construct: &str) -> JsException {
    throw("SyntaxError", format!("unsupported: {construct}"))
}
