//! Context chain (captured bindings) and the global environment: script-level
//! `let`/`const` in a declarative record, `var`/functions on the global object.

use std::rc::Rc;

use velyr_engine_gc::{Gc, Trace, Tracer};

use super::lookup::Extension;
use super::ops::{HOLE, is_hole};
use super::state::{CodeInfo, Lexical};
use super::{ErrorKind, Interpreter, JsResult, object_value};
use crate::bytecode::{ConstIndex, Constant, GlobalDeclarations, Instruction as I};
use crate::{InternalMethods, JsStr, PropertyDescriptor, PropertyValue, Value};

/// Heap cell of one scope's captured bindings.
pub(crate) struct Context {
    pub(crate) slots: Vec<Value>,
    /// Enclosing context, or `Undefined`.
    pub(crate) parent: Value,
    /// Slot `i` binds `names[i]` (`true` = lexical) for dynamic lookup; shared with the code constant.
    pub(crate) names: Option<Rc<[(JsStr, bool)]>>,
    pub(crate) extension: Option<Extension>,
    /// Target of sloppy direct-eval `var` declarations (a function or eval var scope).
    pub(crate) is_var_scope: bool,
}

impl Trace for Context {
    fn trace(&self, tracer: &mut Tracer) {
        self.slots.trace(tracer);
        self.parent.trace(tracer);
        self.extension.trace(tracer);
    }

    /// The slots never change size once the context is created; `names` belongs to the code.
    fn owned_bytes(&self) -> usize {
        self.slots.capacity() * size_of::<Value>()
    }
}

