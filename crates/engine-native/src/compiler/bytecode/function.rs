//! Per-function compiler state, prologues, scope entry/exit and function hoisting.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use oxc_ast::ast::{
    ArrowFunctionExpression, BindingIdentifier, BindingPattern, Expression, FormalParameter,
    FormalParameters, Function, Program, Statement,
};
use oxc_semantic::{ScopeId, SymbolFlags, SymbolId};
use oxc_span::{GetSpan, Span};
use velyr_engine_vm::bytecode::{
    ConstIndex, Constant, CoroutineKind, FunctionCode, FunctionKind, GlobalDeclarations,
    Instruction as I, Reg,
};
use velyr_engine_vm::exception::Script;

use super::control::Control;
use super::emitter::{Emitter, Header, Label};
use super::generators::Coroutine;
use super::outer::OuterScope;
use super::scope::{
    BlockBinding, EvalHome, Location, ScopeAnalysis, bound_identifiers, is_lexical, is_simple,
};
use crate::compiler::JsResult;

pub struct FunctionCompiler<'s> {
    pub e: Emitter,
    pub analysis: &'s ScopeAnalysis<'s>,
    /// Register of every `Location::Local` binding of this function.
    pub registers: HashMap<SymbolId, Reg>,
    /// Scopes whose context is on the context chain, innermost last.
    pub contexts: Vec<ScopeId>,
    pub control: Vec<Control>,
    /// Lexical bindings already initialized on every path to the code compiled next.
    pub initialized: HashSet<SymbolId>,
    pub switch_scopes: HashSet<ScopeId>,
    pub scope: ScopeId,
    pub kind: FunctionKind,
    pub is_strict: bool,
    /// Script completion value register.
    pub completion: Option<Reg>,
    pub arguments: Option<Reg>,
    /// Where an optional link jumps when its base is nullish.
    pub optional_chain: Option<Label>,
    /// Enclosing class scopes (private name environments), innermost last.
    pub classes: Vec<ScopeId>,
    pub script: Rc<Script>,
    /// Set by the `CreateGenerator` prologue of generators and async functions.
    pub coroutine: Option<Coroutine>,
    /// Compile nested function expressions and arrows now instead of on their first call:
    /// code that runs once (script and module top level, and what it compiles eagerly)
    /// tends to call them right away (IIFEs, callbacks), which would re-parse the text.
    pub eager: bool,
    /// Contexts around a lazy body, below `contexts` (see `outer.rs`).
    pub outer: Option<Rc<OuterScope>>,
    /// Set while compiling parameter expressions, which do not see the body's bindings.
    pub parameters_end: Option<u32>,
    /// Register of each `BlockBinding::Synthetic(Local)` Annex B.3.2 block binding.
    pub annex_b_registers: HashMap<SymbolId, Reg>,
    /// B.3.4: a `catch (e)` parameter Oxc merged with a `var e` of its body, bound to its
    /// own register while that body compiles.
    pub catch_bindings: HashMap<SymbolId, Reg>,
    /// Eval home of a lazy body's own function, from its stub (this parse lacks its parent).
    pub own_home: Option<EvalHome>,
}

pub(super) enum Body<'b, 'a> {
    Statements(&'b [Statement<'a>]),
    Expression(&'b Expression<'a>),
}

pub(super) struct FunctionSource<'b, 'a> {
    pub scope: ScopeId,
    pub params: Option<&'b FormalParameters<'a>>,
    pub body: Body<'b, 'a>,
    pub span: Span,
    /// End of the function node itself (`span` may be its class's).
    pub end: u32,
    pub self_name: Option<&'b BindingIdentifier<'a>>,
    pub coroutine: Option<CoroutineKind>,
}

