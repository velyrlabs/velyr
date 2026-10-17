//! Direct eval with static scope (19.2.1.1 PerformEval): a call named `eval` keeps an
//! `EvalSite` constant with the contexts around it (like a lazy body's outer chain), and
//! the VM hands it the text when the callee is %eval%. Eval code compiles in eval root
//! mode (19.2.1.3), for indirect eval too.

use std::rc::Rc;

use oxc_ast::AstKind;
use oxc_ast::ast::{CallExpression, IdentifierReference, Program};
use oxc_semantic::{AstNodes, ScopeId};
use velyr_engine_vm::bytecode::{
    Constant, EvalCode, FunctionCode, FunctionKind, GlobalDeclarations, Instruction as I,
};
use velyr_engine_vm::exception::Script;

use super::function::FunctionCompiler;
use super::outer::OuterScope;
use super::scope::{EvalHome, RootMode, ScopeAnalysis};
use crate::compiler::{JsResult, throw};

const NEW_TARGET_OUTSIDE_FUNCTION: &str = "Unexpected new.target expression";
const SUPER_PROPERTY_OUTSIDE_METHOD: &str =
    "'super' can only be referenced in members of derived classes or object literal expressions.";
const SUPER_CALL_OUTSIDE_CONSTRUCTOR: &str = "Super calls are not permitted outside constructors or in nested functions inside constructors.";

/// The static scope of a direct eval call site.
pub struct EvalSite {
    outer: Option<Rc<OuterScope>>,
    strict: bool,
    var_scope_is_global: bool,
    home: Option<EvalHome>,
}

impl EvalCode for EvalSite {
    fn compile(&self, source: &str) -> Result<FunctionCode, String> {
        self.compile_eval(source).map_err(|error| error.message)
    }
}

impl EvalSite {
    fn compile_eval(&self, source: &str) -> JsResult<FunctionCode> {
        let text = match self.strict {
            true => format!("'use strict';{source}"),
            false => source.to_owned(),
        };
        let mode = RootMode::Eval {
            var_scope_is_global: self.var_scope_is_global,
            home: self.home,
        };
        let allow =
            |message: &str, offset: u32, nodes: &AstNodes| self.allows(message, offset, nodes);
        super::analyze_with(&text, mode, Some(&allow), |program, _, analysis| {
            if self.home.is_some_and(|home| home.class_field) && reads_arguments(analysis) {
                let message = "'arguments' is not allowed in class field initializer";
                return Err(throw("SyntaxError", message));
            }
            let script = Rc::new(Script::new("<eval>", &text));
            compile_eval_program(program, analysis, script, self.outer.clone())
        })
    }

    /// The early errors PerformEval lifts for code in this site's function.
    fn allows(&self, message: &str, offset: u32, nodes: &AstNodes) -> bool {
        let Some(home) = self.home else { return false };
        match message {
            NEW_TARGET_OUTSIDE_FUNCTION => true,
            SUPER_PROPERTY_OUTSIDE_METHOD => home.method && is_eval_level_super(offset, nodes),
            SUPER_CALL_OUTSIDE_CONSTRUCTOR => {
                home.derived_constructor && is_eval_level_super(offset, nodes)
            }
            _ => false,
        }
    }
}

/// Whether the `super` at `offset` belongs to the eval code itself (only arrows between).
fn is_eval_level_super(offset: u32, nodes: &AstNodes) -> bool {
    let found = nodes
        .iter()
        .find(|node| matches!(node.kind(), AstKind::Super(sup) if sup.span.start == offset));
    let Some(node) = found else { return false };
    let mut ancestors = nodes.ancestor_kinds(node.id());
    !ancestors.any(|kind| matches!(kind, AstKind::Function(_)))
}

/// Whether eval code names the `arguments` of its caller (not of a function of its own).
fn reads_arguments(analysis: &ScopeAnalysis) -> bool {
    let scoping = analysis.scoping;
    let root = scoping.root_scope_id();
    let unresolved = scoping.root_unresolved_references().iter();
    let mut references = unresolved
        .filter(|(name, _)| name.as_str() == "arguments")
        .flat_map(|(_, references)| references.iter());
    references.any(|&id| analysis.arguments_owner(scoping.get_reference(id).scope_id()) == root)
}

