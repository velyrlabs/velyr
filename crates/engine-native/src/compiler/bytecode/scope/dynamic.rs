//! Dynamic scope (14.11 `with`, 19.2.1.3 sloppy direct eval `var` injection): which
//! references resolve by name at run time and which contexts carry a names table.
//!
//! A reference is dynamic when a `with` scope, or a sloppy function whose direct eval may
//! inject a `var` into it, lies between the reference and its declaration (or the root,
//! for an unresolved name). Its target binding lives in a named context.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use oxc_ast::AstKind;
use oxc_semantic::{AstNodes, ReferenceId, ScopeId, SymbolFlags, SymbolId};
use oxc_span::{GetSpan, Span};
use velyr_engine_vm::JsStr;

use super::{Location, RootMode, ScopeAnalysis, is_lexical};

#[derive(Default)]
pub(super) struct DynamicScopes {
    references: HashSet<ReferenceId>,
    /// Bindings a dynamic reference may reach: kept in a named context.
    targets: HashSet<SymbolId>,
    /// Scopes whose context carries a names table.
    named: HashSet<ScopeId>,
    /// Sloppy functions containing a direct eval (contexts with `var_scope`).
    var_scopes: HashSet<ScopeId>,
    with_scopes: HashSet<ScopeId>,
    /// Body span and enclosing function of every `with` statement.
    with_bodies: Vec<(Span, ScopeId)>,
}