pub fn compile_program(
    program: &Program,
    analysis: &ScopeAnalysis,
    script: Rc<Script>,
) -> JsResult<Rc<FunctionCode>> {
    let scope = program.scope_id();
    let mut compiler = FunctionCompiler::new(analysis, scope, FunctionKind::Script, script);
    let completion = compiler.e.alloc();
    compiler.completion = Some(completion);
    compiler.assign_local_registers();
    let declarations = analysis.global_declarations();
    if declarations != GlobalDeclarations::default() {
        let index = compiler
            .e
            .constant(Constant::GlobalDeclarations(Rc::new(declarations)));
        compiler.e.emit(I::DeclareGlobals(index));
    }
    compiler.hoist_functions(&program.body)?;
    for statement in &program.body {
        compiler.statement(statement)?;
    }
    compiler.e.emit(I::Ldar(completion));
    compiler.e.emit(I::Return);
    let header = compiler.header("", 0, 0, program.span);
    Ok(Rc::new(compiler.e.finish(header)))
}

impl<'s> FunctionCompiler<'s> {
    pub(super) fn new(
        analysis: &'s ScopeAnalysis<'s>,
        scope: ScopeId,
        kind: FunctionKind,
        script: Rc<Script>,
    ) -> Self {
        Self {
            e: Emitter::default(),
            analysis,
            registers: HashMap::new(),
            contexts: Vec::new(),
            control: Vec::new(),
            initialized: HashSet::new(),
            switch_scopes: HashSet::new(),
            scope,
            kind,
            is_strict: analysis.is_strict(scope),
            completion: None,
            arguments: None,
            optional_chain: None,
            classes: Vec::new(),
            script,
            coroutine: None,
            eager: kind == FunctionKind::Script,
            outer: None,
            parameters_end: None,
            annex_b_registers: HashMap::new(),
            catch_bindings: HashMap::new(),
            own_home: None,
        }
    }

    pub(super) fn header(
        &self,
        name: &str,
        parameter_count: u32,
        length: u32,
        span: Span,
    ) -> Header {
        Header {
            name: self.e.intern(name),
            kind: self.kind,
            is_strict: self.is_strict,
            parameter_count,
            length,
            span,
            script: Some(self.script.clone()),
        }
    }

    /// Emits `CreateClosure` for a function expression or declaration.
    pub fn closure(&mut self, function: &Function, name: &str, kind: FunctionKind) -> JsResult<()> {
        self.method_closure(function, name, kind, None).map(|_| ())
    }

    /// `closure`, returning whether the function needs a [[HomeObject]] (uses `super.x`).
    /// `source` is the span `toString` reports when it is not the function's own (methods
    /// start at their key, class constructors span the whole class).
    pub fn method_closure(
        &mut self,
        function: &Function,
        name: &str,
        kind: FunctionKind,
        source: Option<Span>,
    ) -> JsResult<bool> {
        let code = self.function_code(function, name, kind, source)?;
        let needs_home = uses_super_property(&code);
        let index: ConstIndex = self.e.constant(Constant::Function(Rc::new(code)));
        self.e.emit(I::CreateClosure(index));
        Ok(needs_home)
    }

    pub fn function_code(
        &mut self,
        function: &Function,
        name: &str,
        kind: FunctionKind,
        source: Option<Span>,
    ) -> JsResult<FunctionCode> {
        let source = FunctionSource::function(function, source)?;
        let is_expression = kind == FunctionKind::Normal && function.is_expression();
        // Generator and async functions are not constructors, like methods.
        let kind = match kind {
            FunctionKind::Normal if source.coroutine.is_some() => FunctionKind::Method,
            kind => kind,
        };
        let name = function.id.as_ref().map_or(name, |id| id.name.as_str());
        self.nested_code(&source, name, kind, is_expression)
    }

