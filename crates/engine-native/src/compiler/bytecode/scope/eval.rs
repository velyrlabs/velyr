//! Scope analysis for direct eval (19.2.1.1 PerformEval) and eval code roots (19.2.1.3).

use std::collections::{HashMap, HashSet};

use oxc_ast::AstKind;
use oxc_ast::ast::{MethodDefinitionKind, PropertyKind};
use oxc_semantic::{AstNodes, ScopeId, Scoping, SymbolId};

use super::{ScopeAnalysis, is_lexical};

/// What the root scope of the analyzed text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootMode {
    Script,
    Module,
    /// Eval code: root lexicals are local; root vars and functions are global when the
    /// code is sloppy and its var scope is the global one. `home` is the eval site's.
    Eval {
        var_scope_is_global: bool,
        home: Option<EvalHome>,
    },
}

/// The function an eval site belongs to (arrows are transparent): what its code may name
/// beyond plain script code. Its presence means `new.target` is allowed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EvalHome {
    /// `super.x` is allowed (a method, accessor, field initializer or static block).
    pub method: bool,
    /// `super()` is allowed.
    pub derived_constructor: bool,
    /// `arguments` is a SyntaxError (a field initializer).
    pub class_field: bool,
}

/// The home of every function, class and static block scope containing a direct eval.
pub(super) fn eval_homes(
    scoping: &Scoping,
    nodes: &AstNodes,
    class_scopes: &HashSet<ScopeId>,
) -> HashMap<ScopeId, EvalHome> {
    let root = scoping.root_scope_id();
    if !scoping.scope_flags(root).contains_direct_eval() {
        return HashMap::new();
    }
    let scopes = scoping.scope_descendants_from_root();
    let with_eval =
        scopes.filter(|&scope| scope != root && scoping.scope_flags(scope).contains_direct_eval());
    with_eval
        .filter_map(|scope| Some((scope, home_of(scoping, nodes, class_scopes, scope)?)))
        .collect()
}

fn home_of(
    scoping: &Scoping,
    nodes: &AstNodes,
    class_scopes: &HashSet<ScopeId>,
    scope: ScopeId,
) -> Option<EvalHome> {
    let flags = scoping.scope_flags(scope);
    let method = EvalHome {
        method: true,
        ..EvalHome::default()
    };
    if class_scopes.contains(&scope) {
        // shortcut: a class heritage or computed key counts as a field initializer, upgrade if Test262 shows it
        return Some(EvalHome {
            class_field: true,
            ..method
        });
    }
    if flags.is_class_static_block() {
        return Some(method);
    }
    if !flags.is_function() || flags.is_arrow() {
        return None;
    }
    let function = scoping.get_node_id(scope);
    let parent = nodes.parent_id(function);
    let home = match nodes.kind(parent) {
        AstKind::MethodDefinition(definition) => EvalHome {
            derived_constructor: definition.kind == MethodDefinitionKind::Constructor
                && is_derived_class(nodes, parent),
            ..method
        },
        AstKind::ObjectProperty(property)
            if property.method || property.kind != PropertyKind::Init =>
        {
            method
        }
        _ => EvalHome::default(),
    };
    Some(home)
}

/// Whether the class around the method definition `node` has an `extends` clause.
fn is_derived_class(nodes: &AstNodes, node: oxc_semantic::NodeId) -> bool {
    let mut current = node;
    loop {
        current = nodes.parent_id(current);
        match nodes.kind(current) {
            AstKind::Class(class) => return class.heritage.is_some(),
            AstKind::Program(_) => return false,
            _ => {}
        }
    }
}

impl ScopeAnalysis<'_> {
    /// Non-arrow functions containing a direct eval: the eval may read their `arguments`.
    pub(super) fn eval_functions(&self) -> Vec<ScopeId> {
        let scopes = self.eval_homes.keys().copied();
        let functions = scopes.filter(|&scope| {
            let flags = self.scoping.scope_flags(scope);
            flags.is_function() && !flags.is_arrow() && !self.class_scopes.contains(&scope)
        });
        functions.collect()
    }

    /// The function, class or static block scope containing a direct eval at `scope`.
    pub fn eval_home(&self, scope: ScopeId) -> Option<EvalHome> {
        self.eval_homes.get(&scope).copied()
    }

    /// Whether a root binding is a global name rather than a register or context slot.
    pub(super) fn is_global_root_binding(&self, symbol: SymbolId) -> bool {
        match self.mode {
            RootMode::Script => true,
            RootMode::Module => false,
            RootMode::Eval {
                var_scope_is_global,
                ..
            } => {
                var_scope_is_global && !self.is_strict(self.root) && !is_lexical(self.flags(symbol))
            }
        }
    }
}
