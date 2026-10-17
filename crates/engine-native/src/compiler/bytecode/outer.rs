//! Outer scopes of a lazy body (VELYR-201), like V8's ScopeInfo: its first call parses only
//! the function's own text, so whatever that text names from around it comes from here.
//! One entry per context on the chain at the stub (one hop each), shared by every stub
//! below it, with the bindings captured there, its private names and `arguments` slot.

use std::rc::Rc;

use oxc_semantic::{ScopeId, SymbolFlags};

use super::function::FunctionCompiler;
use super::scope::{EvalHome, Location, ScopeAnalysis};

pub struct OuterScope {
    bindings: Box<[OuterBinding]>,
    privates: Box<[(Box<str>, u32)]>,
    /// Slot of the `arguments` object of the function owning this context.
    arguments: Option<u32>,
    /// The module context (`import.meta`).
    module: bool,
    /// Set when this is the context of a function (or class) containing a direct eval.
    home: Option<EvalHome>,
    /// A `with` object or eval-injected `var`s may shadow names found further out.
    dynamic: bool,
    parent: Option<Rc<OuterScope>>,
}

struct OuterBinding {
    name: Box<str>,
    found: OuterBindingSlot,
}

/// A captured binding of an outer scope.
#[derive(Clone, Copy)]
pub struct OuterBindingSlot {
    pub slot: u32,
    pub flags: SymbolFlags,
    pub import: bool,
    /// A class's own name in its class scope (immutable).
    pub class_name: bool,
}

/// What an unresolved name of a lazy body is outside it.
#[derive(Clone, Copy)]
pub enum OuterRef {
    Binding(OuterBindingSlot),
    /// Slot of the `arguments` object of the function an arrow belongs to.
    Arguments(u32),
}

impl ScopeAnalysis<'_> {
    /// The chain entry of `scope`'s context (cached: a scope always has the same parents);
    /// `before` keeps only bindings declared before it (parameter expressions).
    pub fn outer_scope(
        &self,
        scope: ScopeId,
        parent: Option<Rc<OuterScope>>,
        before: Option<u32>,
        home: Option<EvalHome>,
    ) -> Rc<OuterScope> {
        // A class scope's home depends on the compiler (field initializers have one).
        if before.is_none()
            && let Some(cached) = self.outer_cache.borrow().get(&scope)
            && cached.home == home
        {
            return cached.clone();
        }
        let is_class = self.is_class_scope(scope);
        let bindings = self.bindings(scope).into_iter().filter_map(|symbol| {
            let start = self.scoping.symbol_span(symbol).start;
            if before.is_some_and(|before| start >= before) {
                return None;
            }
            let flags = self.flags(symbol);
            let (slot, import) = match self.location(symbol) {
                Location::Context { slot, .. } => (slot, false),
                Location::Import { slot } => (slot, true),
                _ => return None,
            };
            let class_name = is_class && flags.contains(SymbolFlags::Class);
            let found = OuterBindingSlot {
                slot,
                flags,
                import,
                class_name,
            };
            let name = self.name(symbol).into();
            Some(OuterBinding { name, found })
        });
        let inner = self
            .inner_names_of(scope)
            .map(|(symbol, slot)| OuterBinding {
                name: self.name(symbol).into(),
                found: OuterBindingSlot {
                    slot,
                    flags: SymbolFlags::Class,
                    import: false,
                    class_name: true,
                },
            });
        let annex_b = self
            .annex_b_context_bindings(scope)
            .map(|(symbol, slot)| OuterBinding {
                name: self.name(symbol).into(),
                found: OuterBindingSlot {
                    slot,
                    flags: SymbolFlags::Function,
                    import: false,
                    class_name: false,
                },
            });
        let info = Rc::new(OuterScope {
            bindings: bindings.chain(inner).chain(annex_b).collect(),
            privates: self.private_entries(scope).into(),
            arguments: self.arguments_slot(scope),
            module: self.module && scope == self.scoping.root_scope_id(),
            home,
            dynamic: self.is_with_scope(scope) || self.is_eval_var_scope(scope),
            parent,
        });
        if before.is_none() {
            self.outer_cache.borrow_mut().insert(scope, info.clone());
        }
        info
    }
}

impl FunctionCompiler<'_> {
    /// The outer chain a stub created here keeps: this function's contexts on top of its own.
    pub fn outer_chain(&self) -> Option<Rc<OuterScope>> {
        let contexts = self.contexts.iter();
        contexts.fold(self.outer.clone(), |parent, &scope| {
            let before = self.parameters_end.filter(|_| scope == self.scope);
            let home = self.eval_home(scope);
            Some(self.analysis.outer_scope(scope, parent, before, home))
        })
    }

    /// The innermost outer context where `find` matches, with its depth from here.
    fn outer_find<T>(&self, find: impl Fn(&OuterScope) -> Option<T>) -> Option<(u32, T)> {
        let mut depth = self.contexts.len() as u32;
        let mut scope = self.outer.as_deref();
        while let Some(current) = scope {
            if let Some(found) = find(current) {
                return Some((depth, found));
            }
            depth += 1;
            scope = current.parent.as_deref();
        }
        None
    }

    /// A name this parse left unresolved, looked up outside a lazy body, with its depth;
    /// `None` inside when a dynamic context comes first.
    fn outer_search(&self, name: &str) -> Option<(u32, Option<OuterRef>)> {
        self.outer_find(|scope| {
            let binding = scope.bindings.iter().find(|b| &*b.name == name);
            let arguments = scope.arguments.filter(|_| name == "arguments");
            let found = binding
                .map(|b| OuterRef::Binding(b.found))
                .or(arguments.map(OuterRef::Arguments));
            found.map(Some).or(scope.dynamic.then_some(None))
        })
    }

    /// A name this parse left unresolved, found statically outside a lazy body.
    pub fn outer_reference(&self, name: &str) -> Option<(u32, OuterRef)> {
        let (depth, found) = self.outer_search(name)?;
        Some((depth, found?))
    }

    /// Whether an unresolved name must be looked up by name (see `OuterScope::dynamic`).
    pub fn outer_is_dynamic(&self, name: &str) -> bool {
        matches!(self.outer_search(name), Some((_, None)))
    }

    /// Only a binding: eager code compiles writes and deletes of a bare `arguments` as
    /// global ones, and so does a lazy body.
    pub fn outer_binding(&self, name: &str) -> Option<(u32, OuterBindingSlot)> {
        match self.outer_reference(name)? {
            (depth, OuterRef::Binding(binding)) => Some((depth, binding)),
            (_, OuterRef::Arguments(_)) => None,
        }
    }

    pub fn outer_private(&self, name: &str) -> Option<(u32, u32)> {
        self.outer_find(|scope| {
            let private = scope.privates.iter().find(|(n, _)| &**n == name);
            private.map(|&(_, slot)| slot)
        })
    }

    /// The home of the innermost outer function containing a direct eval.
    pub fn outer_home(&self) -> Option<EvalHome> {
        self.outer_find(|scope| scope.home).map(|(_, home)| home)
    }

    /// Depth of the module context, for `import.meta` in a lazy body.
    pub fn outer_module(&self) -> Option<u32> {
        self.outer_find(|scope| scope.module.then_some(()))
            .map(|(depth, ())| depth)
    }
}
