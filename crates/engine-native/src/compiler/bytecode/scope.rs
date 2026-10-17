//! Where each binding lives, decided once per script from `oxc_semantic`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use oxc_ast::AstKind;
use oxc_ast::ast::{BindingIdentifier, BindingPattern, FormalParameters};
use oxc_semantic::{AstNodes, ReferenceId, ScopeFlags, ScopeId, Scoping, SymbolFlags, SymbolId};
use velyr_engine_vm::JsStr;
use velyr_engine_vm::bytecode::GlobalDeclarations;

mod annex_b;
#[cfg(test)]
mod tests;

use super::outer::OuterScope;
use super::unsupported;
use crate::compiler::JsResult;
#[allow(unused_imports)] // consumed by the Annex B emitter (L14)
pub use annex_b::{AnnexBFunction, BlockBinding, VarTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// Script-level declaration or unresolved name.
    Global,
    /// Only referenced from its own function: a register.
    Local,
    /// Referenced from a nested function: a slot of its scope's context.
    Context { scope: ScopeId, slot: u32 },
    /// Module import: two slots of the module context, filled when the module is linked.
    Import { slot: u32 },
}

/// Module context slot of `import.meta`.
pub const META_SLOT: u32 = 0;
/// Module context slot of an anonymous `export default`.
pub const DEFAULT_SLOT: u32 = 1;

/// A class body: its scope and the private names it declares (getter/setter pairs once).
pub struct ClassInfo {
    pub scope: ScopeId,
    /// The outer binding of a class declaration (Oxc creates no inner one).
    pub declaration: Option<SymbolId>,
    pub private_names: Vec<String>,
}

/// Class scopes count as function boundaries: their field initializers compile to
/// separate functions, so whatever they reference from outside lives in a context.
pub struct ScopeAnalysis<'s> {
    pub scoping: &'s Scoping,
    /// Module code: top-level bindings live in the module context instead of the global.
    pub module: bool,
    root: ScopeId,
    slots: HashMap<SymbolId, u32>,
    context_sizes: HashMap<ScopeId, u32>,
    class_scopes: HashSet<ScopeId>,
    private_slots: HashMap<(ScopeId, String), u32>,
    /// Inner name binding of class declarations referenced from their own body: the class
    /// scope and the context slot holding it.
    inner_names: HashMap<SymbolId, (ScopeId, u32)>,
    /// Parameters of non-simple lists (see `tdz_parameters`).
    tdz_parameters: HashSet<SymbolId>,
    /// Functions that read their `arguments` object.
    arguments_users: HashSet<ScopeId>,
    /// Context slot of `arguments` for functions whose arrows (or class keys) read it.
    arguments_slots: HashMap<ScopeId, u32>,
    /// Sloppy functions with simple parameters that read `arguments`: its indices alias
    /// the parameters, which therefore live in context slots.
    mapped_arguments: HashSet<ScopeId>,
    annex_b: annex_b::AnnexB,
    /// Outer chain entries handed to lazy stubs, one per context scope.
    pub(super) outer_cache: RefCell<HashMap<ScopeId, Rc<OuterScope>>>,
}

/// IsSimpleParameterList: identifiers only, no defaults or rest.
pub fn is_simple(params: &FormalParameters) -> bool {
    params.rest.is_none()
        && params.items.iter().all(|param| {
            param.initializer.is_none()
                && matches!(param.pattern, BindingPattern::BindingIdentifier(_))
        })
}

/// Parameters of every function whose parameter list is simple (identifiers only).
pub fn simple_parameters(nodes: &AstNodes) -> HashMap<ScopeId, Vec<SymbolId>> {
    nodes
        .iter()
        .filter_map(|node| match node.kind() {
            AstKind::Function(function) if is_simple(&function.params) => {
                let symbols =
                    function
                        .params
                        .items
                        .iter()
                        .filter_map(|param| match &param.pattern {
                            BindingPattern::BindingIdentifier(id) => Some(id.symbol_id()),
                            _ => None,
                        });
                Some((function.scope_id(), symbols.collect()))
            }
            _ => None,
        })
        .collect()
}

/// Bindings of every non-simple parameter list: they start in the TDZ, so a default value
/// reading a later (or its own) parameter throws.
pub fn tdz_parameters(nodes: &AstNodes) -> HashSet<SymbolId> {
    let mut symbols = HashSet::new();
    for node in nodes.iter() {
        let params = match node.kind() {
            AstKind::Function(function) => &function.params,
            AstKind::ArrowFunctionExpression(arrow) => &arrow.params,
            _ => continue,
        };
        if !is_simple(params) {
            for id in bound_identifiers(params) {
                symbols.insert(id.symbol_id());
            }
        }
    }
    symbols
}