impl Interpreter {
    pub(crate) fn step_environment(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<()> {
        match instruction {
            I::CreateContext(count) => {
                let parent = self.top().context.clone();
                self.state.acc =
                    self.new_context(vec![HOLE; *count as usize], parent, None, None, false)?;
            }
            I::PushContext(r) => {
                let next = self.state.acc.clone();
                let current = std::mem::replace(&mut self.top_mut().context, next);
                self.set_reg(*r, current);
            }
            I::PopContext(r) => self.top_mut().context = self.reg(*r).clone(),
            I::SaveContext(r) => self.set_reg(*r, self.top().context.clone()),
            I::LdaContextSlot { depth, slot } => {
                self.state.acc = self.context_at(*depth).slots[*slot as usize].clone();
            }
            I::StaContextSlot { depth, slot } => {
                let value = self.state.acc.clone();
                self.context_at_mut(*depth).slots[*slot as usize] = value;
            }
            I::DeclareGlobals(c) => match &info.code.constants[c.0 as usize] {
                Constant::GlobalDeclarations(declarations) => self.declare_globals(declarations)?,
                _ => return Err(self.internal("DeclareGlobals expects declarations")),
            },
            I::LdaGlobal(name) => self.state.acc = self.load_global(info, *name, false)?,
            I::LdaGlobalInsideTypeof(name) => {
                self.state.acc = self.load_global(info, *name, true)?
            }
            I::StaGlobal(name) => self.store_global(info, *name)?,
            I::InitializeGlobalLexical(name) => {
                let name = self.constant_str(info, *name);
                let value = self.state.acc.clone();
                match self.state.lexicals.get_mut(&name) {
                    Some(lexical) => lexical.value = value,
                    None => return Err(self.internal("lexical global was not declared")),
                }
            }
            I::InitializeAnnexBGlobal(name) => {
                let name = self.constant_str(info, *name);
                if !self.state.lexicals.contains_key(&name) {
                    let global = self.state.intrinsics.global;
                    let key = self.string_key(&name);
                    let value = self.state.acc.clone();
                    self.set(global, key, value, &object_value(global))?;
                }
            }
            I::DeleteGlobal(name) => {
                let name = self.constant_str(info, *name);
                self.state.acc = Value::Bool(self.delete_global(&name)?);
            }
            other => return self.step_property(info, other),
        }
        Ok(())
    }

    fn context_ref(&self, depth: u32) -> Gc<Context> {
        let mut context = self.top().context.clone();
        for _ in 0..depth {
            context = self.context_cell(&context).parent.clone();
        }
        match context {
            Value::Object(r) => Gc::from_raw(r),
            _ => panic!("context depth {depth} is past the end of the chain"),
        }
    }

    pub(super) fn context_cell(&self, context: &Value) -> &Context {
        let Value::Object(r) = context else {
            panic!("no context is active");
        };
        self.realm
            .heap
            .get(Gc::from_raw(*r))
            .expect("context is live")
    }

    pub(crate) fn context_at(&self, depth: u32) -> &Context {
        self.realm
            .heap
            .get(self.context_ref(depth))
            .expect("context is live")
    }

    fn context_at_mut(&mut self, depth: u32) -> &mut Context {
        let context = self.context_ref(depth);
        self.realm.heap.get_mut(context).expect("context is live")
    }

    /// GlobalDeclarationInstantiation: every conflict throws before any binding exists.
    pub(super) fn declare_globals(&mut self, declarations: &GlobalDeclarations) -> JsResult<()> {
        let global = self.state.intrinsics.global;
        let lexical_names = declarations.lets.iter().chain(&declarations.consts);
        for name in lexical_names.clone() {
            let key = self.string_key(name);
            let own = self
                .realm
                .get_own_property(global, key)
                .map_err(|e| self.object_error(e))?;
            let is_restricted = own.is_some_and(|p| !p.configurable);
            if self.state.lexicals.contains_key(name) || is_restricted {
                return Err(self.already_declared(name));
            }
        }
        for name in declarations.vars.iter().chain(&declarations.functions) {
            if self.state.lexicals.contains_key(name) {
                return Err(self.already_declared(name));
            }
        }
        self.check_global_var_declarations(declarations)?;
        let configurable = declarations.configurable;
        for name in declarations.vars.iter().chain(&declarations.functions) {
            let key = self.string_key(name);
            let own = self
                .realm
                .get_own_property(global, key)
                .map_err(|e| self.object_error(e))?;
            if own.is_none() {
                let desc = PropertyDescriptor::data(Value::Undefined, true, true, configurable);
                self.define_property(global, key, &desc)?;
            }
        }
        let extensible = self.is_extensible(global)?;
        for name in &declarations.annex_b {
            let key = self.string_key(name);
            let own = self.own_property(global, key)?;
            if own.is_none() && extensible && !self.state.lexicals.contains_key(name) {
                let desc = PropertyDescriptor::data(Value::Undefined, true, true, configurable);
                self.define_property(global, key, &desc)?;
            }
        }
        for (name, is_const) in declarations
            .lets
            .iter()
            .map(|name| (name, false))
            .chain(declarations.consts.iter().map(|name| (name, true)))
        {
            let lexical = Lexical {
                value: HOLE,
                is_const,
            };
            self.state.lexicals.insert(name.clone(), lexical);
        }
        Ok(())
    }

    /// CanDeclareGlobalFunction / CanDeclareGlobalVar for every var-scoped name.
    fn check_global_var_declarations(&mut self, declarations: &GlobalDeclarations) -> JsResult<()> {
        let global = self.state.intrinsics.global;
        let extensible = self.is_extensible(global)?;
        let functions = declarations.functions.iter().map(|name| (name, true));
        for (name, is_function) in functions.chain(declarations.vars.iter().map(|n| (n, false))) {
            let key = self.string_key(name);
            let own = self.own_property(global, key)?;
            let allowed = match own {
                None => extensible,
                Some(property) if is_function => {
                    property.configurable
                        || property.enumerable
                            && matches!(property.value, PropertyValue::Data { writable: true, .. })
                }
                Some(_) => true,
            };
            if !allowed {
                let kind = if is_function { "function" } else { "variable" };
                let message = format!("Cannot declare global {kind} '{}'", name.to_string_lossy());
                return Err(self.error(ErrorKind::Type, &message));
            }
        }
        Ok(())
    }

    pub(super) fn already_declared(&mut self, name: &JsStr) -> super::Abrupt {
        let message = format!(
            "Identifier '{}' has already been declared",
            name.to_string_lossy()
        );
        self.error(ErrorKind::Syntax, &message)
    }

    pub(super) fn delete_global(&mut self, name: &JsStr) -> JsResult<bool> {
        if self.state.lexicals.contains_key(name) {
            return Ok(false);
        }
        let key = self.string_key(name);
        let global = self.state.intrinsics.global;
        self.realm
            .delete(global, key)
            .map_err(|e| self.object_error(e))
    }

    fn load_global(
        &mut self,
        info: &CodeInfo,
        name: ConstIndex,
        inside_typeof: bool,
    ) -> JsResult<Value> {
        let name = self.constant_str(info, name);
        self.load_global_name(&name, inside_typeof)
    }

    pub(super) fn load_global_name(
        &mut self,
        name: &JsStr,
        inside_typeof: bool,
    ) -> JsResult<Value> {
        if let Some(lexical) = self.state.lexicals.get(name) {
            let value = lexical.value.clone();
            if is_hole(&value) {
                return Err(self.not_initialized(name));
            }
            return Ok(value);
        }
        let global = self.state.intrinsics.global;
        let key = self.string_key(name);
        if self
            .realm
            .has_property(global, key)
            .map_err(|e| self.object_error(e))?
        {
            return self.get(global, key, &object_value(global));
        }
        if inside_typeof {
            return Ok(Value::Undefined);
        }
        Err(self.not_defined(name))
    }

    fn store_global(&mut self, info: &CodeInfo, name: ConstIndex) -> JsResult<()> {
        let name = self.constant_str(info, name);
        let value = self.state.acc.clone();
        let is_strict = self.top().info.code.is_strict;
        self.store_global_name(&name, value, is_strict)
    }

    pub(super) fn store_global_name(
        &mut self,
        name: &JsStr,
        value: Value,
        is_strict: bool,
    ) -> JsResult<()> {
        if let Some(lexical) = self.state.lexicals.get_mut(name) {
            if is_hole(&lexical.value) {
                return Err(self.not_initialized(name));
            }
            if lexical.is_const {
                let message = format!(
                    "Assignment to constant variable '{}'",
                    name.to_string_lossy()
                );
                return Err(self.error(ErrorKind::Type, &message));
            }
            lexical.value = value;
            return Ok(());
        }
        let global = self.state.intrinsics.global;
        let key = self.string_key(name);
        let exists = self
            .realm
            .has_property(global, key)
            .map_err(|e| self.object_error(e))?;
        if !exists && is_strict {
            return Err(self.not_defined(name));
        }
        let done = self.set(global, key, value, &object_value(global))?;
        if !done && is_strict {
            let message = format!(
                "Cannot assign to read only property '{}'",
                name.to_string_lossy()
            );
            return Err(self.error(ErrorKind::Type, &message));
        }
        Ok(())
    }

    pub(super) fn not_initialized(&mut self, name: &JsStr) -> super::Abrupt {
        let message = format!("{} is not initialized", name.to_string_lossy());
        self.error(ErrorKind::Reference, &message)
    }

    pub(super) fn not_defined(&mut self, name: &JsStr) -> super::Abrupt {
        let message = format!("{} is not defined", name.to_string_lossy());
        self.error(ErrorKind::Reference, &message)
    }
}