    /// The code of a nested function: compiled now (an expression in eager code), else a
    /// stub compiled on its first call.
    fn nested_code(
        &mut self,
        source: &FunctionSource,
        name: &str,
        kind: FunctionKind,
        is_expression: bool,
    ) -> JsResult<FunctionCode> {
        if !(self.eager && is_expression) {
            return Ok(self.lazy_stub(source, name, kind, is_expression));
        }
        let mut inner = self.inner(source.scope, kind);
        inner.eager = true;
        inner.compile_function(source, name)
    }

    /// A class static block as a parameterless method called with the class as `this`.
    pub fn block_code(
        &mut self,
        scope: ScopeId,
        statements: &[Statement],
        span: Span,
    ) -> JsResult<FunctionCode> {
        let source = FunctionSource {
            scope,
            params: None,
            body: Body::Statements(statements),
            span,
            end: span.end,
            self_name: None,
            coroutine: None,
        };
        self.inner(scope, FunctionKind::Method)
            .compile_function(&source, "")
    }

    /// A field initializer: a function of the class scope returning the field value.
    /// With `named_by_key` it takes the field key as its argument and names the value
    /// (an anonymous function definition) after it.
    pub fn initializer_code(
        &mut self,
        class_scope: ScopeId,
        value: &Expression,
        name: Option<&str>,
        named_by_key: bool,
    ) -> JsResult<FunctionCode> {
        let mut inner = self.inner(class_scope, FunctionKind::ClassField);
        let key = named_by_key.then(|| inner.e.alloc());
        inner.expression_named(value, name)?;
        if let Some(key) = key {
            inner.e.emit(I::NameFunction(key));
        }
        inner.e.emit(I::Return);
        let header = inner.header("", u32::from(named_by_key), 0, value.span());
        Ok(inner.e.finish(header))
    }