/// Eval code: a script body whose vars are configurable globals (or locals, see
/// `RootMode::Eval`), run in its caller's context with its `this` and `new.target`.
fn compile_eval_program(
    program: &Program,
    analysis: &ScopeAnalysis,
    script: Rc<Script>,
    outer: Option<Rc<OuterScope>>,
) -> JsResult<FunctionCode> {
    let scope = program.scope_id();
    let mut compiler = FunctionCompiler::new(analysis, scope, FunctionKind::Eval, script);
    compiler.eager = true;
    compiler.outer = outer;
    let completion = compiler.e.alloc();
    compiler.completion = Some(completion);
    compiler.assign_local_registers();
    let declarations = GlobalDeclarations {
        configurable: true,
        ..analysis.global_declarations()
    };
    if declarations.vars.len() + declarations.functions.len() + declarations.annex_b.len() > 0 {
        let index = compiler
            .e
            .constant(Constant::GlobalDeclarations(Rc::new(declarations)));
        compiler.e.emit(I::DeclareGlobals(index));
    }
    let saved = compiler.enter_scope(scope);
    compiler.hoist_functions(&program.body)?;
    for statement in &program.body {
        compiler.statement(statement)?;
    }
    compiler.exit_scope(saved);
    compiler.e.emit(I::Ldar(completion));
    compiler.e.emit(I::Return);
    let header = compiler.header("", 0, 0, program.span);
    Ok(compiler.e.finish(header))
}

/// Compiles the text of an indirect eval: global eval code (19.2.1.3), whose lexicals stay
/// local and whose var and function declarations become configurable global properties.
pub fn compile_indirect_eval(text: &str) -> JsResult<Rc<FunctionCode>> {
    let site = EvalSite {
        outer: None,
        strict: false,
        var_scope_is_global: true,
        home: None,
    };
    site.compile_eval(text).map(Rc::new)
}

impl FunctionCompiler<'_> {
    /// `eval(args)`: the callee and arguments as for a call, then `CallDirectEval`.
    pub(super) fn direct_eval(
        &mut self,
        call: &CallExpression,
        id: &IdentifierReference,
    ) -> JsResult<()> {
        let (callee, _) = self.callee(&call.callee)?;
        let args = self.arguments_in_registers(&call.arguments)?;
        let scope = self
            .analysis
            .scoping
            .get_reference(id.reference_id())
            .scope_id();
        let site = self.eval_site(scope);
        let site = self.e.constant(Constant::EvalSite(Rc::new(site)));
        self.e
            .emit_at(call.span, I::CallDirectEval { callee, args, site });
        Ok(())
    }

    /// The analysis' home of `scope`, refined by this compiler's kind when it is its own
    /// scope: a lazy body parses a method as a plain `function`.
    pub(super) fn eval_home(&self, scope: ScopeId) -> Option<EvalHome> {
        let home = self.analysis.eval_home(scope)?;
        if scope != self.scope {
            return Some(home);
        }
        let method = home.method
            || matches!(
                self.kind,
                FunctionKind::Method
                    | FunctionKind::Getter
                    | FunctionKind::Setter
                    | FunctionKind::ClassConstructor
                    | FunctionKind::DerivedConstructor
                    | FunctionKind::ClassField
            );
        Some(EvalHome {
            method,
            derived_constructor: home.derived_constructor
                || self.kind == FunctionKind::DerivedConstructor,
            class_field: home.class_field || self.kind == FunctionKind::ClassField,
        })
    }

    fn eval_site(&self, scope: ScopeId) -> EvalSite {
        let analysis = self.analysis;
        let mut ancestors = analysis.scoping.scope_ancestors(scope);
        let local = ancestors.find_map(|ancestor| self.eval_home(ancestor));
        let (home, var_scope_is_global) = match (local, analysis.mode) {
            (Some(home), _) => (Some(home), false),
            (
                None,
                RootMode::Eval {
                    var_scope_is_global,
                    home,
                },
            ) => (home, var_scope_is_global),
            (None, RootMode::Module) => (self.outer_home(), false),
            (None, RootMode::Script) => {
                // A lazy arrow's function is outside this parse, on its outer chain.
                let home = self.outer_home();
                (home, home.is_none() && self.outer_module().is_none())
            }
        };
        EvalSite {
            outer: self.outer_chain(),
            strict: analysis.is_strict(scope),
            var_scope_is_global,
            home,
        }
    }
}