/// Every identifier bound by a parameter list, in source order.
pub fn bound_identifiers<'p, 'a>(
    params: &'p FormalParameters<'a>,
) -> Vec<&'p BindingIdentifier<'a>> {
    let patterns = params.items.iter().map(|param| &param.pattern);
    let rest = params.rest.iter().map(|rest| &rest.rest.argument);
    patterns
        .chain(rest)
        .flat_map(|pattern| pattern.get_binding_identifiers())
        .collect()
}

impl<'s> ScopeAnalysis<'s> {
    pub fn new(
        scoping: &'s Scoping,
        nodes: &AstNodes,
        classes: Vec<ClassInfo>,
        simple_parameters: &HashMap<ScopeId, Vec<SymbolId>>,
        tdz_parameters: HashSet<SymbolId>,
        module: bool,
    ) -> JsResult<Self> {
        for scope in scoping.scope_descendants_from_root() {
            let flags = scoping.scope_flags(scope);
            if flags.contains(ScopeFlags::DirectEval) {
                return Err(unsupported("direct eval"));
            }
            if flags.is_with() {
                return Err(unsupported("with statement"));
            }
        }
        let mut analysis = Self {
            scoping,
            module,
            root: scoping.root_scope_id(),
            slots: HashMap::new(),
            context_sizes: HashMap::new(),
            class_scopes: classes.iter().map(|class| class.scope).collect(),
            private_slots: HashMap::new(),
            inner_names: HashMap::new(),
            tdz_parameters,
            arguments_users: HashSet::new(),
            arguments_slots: HashMap::new(),
            mapped_arguments: HashSet::new(),
            annex_b: annex_b::AnnexB::empty(),
            outer_cache: RefCell::default(),
        };
        analysis.collect_arguments_users();
        // Unreferenced parameters are only observable through `arguments`: no aliasing.
        let mut aliased = HashSet::new();
        for (&function, parameters) in simple_parameters {
            if analysis.arguments_users.contains(&function) && !analysis.is_strict(function) {
                analysis.mapped_arguments.insert(function);
                aliased.extend(
                    parameters
                        .iter()
                        .copied()
                        .filter(|&symbol| !scoping.get_resolved_reference_ids(symbol).is_empty()),
                );
            }
        }
        if module {
            analysis.new_slot(analysis.root);
            analysis.new_slot(analysis.root);
        }
        let mut captured: Vec<SymbolId> = scoping
            .symbol_ids()
            .filter(|&symbol| aliased.contains(&symbol) || analysis.is_captured(symbol))
            .collect();
        captured.sort_by_key(|&symbol| scoping.symbol_span(symbol).start);
        for symbol in captured {
            let size = analysis
                .context_sizes
                .entry(scoping.symbol_scope_id(symbol))
                .or_insert(0);
            analysis.slots.insert(symbol, *size);
            let is_import = scoping.symbol_flags(symbol).contains(SymbolFlags::Import);
            *size += if module && is_import { 2 } else { 1 };
        }
        for class in classes {
            if let Some(symbol) = class.declaration
                && analysis.is_referenced_inside(symbol, class.scope)
            {
                let slot = analysis.new_slot(class.scope);
                analysis.inner_names.insert(symbol, (class.scope, slot));
            }
            for name in class.private_names {
                let slot = analysis.new_slot(class.scope);
                analysis.private_slots.insert((class.scope, name), slot);
            }
        }
        analysis.allocate_arguments_slots();
        analysis.collect_annex_b(nodes);
        Ok(analysis)
    }

    fn is_referenced_inside(&self, symbol: SymbolId, class: ScopeId) -> bool {
        let mut references = self.scoping.get_resolved_references(symbol);
        references.any(|reference| {
            let mut ancestors = self.scoping.scope_ancestors(reference.scope_id());
            ancestors.any(|scope| scope == class)
        })
    }

    pub fn is_tdz_parameter(&self, symbol: SymbolId) -> bool {
        self.tdz_parameters.contains(&symbol)
    }

    /// The class scope and slot of the inner name binding of a class declaration.
    pub fn inner_name(&self, symbol: SymbolId) -> Option<(ScopeId, u32)> {
        self.inner_names.get(&symbol).copied()
    }