    fn inner(&self, scope: ScopeId, kind: FunctionKind) -> FunctionCompiler<'s> {
        let mut inner = FunctionCompiler::new(self.analysis, scope, kind, self.script.clone());
        inner.contexts = self.contexts.clone();
        inner.classes = self.classes.clone();
        inner.outer = self.outer.clone();
        inner.e.pool = self.e.pool.clone();
        inner.e.shift = self.e.shift;
        inner
    }

    pub fn arrow(&mut self, arrow: &ArrowFunctionExpression, name: &str) -> JsResult<()> {
        let source = FunctionSource::arrow(arrow);
        let code = self.nested_code(&source, name, FunctionKind::Arrow, true)?;
        let index: ConstIndex = self.e.constant(Constant::Function(Rc::new(code)));
        self.e.emit(I::CreateClosure(index));
        Ok(())
    }

    pub(super) fn compile_function(
        mut self,
        source: &FunctionSource,
        name: &str,
    ) -> JsResult<FunctionCode> {
        let items = source.params.map_or(&[][..], |params| &params.items[..]);
        let is_simple = source.params.is_none_or(is_simple);
        let arguments = self.e.alloc_range(items.len() as u32);
        if is_simple {
            for (index, param) in items.iter().enumerate() {
                if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                    self.registers
                        .insert(id.symbol_id(), Reg(arguments.first.0 + index as u32));
                }
            }
        }
        self.assign_local_registers();
        let async_body = self.start_coroutine(source.coroutine, true);
        self.enter_scope(self.scope);
        self.prepare_arguments_object(items)?;
        self.bind_self_name(source.self_name)?;
        match source.params {
            Some(params) if is_simple => self.copy_captured_parameters(params, arguments.first)?,
            Some(params) => self.bind_parameters(params, arguments.first)?,
            None => {}
        }
        match source.body {
            Body::Statements(statements) => {
                self.hoist_functions(statements)?;
                self.start_coroutine(source.coroutine, false);
                self.statement_list(statements)?;
                // A derived constructor's `undefined` becomes its `this` (VM return check).
                self.e.emit(I::LdaUndefined);
            }
            Body::Expression(expression) => self.expression(expression)?,
        }
        self.e.emit(I::Return);
        self.finish_async_function(async_body);
        let length = items.iter().take_while(|p| p.initializer.is_none()).count();
        let header = self.header(name, arguments.count, length as u32, source.span);
        Ok(self.e.finish(header))
    }

    /// Gives every register-held binding of this function (blocks included) a register.
    pub(super) fn assign_local_registers(&mut self) {
        for symbol in self.analysis.local_symbols(self.scope) {
            if !self.registers.contains_key(&symbol) {
                let register = self.e.alloc();
                self.registers.insert(symbol, register);
            }
        }
    }

    fn prepare_arguments_object(&mut self, items: &[FormalParameter]) -> JsResult<()> {
        if self.kind == FunctionKind::Arrow || !self.analysis.uses_arguments(self.scope) {
            return Ok(());
        }
        self.e.emit(I::CreateUnmappedArguments);
        if self.analysis.has_mapped_arguments(self.scope) {
            self.map_arguments(items);
        }
        if let Some(slot) = self.analysis.arguments_slot(self.scope) {
            self.e.emit(I::StaContextSlot { depth: 0, slot });
            return Ok(());
        }
        let register = self.e.alloc();
        self.e.emit(I::Star(register));
        self.arguments = Some(register);
        Ok(())
    }

    /// Aliases each argument index to its parameter's context slot; a duplicated name maps
    /// only its last index. Unreferenced parameters stay unmapped (unobservable).
    fn map_arguments(&mut self, items: &[FormalParameter]) {
        let mut seen = HashSet::new();
        for (index, param) in items.iter().enumerate().rev() {
            let BindingPattern::BindingIdentifier(id) = &param.pattern else {
                continue;
            };
            if !seen.insert(id.symbol_id()) {
                continue;
            }
            if let Location::Context { slot, .. } = self.analysis.location(id.symbol_id()) {
                self.e.emit(I::MapArgument(index as u32, slot));
            }
        }
    }

    fn bind_self_name(&mut self, name: Option<&BindingIdentifier>) -> JsResult<()> {
        let Some(id) = name else { return Ok(()) };
        let symbol = id.symbol_id();
        if !self.analysis.is_named(symbol) {
            return Ok(());
        }
        self.e.emit(I::LdaCurrentClosure);
        self.initialize_binding(symbol)
    }

    fn copy_captured_parameters(&mut self, params: &FormalParameters, first: Reg) -> JsResult<()> {
        for (index, param) in params.items.iter().enumerate() {
            let BindingPattern::BindingIdentifier(id) = &param.pattern else {
                continue;
            };
            if matches!(
                self.analysis.location(id.symbol_id()),
                Location::Context { .. }
            ) {
                self.e.emit(I::Ldar(Reg(first.0 + index as u32)));
                self.initialize_binding(id.symbol_id())?;
            }
        }
        Ok(())
    }

    fn bind_parameters(&mut self, params: &FormalParameters, first: Reg) -> JsResult<()> {
        self.parameters_end = Some(params.span.end);
        for id in bound_identifiers(params) {
            self.put_in_tdz(id.symbol_id())?;
        }
        for (index, param) in params.items.iter().enumerate() {
            self.e.emit(I::Ldar(Reg(first.0 + index as u32)));
            if let Some(initializer) = &param.initializer {
                self.default_value(initializer, binding_name(&param.pattern))?;
            }
            self.bind_pattern(&param.pattern)?;
        }
        if let Some(rest) = &params.rest {
            self.e
                .emit(I::CreateRestParameter(params.items.len() as u32));
            self.bind_pattern(&rest.rest.argument)?;
        }
        self.parameters_end = None;
        Ok(())
    }

    /// Function declarations directly in `statements` (labelled ones included, B.3.1) are
    /// initialized on scope entry.
    pub fn hoist_functions(&mut self, statements: &[Statement]) -> JsResult<()> {
        for mut statement in statements {
            while let Statement::LabeledStatement(labeled) = statement {
                statement = &labeled.body;
            }
            let Statement::FunctionDeclaration(function) = statement else {
                continue;
            };
            let Some(id) = &function.id else { continue };
            self.closure(function, id.name.as_str(), FunctionKind::Normal)?;
            self.initialize_function(id.symbol_id())?;
        }
        Ok(())
    }

    /// A block function hoisted to this var scope (B.3.2.1): `undefined` until its
    /// declaration is evaluated, unlike a var-scoped function initialized on entry.
    fn is_annex_b_var(&self, symbol: SymbolId) -> bool {
        let info = self.analysis.annex_b_function(symbol);
        info.is_some_and(|info| matches!(info.block, BlockBinding::Synthetic(_)))
    }

    /// Allocates the scope's context if it has captured bindings and puts its lexical
    /// registers in the TDZ. Returns the register holding the outer context.
    pub fn enter_scope(&mut self, scope: ScopeId) -> Option<Reg> {
        let size = self.analysis.context_size(scope);
        let saved = (size > 0 || self.analysis.is_eval_var_scope(scope)).then(|| {
            let saved = self.e.alloc();
            self.create_context(scope, size);
            self.e.emit(I::PushContext(saved));
            self.contexts.push(scope);
            self.control.push(Control::Context(saved));
            saved
        });
        for symbol in self.analysis.bindings(scope) {
            let flags = self.analysis.flags(symbol);
            match self.analysis.location(symbol) {
                Location::Local if is_lexical(flags) => {
                    // A class nested in a class heritage or field initializer belongs to
                    // the code compiling it, which `assign_local_registers` does not see.
                    let register = match self.registers.get(&symbol) {
                        Some(&register) => register,
                        None => {
                            let register = self.e.alloc();
                            self.registers.insert(symbol, register);
                            register
                        }
                    };
                    self.e.emit(I::LdaTheHole);
                    self.e.emit(I::Star(register));
                }
                Location::Local if self.is_annex_b_var(symbol) => {
                    if let Some(&register) = self.registers.get(&symbol) {
                        self.e.emit(I::LdaUndefined);
                        self.e.emit(I::Star(register));
                    }
                }
                Location::Context { slot, .. }
                    if !is_lexical(flags)
                        && (!flags.contains(SymbolFlags::Function)
                            || self.is_annex_b_var(symbol)) =>
                {
                    self.e.emit(I::LdaUndefined);
                    self.e.emit(I::StaContextSlot { depth: 0, slot });
                }
                _ => {}
            }
        }
        saved
    }

    pub fn exit_scope(&mut self, saved: Option<Reg>) {
        if let Some(saved) = saved {
            self.e.emit(I::PopContext(saved));
            self.contexts.pop();
            self.control.pop();
        }
    }
}

/// Whether `code` (or an arrow inside it) reads or writes a `super` property.
pub(super) fn uses_super_property(code: &FunctionCode) -> bool {
    if code.lazy.is_some() {
        // Not compiled yet: any `super` or `eval` in its text (an unneeded [[HomeObject]] is unused).
        let script = code.script.as_ref();
        let text = script.map(|s| s.slice(code.source_start, code.source_end));
        return text.is_some_and(|text| text.contains("super") || text.contains("eval"));
    }
    code.instructions.iter().any(|i| {
        matches!(
            i,
            I::GetSuper
                | I::SetSuper(_)
                | I::GetSuperFrom(_)
                | I::SetSuperFrom { .. }
                | I::CallDirectEval { .. }
        )
    }) || code.constants.iter().any(|constant| {
        matches!(constant, Constant::Function(inner)
                if inner.kind == FunctionKind::Arrow && uses_super_property(inner))
    })
}

pub fn binding_name<'b>(pattern: &'b BindingPattern) -> Option<&'b str> {
    match pattern {
        BindingPattern::BindingIdentifier(id) => Some(id.name.as_str()),
        _ => None,
    }
}