impl ScopeAnalysis<'_> {
    pub(super) fn collect_dynamic_scopes(&mut self, nodes: &AstNodes) {
        let scoping = self.scoping;
        let mut dynamic = DynamicScopes::default();
        for node in nodes.iter() {
            match node.kind() {
                AstKind::WithStatement(with) => {
                    let scope = with.scope_id();
                    dynamic.with_scopes.insert(scope);
                    dynamic
                        .with_bodies
                        .push((with.body.span(), self.function_scope(scope)));
                }
                AstKind::CallExpression(call)
                    if !call.optional && call.callee.is_specific_id("eval") =>
                {
                    dynamic
                        .var_scopes
                        .extend(self.eval_var_scope(node.scope_id()));
                }
                _ => {}
            }
        }
        for scope in scoping.scope_descendants_from_root() {
            let flags = scoping.scope_flags(scope);
            // Eval code may hold a `with` or inject a `var`: what it sees is named.
            if flags.contains_direct_eval() && !flags.is_strict_mode() {
                dynamic.named.insert(scope);
            }
        }
        if !dynamic.with_scopes.is_empty() || !dynamic.var_scopes.is_empty() {
            self.collect_dynamic_references(&mut dynamic);
        }
        for &scope in dynamic.with_scopes.iter().chain(&dynamic.var_scopes) {
            // Pushes a context: seen by `has_inner_contexts`.
            self.context_sizes.entry(scope).or_insert(0);
        }
        self.dynamic = dynamic;
    }

    /// The function (arrows included) a sloppy direct eval at `scope` may add `var`s to.
    fn eval_var_scope(&self, scope: ScopeId) -> Option<ScopeId> {
        if self.is_strict(scope) {
            return None;
        }
        let mut ancestors = self.scoping.scope_ancestors(scope);
        let var_scope = ancestors.find(|&ancestor| self.is_var_scope_kind(ancestor))?;
        let is_function = self.scoping.scope_flags(var_scope).is_function();
        is_function.then_some(var_scope)
    }

    fn collect_dynamic_references(&self, dynamic: &mut DynamicScopes) {
        let scoping = self.scoping;
        let resolved = scoping.symbol_ids().flat_map(|symbol| {
            let references = scoping.get_resolved_reference_ids(symbol).iter();
            references.map(move |&reference| (reference, Some(symbol)))
        });
        let unresolved = scoping.root_unresolved_references().values();
        let unresolved = unresolved.flatten().map(|&reference| (reference, None));
        for (reference, symbol) in resolved.chain(unresolved) {
            let target = symbol.map(|symbol| scoping.symbol_scope_id(symbol));
            let scope = scoping.get_reference(reference).scope_id();
            let mut between = scoping
                .scope_ancestors(scope)
                .take_while(|&ancestor| Some(ancestor) != target);
            if !between.any(|a| dynamic.with_scopes.contains(&a) || dynamic.var_scopes.contains(&a))
            {
                continue;
            }
            dynamic.references.insert(reference);
            if let (Some(symbol), Some(target)) = (symbol, target) {
                dynamic.targets.insert(symbol);
                dynamic.named.insert(target);
            }
        }
    }

    /// A function, static block or the root: what a `var` belongs to.
    fn is_var_scope_kind(&self, scope: ScopeId) -> bool {
        self.scoping.scope_flags(scope).is_var()
    }

    pub fn is_dynamic_reference(&self, reference: ReferenceId) -> bool {
        self.dynamic.references.contains(&reference)
    }

    /// Whether `symbol` gets a context slot: captured or `aliased` (by `arguments`), or a
    /// dynamic target; eval-injected names have none.
    pub(super) fn needs_slot(&self, symbol: SymbolId, aliased: &HashSet<SymbolId>) -> bool {
        if self.is_eval_injected(symbol) {
            return false;
        }
        let is_target = self.dynamic.targets.contains(&symbol);
        is_target || aliased.contains(&symbol) || self.is_captured(symbol)
    }

    pub fn is_with_scope(&self, scope: ScopeId) -> bool {
        self.dynamic.with_scopes.contains(&scope)
    }

    pub fn is_eval_var_scope(&self, scope: ScopeId) -> bool {
        self.dynamic.var_scopes.contains(&scope)
    }

    /// Whether code can name `symbol`: it has references, or a direct eval sees it.
    pub fn is_named(&self, symbol: SymbolId) -> bool {
        let scope = self.scoping.symbol_scope_id(symbol);
        !self.scoping.get_resolved_reference_ids(symbol).is_empty()
            || self.scoping.scope_flags(scope).contains_direct_eval()
    }

    /// Whether `var` initializer stores of `symbol` declared at `position` go through a
    /// `with` object (14.11: PutValue on the resolved name).
    pub fn is_var_in_with(&self, symbol: SymbolId, position: u32) -> bool {
        let flags = self.flags(symbol);
        let not_var = SymbolFlags::Function | SymbolFlags::CatchVariable;
        if !flags.contains(SymbolFlags::FunctionScopedVariable) || flags.intersects(not_var) {
            return false;
        }
        let home = self.function_scope(self.scoping.symbol_scope_id(symbol));
        let mut bodies = self.dynamic.with_bodies.iter();
        bodies.any(|&(span, function)| {
            function == home && span.start <= position && position < span.end
        })
    }

    /// A root `var` or function of sloppy eval code whose var scope is a function: declared
    /// at entry with `DeclareEvalVar` and always looked up by name.
    pub(super) fn is_eval_injected(&self, symbol: SymbolId) -> bool {
        let is_root = self.scoping.symbol_scope_id(symbol) == self.root;
        is_root && self.is_eval_in_function() && !is_lexical(self.flags(symbol))
    }

    /// Names of the eval-injected root bindings.
    pub fn eval_injected_names(&self) -> impl Iterator<Item = &str> + '_ {
        let bindings = self.bindings(self.root).into_iter();
        let injected = bindings.filter(|&symbol| self.is_eval_injected(symbol));
        injected.map(|symbol| self.name(symbol))
    }

    /// Sloppy eval code whose var scope is its caller's function.
    pub(super) fn is_eval_in_function(&self) -> bool {
        let in_function = matches!(
            self.mode,
            RootMode::Eval {
                var_scope_is_global: false,
                ..
            }
        );
        in_function && !self.is_strict(self.root)
    }

    /// The names table of `scope`'s context: entry `i` names slot `i` (`""` for slots that
    /// no identifier can name).
    pub fn context_names(&self, scope: ScopeId) -> Option<Rc<[(JsStr, bool)]>> {
        if !self.dynamic.named.contains(&scope) && !self.is_eval_var_scope(scope) {
            return None;
        }
        let is_var_scope = self.is_var_scope_kind(scope);
        let mut names: HashMap<u32, (JsStr, bool)> = HashMap::new();
        for symbol in self.bindings(scope) {
            let Location::Context { slot, .. } = self.location(symbol) else {
                continue;
            };
            let flags = self.flags(symbol);
            let lexical = match is_var_scope {
                true => is_lexical(flags),
                // 19.2.1.3 step 3.d: any binding crossed blocks an eval `var`, except a
                // simple `catch` parameter (B.3.4).
                false => !flags.contains(SymbolFlags::CatchVariable),
            };
            names.insert(slot, (JsStr::from_str(self.name(symbol)), lexical));
        }
        let lexical_slots = self.inner_names_of(scope);
        let lexical_slots = lexical_slots.chain(self.annex_b_context_bindings(scope));
        for (symbol, slot) in lexical_slots.collect::<Vec<_>>() {
            names.insert(slot, (JsStr::from_str(self.name(symbol)), true));
        }
        let size = self.context_size(scope);
        let unnamed = (JsStr::from_str(""), false);
        let table = (0..size).map(|slot| names.remove(&slot).unwrap_or_else(|| unnamed.clone()));
        Some(table.collect())
    }
}
