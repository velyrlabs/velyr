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
mod literal;
mod loops;
#[path = "../../modules/compile.rs"]
mod module;
mod pattern;
mod scope;
mod statement;
mod variable;

use std::rc::Rc;

use oxc_allocator::Allocator;
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::SemanticBuilder;
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
    let allocator = Allocator::default();
    let options = ParseOptions {
        preserve_parens: false,
        ..ParseOptions::default()
    };
    let parsed = Parser::new(&allocator, text, SourceType::script())
        .with_options(options)
        .parse();
    if let Some(error) = parsed.diagnostics.first() {
        return Err(throw("SyntaxError", &error.message));
    }
    // Node store needed by `collect_classes` (private names per class).
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .with_check_syntax_error(true)
        .build(&parsed.program);
    if let Some(error) = semantic.diagnostics.first() {
        return Err(throw("SyntaxError", &error.message));
    }
    let classes = class::collect_classes(semantic.semantic.nodes());
    let parameters = scope::simple_parameters(semantic.semantic.nodes());
    let scoping = semantic.semantic.into_scoping();
    let analysis = scope::ScopeAnalysis::new(&scoping, classes, &parameters, false)?;
    let script = Rc::new(Script::new(name, text));
    function::compile_program(&parsed.program, &analysis, script)
}

fn unsupported(construct: &str) -> JsException {
    throw("SyntaxError", format!("unsupported: {construct}"))
}
