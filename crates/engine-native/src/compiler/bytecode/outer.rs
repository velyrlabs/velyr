//! Outer scopes of a lazy body (VELYR-201), like V8's ScopeInfo: its first call parses only
//! the function's own text, so whatever that text names from around it comes from here.
//! One entry per context on the chain at the stub (one hop each), shared by every stub
//! below it, with the bindings captured there, its private names and `arguments` slot.

use std::rc::Rc;

use oxc_semantic::{ScopeId, SymbolFlags};

use super::function::FunctionCompiler;
use super::scope::{Location, ScopeAnalysis};

pub struct OuterScope {
    bindings: Box<[OuterBinding]>,
    privates: Box<[(Box<str>, u32)]>,
    /// Slot of the `arguments` object of the function owning this context.
    arguments: Option<u32>,
    /// The module context (`import.meta`).
    module: bool,
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
    ) -> Rc<OuterScope> {
        if before.is_none()
            && let Some(cached) = self.outer_cache.borrow().get(&scope)
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
            Some(self.analysis.outer_scope(scope, parent, before))
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

    /// A name this parse left unresolved, looked up outside a lazy body, with its depth.
    pub fn outer_reference(&self, name: &str) -> Option<(u32, OuterRef)> {
        self.outer_find(|scope| {
            let binding = scope.bindings.iter().find(|b| &*b.name == name);
            let arguments = scope.arguments.filter(|_| name == "arguments");
            binding
                .map(|b| OuterRef::Binding(b.found))
                .or(arguments.map(OuterRef::Arguments))
        })
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

    /// Depth of the module context, for `import.meta` in a lazy body.
    pub fn outer_module(&self) -> Option<u32> {
        self.outer_find(|scope| scope.module.then_some(()))
            .map(|(depth, ())| depth)
    }
}