    /// Inner name bindings living in `scope`, with their slots.
    pub fn inner_names_of(&self, scope: ScopeId) -> impl Iterator<Item = (SymbolId, u32)> + '_ {
        let declared = self.inner_names.iter();
        declared
            .filter(move |(_, (class, _))| *class == scope)
            .map(|(&symbol, &(_, slot))| (symbol, slot))
    }

    fn new_slot(&mut self, scope: ScopeId) -> u32 {
        let size = self.context_sizes.entry(scope).or_insert(0);
        *size += 1;
        *size - 1
    }

    /// Scopes of the references to the implicit `arguments`, with their owning function.
    fn arguments_references(&self) -> Vec<(ScopeId, ScopeId)> {
        let scoping = self.scoping;
        scoping
            .root_unresolved_references()
            .iter()
            .filter(|(name, _)| name.as_str() == "arguments")
            .flat_map(|(_, references)| references.iter())
            .map(|&id| {
                let scope = scoping.get_reference(id).scope_id();
                (scope, self.arguments_owner(scope))
            })
            .filter(|&(_, owner)| owner != self.root)
            .collect()
    }

    fn collect_arguments_users(&mut self) {
        for (_, owner) in self.arguments_references() {
            self.arguments_users.insert(owner);
        }
    }

    fn allocate_arguments_slots(&mut self) {
        for (scope, owner) in self.arguments_references() {
            if self.function_scope(scope) != owner && !self.arguments_slots.contains_key(&owner) {
                let slot = self.new_slot(owner);
                self.arguments_slots.insert(owner, slot);
            }
        }
    }

    fn is_captured(&self, symbol: SymbolId) -> bool {
        let scope = self.scoping.symbol_scope_id(symbol);
        if scope == self.root {
            return self.module;
        }
        if self.class_scopes.contains(&scope) {
            return !self.scoping.get_resolved_reference_ids(symbol).is_empty();
        }
        let home = self.function_scope(scope);
        self.scoping
            .get_resolved_references(symbol)
            .any(|reference| self.function_scope(reference.scope_id()) != home)
    }

    /// The nearest enclosing function (or script, static block or class) scope, `scope`
    /// included.
    pub fn function_scope(&self, scope: ScopeId) -> ScopeId {
        self.scoping
            .scope_ancestors(scope)
            .find(|&ancestor| {
                let flags = self.scoping.scope_flags(ancestor);
                flags.is_function()
                    || flags.is_top()
                    || flags.is_class_static_block()
                    || self.class_scopes.contains(&ancestor)
            })
            .unwrap_or(self.root)
    }

    /// The function whose code allocates `scope`'s context: a class scope is entered by
    /// the code around the class.
    fn creating_function(&self, scope: ScopeId) -> ScopeId {
        match self.scoping.scope_parent_id(scope) {
            Some(parent) if self.class_scopes.contains(&scope) => self.function_scope(parent),
            _ => self.function_scope(scope),
        }
    }

    /// The function an `arguments` reference in `scope` belongs to (arrows and class
    /// scopes are transparent).
    pub fn arguments_owner(&self, scope: ScopeId) -> ScopeId {
        self.scoping
            .scope_ancestors(scope)
            .find(|&ancestor| {
                let flags = self.scoping.scope_flags(ancestor);
                (flags.is_function() && !flags.is_arrow())
                    || flags.is_top()
                    || flags.is_class_static_block()
            })
            .unwrap_or(self.root)
    }

    pub fn arguments_slot(&self, function: ScopeId) -> Option<u32> {
        self.arguments_slots.get(&function).copied()
    }

    pub fn private_slot(&self, class: ScopeId, name: &str) -> Option<u32> {
        self.private_slots.get(&(class, name.to_owned())).copied()
    }

    /// Private names declared by `scope` (a class) with their context slots.
    pub fn private_entries(&self, scope: ScopeId) -> Vec<(Box<str>, u32)> {
        let entries = self.private_slots.iter();
        let declared = entries.filter(|((class, _), _)| *class == scope);
        declared
            .map(|((_, name), &slot)| (name.as_str().into(), slot))
            .collect()
    }

    pub fn is_class_scope(&self, scope: ScopeId) -> bool {
        self.class_scopes.contains(&scope)
    }

    /// Private names of a class with their context slots, in declaration order.
    pub fn private_names(&self, class: ScopeId) -> Vec<u32> {
        let mut slots: Vec<u32> = self
            .private_slots
            .iter()
            .filter(|((scope, _), _)| *scope == class)
            .map(|(_, &slot)| slot)
            .collect();
        slots.sort_unstable();
        slots
    }

    pub fn location(&self, symbol: SymbolId) -> Location {
        let scope = self.scoping.symbol_scope_id(symbol);
        if scope == self.root && !self.module {
            return Location::Global;
        }
        match self.slots.get(&symbol) {
            Some(&slot) if self.flags(symbol).contains(SymbolFlags::Import) => {
                Location::Import { slot }
            }
            Some(&slot) => Location::Context { scope, slot },
            None => Location::Local,
        }
    }

    pub fn context_size(&self, scope: ScopeId) -> u32 {
        self.context_sizes.get(&scope).copied().unwrap_or(0)
    }

    /// Whether a scope below `function` (not `function` itself) allocates a context.
    pub fn has_inner_contexts(&self, function: ScopeId) -> bool {
        self.context_sizes
            .keys()
            .any(|&scope| scope != function && self.creating_function(scope) == function)
    }

    pub fn symbol_of(&self, reference: ReferenceId) -> Option<SymbolId> {
        self.scoping.get_reference(reference).symbol_id()
    }

    pub fn name(&self, symbol: SymbolId) -> &'s str {
        self.scoping.symbol_name(symbol)
    }

    pub fn flags(&self, symbol: SymbolId) -> SymbolFlags {
        self.scoping.symbol_flags(symbol)
    }

    pub fn is_strict(&self, scope: ScopeId) -> bool {
        self.scoping.scope_flags(scope).is_strict_mode()
    }

    /// Bindings declared directly in `scope`, in source order.
    pub fn bindings(&self, scope: ScopeId) -> Vec<SymbolId> {
        let mut symbols: Vec<SymbolId> =
            self.scoping.get_bindings(scope).values().copied().collect();
        symbols.sort_by_key(|&symbol| self.scoping.symbol_span(symbol).start);
        symbols
    }

    /// Register-held bindings of the function whose scope is `function`, in source order.
    pub fn local_symbols(&self, function: ScopeId) -> Vec<SymbolId> {
        let mut symbols: Vec<SymbolId> = self
            .scoping
            .symbol_ids()
            .filter(|&symbol| {
                self.location(symbol) == Location::Local
                    && self.creating_function(self.scoping.symbol_scope_id(symbol)) == function
            })
            .collect();
        symbols.sort_by_key(|&symbol| self.scoping.symbol_span(symbol).start);
        symbols
    }

    /// Whether `function` (or an arrow in it) reads the implicit `arguments` object.
    pub fn uses_arguments(&self, function: ScopeId) -> bool {
        self.arguments_users.contains(&function)
    }

    /// Whether `function`'s `arguments` object aliases its parameters.
    pub fn has_mapped_arguments(&self, function: ScopeId) -> bool {
        self.mapped_arguments.contains(&function)
    }

    pub fn global_declarations(&self) -> GlobalDeclarations {
        let mut declarations = GlobalDeclarations::default();
        let mut var_names = HashSet::new();
        for symbol in self.bindings(self.root) {
            let annex_b = self.annex_b_function(symbol);
            if annex_b.is_some_and(|info| matches!(info.block, BlockBinding::Synthetic(_))) {
                // A block function Oxc hoisted to the root: declared through `annex_b`.
                continue;
            }
            let flags = self.flags(symbol);
            if !is_lexical(flags) {
                var_names.insert(self.name(symbol));
            }
            let name = JsStr::from_str(self.name(symbol));
            let list = if flags.contains(SymbolFlags::Function) {
                &mut declarations.functions
            } else if flags.contains(SymbolFlags::ConstVariable) {
                &mut declarations.consts
            } else if flags.intersects(SymbolFlags::BlockScopedVariable | SymbolFlags::Class) {
                &mut declarations.lets
            } else {
                &mut declarations.vars
            };
            list.push(name);
        }
        // B.3.2.2 step 12.a: names already declared by a function or var are skipped.
        let annex_b = self.annex_b_globals().into_iter();
        let annex_b = annex_b.filter(|name| !var_names.contains(name));
        declarations.annex_b = annex_b.map(JsStr::from_str).collect();
        declarations
    }
}

/// `let`/`const`/`class` bindings: they start in the TDZ.
pub fn is_lexical(flags: SymbolFlags) -> bool {
    flags.intersects(
        SymbolFlags::BlockScopedVariable | SymbolFlags::ConstVariable | SymbolFlags::Class,
    )
}
