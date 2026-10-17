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
mod dynamic;
mod emitter;
mod eval;
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
mod using;
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
pub use eval::compile_indirect_eval;
pub use module::compile_module;
use scope::RootMode;

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
    let mode = match module {
        true => RootMode::Module,
        false => RootMode::Script,
    };
    let reject: Allow = &|_, _, _| false;
    analyze_with(text, mode, check.then_some(reject), run)
}

/// Whether an early error (message, offset of its first label) is not one for this text.
type Allow<'f> = &'f dyn Fn(&str, u32, &AstNodes) -> bool;

/// `analyze` with the root as `mode`; with `check`, the early errors it does not allow.
fn analyze_with<R>(
    text: &str,
    mode: RootMode,
    check: Option<Allow>,
    run: impl FnOnce(&Program, &AstNodes, &scope::ScopeAnalysis) -> JsResult<R>,
) -> JsResult<R> {
    let allocator = Allocator::default();
    let options = ParseOptions {
        preserve_parens: false,
        ..ParseOptions::default()
    };
    let source_type = match mode {
        RootMode::Module => SourceType::mjs(),
        _ => SourceType::script(),
    };
    let parsed = Parser::new(&allocator, text, source_type)
        .with_options(options)
        .parse();
    // Node store needed by `collect_classes` (private names per class) and lazy bodies.
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .with_check_syntax_error(check.is_some())
        .build(&parsed.program);
    let parse_errors = parsed.diagnostics.iter().filter(|_| check.is_some());
    let allow = check.unwrap_or(&|_, _, _| false);
    let nodes = semantic.semantic.nodes();
    let mut errors = parse_errors.chain(&semantic.diagnostics);
    if let Some(error) = errors.find(|error| {
        let offset = error.labels.first().map_or(0, |label| label.offset());
        !allow(&error.message, offset, nodes)
    }) {
        return Err(throw("SyntaxError", &error.message));
    }
    let classes = class::collect_classes(semantic.semantic.nodes());
    let parameters = scope::simple_parameters(semantic.semantic.nodes());
    let tdz_parameters = scope::tdz_parameters(semantic.semantic.nodes());
    let (scoping, nodes) = semantic.semantic.into_scoping_and_nodes();
    let analysis =
        scope::ScopeAnalysis::new(&scoping, &nodes, classes, &parameters, tdz_parameters, mode)?;
    run(&parsed.program, &nodes, &analysis)
}

fn unsupported(construct: &str) -> JsException {
    throw("SyntaxError", format!("unsupported: {construct}"))
}
