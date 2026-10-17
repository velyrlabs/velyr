//! Loads, stores and initialization of named bindings, with TDZ checks.
//!
//! A TDZ check (`ThrowIfHole`) is elided only when the read is in the binding's own
//! function and its declaration was already compiled outside a `switch` (so every path to
//! the read runs the initializer first). Global lexical bindings are checked by the VM.

use oxc_ast::ast::IdentifierReference;
use oxc_semantic::{ReferenceId, ScopeId, SymbolFlags, SymbolId};
use velyr_engine_vm::bytecode::{Instruction as I, Reg};

use super::function::FunctionCompiler;
use super::outer::{OuterBindingSlot, OuterRef};
use super::scope::{Location, is_lexical};
use crate::compiler::{JsResult, throw};

#[derive(Debug, Clone, Copy)]
enum Slot {
    Global,
    Register(Reg),
    Context { depth: u32, slot: u32 },
    Import { depth: u32, slot: u32 },
}

impl FunctionCompiler<'_> {
    fn slot_of(&self, symbol: Option<SymbolId>) -> JsResult<Slot> {
        let Some(symbol) = symbol else {
            return Ok(Slot::Global);
        };
        match self.analysis.location(symbol) {
            Location::Global => Ok(Slot::Global),
            Location::Local => self
                .registers
                .get(&symbol)
                .map(|&register| Slot::Register(register))
                .ok_or_else(|| internal("binding without register")),
            Location::Context { scope, slot } => Ok(Slot::Context {
                depth: self.context_depth(scope)?,
                slot,
            }),
            Location::Import { slot } => Ok(Slot::Import {
                depth: self.context_depth(self.analysis.scoping.root_scope_id())?,
                slot,
            }),
        }
    }

    /// The inner name binding of a class declaration, for a reference made inside its body.
    fn inner_class_slot(
        &self,
        symbol: Option<SymbolId>,
        reference: ReferenceId,
    ) -> JsResult<Option<Slot>> {
        let Some((class, slot)) = symbol.and_then(|symbol| self.analysis.inner_name(symbol)) else {
            return Ok(None);
        };
        let scope = self.analysis.scoping.get_reference(reference).scope_id();
        let mut ancestors = self.analysis.scoping.scope_ancestors(scope);
        if !ancestors.any(|ancestor| ancestor == class) {
            return Ok(None);
        }
        let depth = self.context_depth(class)?;
        Ok(Some(Slot::Context { depth, slot }))
    }

    /// Hops from the current context to `scope`'s.
    pub fn context_depth(&self, scope: ScopeId) -> JsResult<u32> {
        let position = self
            .contexts
            .iter()
            .rposition(|&s| s == scope)
            .ok_or_else(|| internal("binding outside the context chain"))?;
        Ok((self.contexts.len() - 1 - position) as u32)
    }

    /// The function owning the `arguments` object an unresolved `arguments` names, or
    /// `None` at script level (a global lookup).
    pub(super) fn arguments_owner(&self, id: &IdentifierReference) -> Option<ScopeId> {
        if id.name != "arguments" {
            return None;
        }
        let scope = self
            .analysis
            .scoping
            .get_reference(id.reference_id())
            .scope_id();
        let owner = self.analysis.arguments_owner(scope);
        (owner != self.analysis.scoping.root_scope_id()).then_some(owner)
    }

    fn load_slot(&mut self, slot: Slot, name: &str) {
        let instruction = match slot {
            Slot::Global => I::LdaGlobal(self.e.string(name)),
            Slot::Register(register) => I::Ldar(register),
            Slot::Context { depth, slot } => I::LdaContextSlot { depth, slot },
            Slot::Import { depth, slot } => {
                self.e.emit(I::LdaModuleImport { depth, slot });
                I::ThrowIfHole(self.e.string(name))
            }
        };
        self.e.emit(instruction);
    }

    fn store_slot(&mut self, slot: Slot, name: &str) {
        let instruction = match slot {
            Slot::Global => I::StaGlobal(self.e.string(name)),
            Slot::Register(register) => I::Star(register),
            Slot::Context { depth, slot } => I::StaContextSlot { depth, slot },
            // Import bindings are immutable.
            Slot::Import { .. } => I::ThrowConstAssignment(self.e.string(name)),
        };
        self.e.emit(instruction);
    }

    fn needs_tdz_check(&self, symbol: SymbolId) -> bool {
        let has_tdz =
            is_lexical(self.analysis.flags(symbol)) || self.analysis.is_tdz_parameter(symbol);
        if !has_tdz || self.analysis.location(symbol) == Location::Global {
            return false;
        }
        let home = self.analysis.scoping.symbol_scope_id(symbol);
        self.analysis.function_scope(home) != self.scope || !self.initialized.contains(&symbol)
    }

    fn emit_tdz_check(&mut self, name: &str) {
        let index = self.e.string(name);
        self.e.emit(I::ThrowIfHole(index));
    }

    pub fn load_identifier(&mut self, id: &IdentifierReference) -> JsResult<()> {
        let symbol = self.analysis.symbol_of(id.reference_id());
        let name = id.name.as_str();
        if symbol.is_none() {
            if let Some(owner) = self.arguments_owner(id) {
                return self.load_arguments(owner);
            }
            if let Some(found) = self.outer_reference(name) {
                self.load_outer(found, name);
                return Ok(());
            }
            if name == "undefined" {
                self.e.emit(I::LdaUndefined);
                return Ok(());
            }
        }
        if let Some(slot) = self.inner_class_slot(symbol, id.reference_id())? {
            self.load_slot(slot, name);
            self.emit_tdz_check(name);
            return Ok(());
        }
        let slot = self.slot_of(symbol)?;
        self.load_slot(slot, name);
        if let Some(symbol) = symbol
            && self.needs_tdz_check(symbol)
        {
            self.emit_tdz_check(name);
        }
        Ok(())
    }

    /// `typeof name`: a missing global is `undefined` instead of a ReferenceError.
    pub fn load_identifier_for_typeof(&mut self, id: &IdentifierReference) -> JsResult<()> {
        let symbol = self.analysis.symbol_of(id.reference_id());
        let name = id.name.as_str();
        let is_arguments_object = self.arguments_owner(id).is_some();
        let is_outer = self.outer_reference(name).is_some();
        if symbol.is_some() || is_arguments_object || is_outer || name == "undefined" {
            return self.load_identifier(id);
        }
        let index = self.e.string(name);
        self.e.emit(I::LdaGlobalInsideTypeof(index));
        Ok(())
    }

    /// Load of a binding (TDZ-checked: its initializer is outside) or `arguments` object
    /// around a lazy body.
    fn load_outer(&mut self, (depth, found): (u32, OuterRef), name: &str) {
        let binding = match found {
            OuterRef::Arguments(slot) => {
                return self.e.emit(I::LdaContextSlot { depth, slot });
            }
            OuterRef::Binding(binding) => binding,
        };
        self.load_slot(outer_slot(depth, binding), name);
        if is_lexical(binding.flags) {
            self.emit_tdz_check(name);
        }
    }

    /// `store_identifier` on a binding around a lazy body.
    fn store_outer(&mut self, depth: u32, binding: OuterBindingSlot, name: &str) {
        let slot = outer_slot(depth, binding);
        let flags = binding.flags;
        if flags.contains(SymbolFlags::FunctionExpression) {
            if self.is_strict {
                let index = self.e.string(name);
                self.e.emit(I::ThrowConstAssignment(index));
            }
            return;
        }
        if is_lexical(flags) {
            let mark = self.e.mark();
            let value = self.e.alloc();
            self.e.emit(I::Star(value));
            self.load_slot(slot, name);
            self.emit_tdz_check(name);
            self.e.emit(I::Ldar(value));
            self.e.release(mark);
        }
        if flags.contains(SymbolFlags::ConstVariable) || binding.class_name {
            let index = self.e.string(name);
            self.e.emit(I::ThrowConstAssignment(index));
            return;
        }
        self.store_slot(slot, name);
    }

    fn load_arguments(&mut self, owner: ScopeId) -> JsResult<()> {
        if let Some(slot) = self.analysis.arguments_slot(owner) {
            let depth = self.context_depth(owner)?;
            self.e.emit(I::LdaContextSlot { depth, slot });
            return Ok(());
        }
        let register = self
            .arguments
            .ok_or_else(|| internal("arguments object missing"))?;
        self.e.emit(I::Ldar(register));
        Ok(())
    }

    /// PutValue of `acc` (preserved) on a name.
    pub fn store_identifier(&mut self, id: &IdentifierReference) -> JsResult<()> {
        let symbol = self.analysis.symbol_of(id.reference_id());
        let name = id.name.as_str();
        if symbol.is_none()
            && let Some((depth, binding)) = self.outer_binding(name)
        {
            self.store_outer(depth, binding, name);
            return Ok(());
        }
        if let Some(slot) = self.inner_class_slot(symbol, id.reference_id())? {
            // TDZ first, then the immutable binding rejects the write.
            let mark = self.e.mark();
            let value = self.e.alloc();
            self.e.emit(I::Star(value));
            self.load_slot(slot, name);
            self.emit_tdz_check(name);
            self.e.emit(I::Ldar(value));
            self.e.release(mark);
            let index = self.e.string(name);
            self.e.emit(I::ThrowConstAssignment(index));
            return Ok(());
        }
        let slot = self.slot_of(symbol)?;
        if let Some(symbol) = symbol
            && self.analysis.location(symbol) != Location::Global
        {
            let flags = self.analysis.flags(symbol);
            if flags.contains(SymbolFlags::FunctionExpression) {
                // The name binding of a function expression is immutable; sloppy code ignores
                // the write.
                if self.is_strict {
                    let index = self.e.string(name);
                    self.e.emit(I::ThrowConstAssignment(index));
                }
                return Ok(());
            }
            if self.needs_tdz_check(symbol) {
                let mark = self.e.mark();
                let value = self.e.alloc();
                self.e.emit(I::Star(value));
                self.load_slot(slot, name);
                self.emit_tdz_check(name);
                self.e.emit(I::Ldar(value));
                self.e.release(mark);
            }
            // A class expression's own name lives in its class scope and is immutable.
            let is_class_name = flags.contains(SymbolFlags::Class)
                && self
                    .classes
                    .contains(&self.analysis.scoping.symbol_scope_id(symbol));
            if flags.contains(SymbolFlags::ConstVariable) || is_class_name {
                let index = self.e.string(name);
                self.e.emit(I::ThrowConstAssignment(index));
                return Ok(());
            }
        }
        self.store_slot(slot, name);
        Ok(())
    }

    /// Puts a binding in the TDZ.
    pub fn put_in_tdz(&mut self, symbol: SymbolId) -> JsResult<()> {
        let slot = self.slot_of(Some(symbol))?;
        self.e.emit(I::LdaTheHole);
        self.store_slot(slot, self.analysis.name(symbol));
        Ok(())
    }

    /// InitializeBinding (or PutValue for `var`) of `acc` on a declared binding.
    pub fn initialize_binding(&mut self, symbol: SymbolId) -> JsResult<()> {
        let name = self.analysis.name(symbol);
        let slot = self.slot_of(Some(symbol))?;
        let flags = self.analysis.flags(symbol);
        if matches!(slot, Slot::Global) && is_lexical(flags) {
            let index = self.e.string(name);
            self.e.emit(I::InitializeGlobalLexical(index));
        } else {
            self.store_slot(slot, name);
        }
        let home = self.analysis.scoping.symbol_scope_id(symbol);
        if !self.switch_scopes.contains(&home) {
            self.initialized.insert(symbol);
        }
        Ok(())
    }
}

fn outer_slot(depth: u32, binding: OuterBindingSlot) -> Slot {
    let slot = binding.slot;
    match binding.import {
        true => Slot::Import { depth, slot },
        false => Slot::Context { depth, slot },
    }
}

fn internal(what: &str) -> velyr_engine_api::JsException {
    throw("SyntaxError", format!("internal compiler error: {what}"))
}
