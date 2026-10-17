//! Names resolved at run time (`scope/dynamic.rs`): `with` statements, references a sloppy
//! direct eval may shadow, and the `var`s sloppy eval code adds to its caller's function.

use oxc_ast::ast::{
    BindingIdentifier, BindingPattern, IdentifierReference, Program, WithStatement,
};
use oxc_semantic::{ScopeId, SymbolFlags};
use velyr_engine_vm::bytecode::{Constant, FunctionKind, Instruction as I, Reg};

use super::assignment::Place;
use super::control::Control;
use super::function::FunctionCompiler;
use super::scope::Location;
use crate::compiler::JsResult;

impl FunctionCompiler<'_> {
    /// Whether `id` resolves by name at run time.
    pub(super) fn uses_lookup(&self, id: &IdentifierReference) -> bool {
        if self.arguments_owner(id).is_some() {
            return false;
        }
        let reference = id.reference_id();
        if self.analysis.is_dynamic_reference(reference) {
            return true;
        }
        match self.analysis.symbol_of(reference) {
            Some(symbol) => self.analysis.location(symbol) == Location::Lookup,
            None => self.outer_is_dynamic(&id.name),
        }
    }

    /// `acc` = the value of the name `id` (`typeof` flavour: unresolvable is `undefined`).
    pub(super) fn load_lookup(&mut self, id: &IdentifierReference, for_typeof: bool) {
        let name = self.e.string(&id.name);
        self.e.emit(match for_typeof {
            true => I::LdaLookupForTypeof(name),
            false => I::LdaLookup(name),
        });
    }

    pub(super) fn store_lookup(&mut self, name: &str) {
        let name = self.e.string(name);
        let strict = self.is_strict;
        self.e.emit(I::StaLookup { name, strict });
    }

    pub(super) fn delete_lookup(&mut self, id: &IdentifierReference) {
        let name = self.e.string(&id.name);
        self.e.emit(I::DeleteLookup(name));
    }

    /// `acc` = the callee `id`, and its receiver (the `with` object or `undefined`).
    pub(super) fn callee_lookup(&mut self, id: &IdentifierReference) -> Reg {
        let receiver = self.e.alloc();
        let name = self.e.string(&id.name);
        self.e.emit(I::LdaLookupCallee { name, receiver });
        receiver
    }

    /// An assignment target named `id`, resolved now (before its value is evaluated).
    pub(super) fn resolve_lookup<'b, 'a>(&mut self, name: &str) -> Place<'b, 'a> {
        let base = self.e.alloc();
        let name = self.e.string(name);
        self.e.emit(I::ResolveLookup { name, base });
        Place::Lookup { name, base }
    }

    /// `var name = init` inside a `with`: the name resolves before `init` (14.3.2.1).
    pub(super) fn var_in_with_place<'b, 'a>(
        &mut self,
        pattern: &BindingPattern,
    ) -> Option<Place<'b, 'a>> {
        let BindingPattern::BindingIdentifier(id) = pattern else {
            return None;
        };
        let is_in_with = self.analysis.is_var_in_with(id.symbol_id(), id.span.start);
        is_in_with.then(|| self.resolve_lookup(&id.name))
    }

    /// InitializeBinding of `acc` on a declared name; a `var` initializer inside a `with`
    /// is a PutValue on the resolved name instead.
    pub fn bind_identifier(&mut self, id: &BindingIdentifier) -> JsResult<()> {
        let symbol = id.symbol_id();
        if !self.analysis.is_var_in_with(symbol, id.span.start) {
            return self.initialize_binding(symbol);
        }
        self.store_lookup(&id.name);
        Ok(())
    }

    /// `CreateContext`, with a names table when dynamic references may search it.
    pub fn create_context(&mut self, scope: ScopeId, size: u32) {
        let Some(names) = self.analysis.context_names(scope) else {
            return self.e.emit(I::CreateContext(size));
        };
        let names = self.e.constant(Constant::ContextNames(names));
        let var_scope = self.analysis.is_eval_var_scope(scope);
        self.e.emit(I::CreateContextWithNames {
            count: size,
            names,
            var_scope,
        });
    }

    /// 14.11.2: the body runs in an object environment over ToObject(object).
    pub(super) fn with_statement(&mut self, statement: &WithStatement) -> JsResult<()> {
        self.reset_completion();
        self.expression(&statement.object)?;
        let object = self.e.alloc();
        self.e.emit(I::Star(object));
        let saved = self.e.alloc();
        self.e.emit_at(statement.span, I::PushWithContext(object));
        self.e.emit(I::PushContext(saved));
        self.contexts.push(statement.scope_id());
        self.control.push(Control::Context(saved));
        let result = self.statement(&statement.body);
        self.exit_scope(Some(saved));
        result
    }

    /// While a parameter list compiles: its bindings, and `arguments` unless in an arrow
    /// (10.2.11 steps 19-20 give them an environment of their own).
    pub(super) fn parameter_names(&self) -> Vec<String> {
        if self.parameters_end.is_none() {
            return Vec::new();
        }
        let analysis = self.analysis;
        let bindings = analysis.bindings(self.scope).into_iter();
        let parameters = bindings.filter(|&symbol| analysis.is_tdz_parameter(symbol));
        let mut names: Vec<String> = parameters.map(|s| analysis.name(s).to_owned()).collect();
        if self.kind != FunctionKind::Arrow {
            names.push("arguments".to_owned());
        }
        names
    }

    /// 19.2.1.3 steps 3 and 10-11: the root `var`s and functions of sloppy eval code whose
    /// var scope is its caller's function.
    pub(super) fn declare_eval_vars(&mut self, program: &Program) {
        let analysis = self.analysis;
        for symbol in analysis.bindings(program.scope_id()) {
            if analysis.location(symbol) != Location::Lookup {
                continue;
            }
            let name = self.e.string(analysis.name(symbol));
            let function = analysis.flags(symbol).contains(SymbolFlags::Function);
            self.e.emit(I::DeclareEvalVar { name, function });
        }
    }
}
