//! Annex B.3.2 (block-level functions) and B.3.4 (`var` in a `catch` block), decided once per
//! script. Analysis only: the compiler consumes it through `annex_b_function`,
//! `annex_b_globals` and `catch_parameter_for`.

use std::collections::{HashMap, HashSet};

use oxc_ast::AstKind;
use oxc_ast::ast::{BindingPattern, Function};
use oxc_semantic::{AstNodes, NodeId, ScopeId, SymbolFlags, SymbolId};

use super::{Location, ScopeAnalysis, bound_identifiers};

/// Where the block-scoped copy of a block-level function lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockBinding {
    /// The declaration's own symbol (a var binding with that name already existed).
    Symbol(SymbolId),
    /// The symbol moved to the var scope: the block binding is synthetic, a register
    /// (allocated by the function compiler) or a slot of the block's context.
    Synthetic(Location),
}

/// The var-scoped copy that receives the function when its declaration is evaluated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarTarget {
    Symbol(SymbolId),
    /// Script top level: a property of the global object.
    Global(Box<str>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnexBFunction {
    pub block_scope: ScopeId,
    pub block: BlockBinding,
    pub var: VarTarget,
}

pub(super) struct AnnexB {
    functions: HashMap<SymbolId, AnnexBFunction>,
    /// Simple `catch (e)` parameters by the scope of the catch clause.
    catch_parameters: HashMap<ScopeId, SymbolId>,
    /// `if` clause functions whose name is a parameter or a lexical binding of the var scope:
    /// `var name` would not be created (B.3.3.1), so the declaration has no effect outside.
    skipped_clause_functions: HashSet<SymbolId>,
}

impl AnnexB {
    pub(super) fn empty() -> Self {
        Self {
            functions: HashMap::new(),
            catch_parameters: HashMap::new(),
            skipped_clause_functions: HashSet::new(),
        }
    }
}

/// A plain function declaration in a sloppy block: `(declaration symbol, block scope)`.
fn block_function(scope: &ScopeAnalysis, function: &Function) -> Option<(SymbolId, ScopeId)> {
    if function.r#async || function.generator || !function.is_declaration() {
        return None;
    }
    let symbol = function.id.as_ref()?.symbol_id();
    let block = scope.scoping.scope_parent_id(function.scope_id())?;
    let flags = scope.scoping.scope_flags(block);
    (!flags.is_var() && !flags.is_strict_mode()).then_some((symbol, block))
}

impl ScopeAnalysis<'_> {
    pub(super) fn collect_annex_b(&mut self, nodes: &AstNodes) {
        let mut candidates = Vec::new();
        for node in nodes.iter() {
            match node.kind() {
                AstKind::Function(function) => {
                    candidates.extend(block_function(self, function));
                    self.collect_skipped_clause_function(nodes, node.id(), function);
                }
                AstKind::CatchClause(clause) => {
                    if let (Some(param), Some(scope)) = (&clause.param, clause.scope_id.get())
                        && let BindingPattern::BindingIdentifier(id) = &param.pattern
                    {
                        self.annex_b.catch_parameters.insert(scope, id.symbol_id());
                    }
                }
                _ => {}
            }
        }
        candidates.sort_by_key(|&(symbol, _)| self.scoping.symbol_span(symbol).start);
        candidates.dedup();
        let block_functions: HashSet<(ScopeId, &str)> = candidates
            .iter()
            .map(|&(symbol, block)| (block, self.name(symbol)))
            .collect();
        for (symbol, block) in candidates {
            if let Some(info) = self.annex_b_entry(nodes, &block_functions, symbol, block) {
                self.annex_b.functions.insert(symbol, info);
            }
        }
    }

    /// B.3.3.1: an `if` clause function (given no scope of its own by Oxc) that shares its name
    /// with a parameter or a let/const/class of the var scope.
    fn collect_skipped_clause_function(
        &mut self,
        nodes: &AstNodes,
        node: NodeId,
        function: &Function,
    ) {
        let (Some(id), true) = (&function.id, function.is_declaration()) else {
            return;
        };
        if !matches!(nodes.parent_kind(node), AstKind::IfStatement(_))
            || function.r#async
            || function.generator
        {
            return;
        }
        let symbol = id.symbol_id();
        let scope = self.scoping.symbol_scope_id(symbol);
        let flags = self.scoping.scope_flags(scope);
        if flags.is_var()
            && !flags.is_strict_mode()
            && (super::is_lexical(self.flags(symbol))
                || self.is_parameter_or_arguments(nodes, scope, self.name(symbol)))
        {
            self.annex_b.skipped_clause_functions.insert(symbol);
        }
    }

    pub fn is_skipped_clause_function(&self, symbol: SymbolId) -> bool {
        self.annex_b.skipped_clause_functions.contains(&symbol)
    }

    fn annex_b_entry(
        &mut self,
        nodes: &AstNodes,
        block_functions: &HashSet<(ScopeId, &str)>,
        symbol: SymbolId,
        block: ScopeId,
    ) -> Option<AnnexBFunction> {
        let name = self.name(symbol);
        let var_scope = self.var_scope_of(block)?;
        let between = self.scoping.scope_ancestors(block).skip(1);
        for scope in between.take_while(|&scope| scope != var_scope) {
            if block_functions.contains(&(scope, name)) || self.blocks_var(scope, name) {
                return None;
            }
        }
        if self
            .scoping
            .get_bindings(var_scope)
            .get(name)
            .is_some_and(|&other| {
                let flags = self.flags(other);
                super::is_lexical(flags) && !flags.contains(SymbolFlags::Function)
            })
        {
            return None;
        }
        if self.is_parameter_or_arguments(nodes, var_scope, name) {
            return None;
        }
        let moved = self.scoping.symbol_scope_id(symbol) != block;
        let var_symbol = if moved {
            Some(symbol)
        } else {
            self.scoping.get_bindings(var_scope).get(name).copied()
        };
        // B.3.2.3: sloppy eval code in a function writes its caller's var scope.
        let is_global = var_scope == self.root && !self.module && !self.is_eval_in_function();
        let var = match (is_global, var_symbol) {
            (true, _) => VarTarget::Global(name.into()),
            (false, Some(var_symbol)) => VarTarget::Symbol(var_symbol),
            (false, None) => return None,
        };
        let block_binding = match moved {
            false => BlockBinding::Symbol(symbol),
            true => BlockBinding::Synthetic(self.synthetic_location(symbol, block)),
        };
        Some(AnnexBFunction {
            block_scope: block,
            block: block_binding,
            var,
        })
    }

    fn var_scope_of(&self, block: ScopeId) -> Option<ScopeId> {
        let mut ancestors = self.scoping.scope_ancestors(block).skip(1);
        ancestors.find(|&scope| self.scoping.scope_flags(scope).is_var())
    }

    /// Whether a declaration in `scope` (between a block function and its var scope) would
    /// make `var name` an early error. A simple `catch (name)` parameter does not (B.3.4).
    fn blocks_var(&self, scope: ScopeId, name: &str) -> bool {
        let Some(&other) = self.scoping.get_bindings(scope).get(name) else {
            return false;
        };
        self.annex_b.catch_parameters.get(&scope) != Some(&other)
    }

    fn is_parameter_or_arguments(&self, nodes: &AstNodes, var_scope: ScopeId, name: &str) -> bool {
        if name == "arguments" && var_scope != self.root {
            return true;
        }
        let node = nodes.get_node(self.scoping.get_node_id(var_scope));
        let params = match node.kind() {
            AstKind::Function(function) => &function.params,
            AstKind::ArrowFunctionExpression(arrow) => &arrow.params,
            _ => return false,
        };
        bound_identifiers(params).iter().any(|id| id.name == name)
    }

    /// A context slot of the block when a closure inside it names the function, else a register.
    fn synthetic_location(&mut self, symbol: SymbolId, block: ScopeId) -> Location {
        let home = self.function_scope(block);
        let captured = self
            .scoping
            .get_resolved_references(symbol)
            .any(|reference| {
                let scope = reference.scope_id();
                let inside = self.scoping.scope_ancestors(scope).any(|s| s == block);
                inside && self.function_scope(scope) != home
            });
        match captured {
            true => Location::Context {
                scope: block,
                slot: self.new_slot(block),
            },
            false => Location::Local,
        }
    }

    pub fn annex_b_function(&self, declaration: SymbolId) -> Option<&AnnexBFunction> {
        self.annex_b.functions.get(&declaration)
    }

    #[allow(dead_code)] // consumed by L14's `global_declarations()`
    /// Names of the `VarTarget::Global` entries, in source order, without repeats.
    pub fn annex_b_globals(&self) -> Vec<&str> {
        let mut globals: Vec<(u32, &str)> = self
            .annex_b
            .functions
            .iter()
            .filter_map(|(&symbol, info)| match &info.var {
                VarTarget::Global(name) => Some((self.scoping.symbol_span(symbol).start, &**name)),
                VarTarget::Symbol(_) => None,
            })
            .collect();
        globals.sort_unstable();
        let mut names: Vec<&str> = globals.into_iter().map(|(_, name)| name).collect();
        let mut seen = HashSet::new();
        names.retain(|name| seen.insert(*name));
        names
    }

    #[allow(dead_code)] // consumed by the Annex B emitter (L14)
    /// B.3.4: the simple `catch (name)` parameter a `var name` declared in `declarator_scope`
    /// writes through.
    pub fn catch_parameter_for(&self, declarator_scope: ScopeId, name: &str) -> Option<SymbolId> {
        for scope in self.scoping.scope_ancestors(declarator_scope) {
            let parameter = self.annex_b.catch_parameters.get(&scope).copied();
            if let Some(symbol) = parameter.filter(|&symbol| self.name(symbol) == name) {
                return Some(symbol);
            }
            if self.scoping.scope_flags(scope).is_var() {
                return None;
            }
        }
        None
    }

    /// Synthetic block bindings living in `scope`'s context, with their slots.
    pub fn annex_b_context_bindings(
        &self,
        scope: ScopeId,
    ) -> impl Iterator<Item = (SymbolId, u32)> + '_ {
        self.annex_b
            .functions
            .iter()
            .filter_map(move |(&symbol, info)| match info.block {
                BlockBinding::Synthetic(Location::Context { scope: s, slot }) if s == scope => {
                    Some((symbol, slot))
                }
                _ => None,
            })
    }
}
