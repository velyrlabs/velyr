//! Dynamic name lookup (9.1.2.1 GetIdentifierReference) over contexts that carry a names
//! table and an optional object: `with` (9.1.1.2) and sloppy direct-eval `var` injection
//! (19.2.1.3).

use std::rc::Rc;

use velyr_engine_gc::{Gc, Trace, Tracer};

use super::builtins::symbol::SYMBOL_UNSCOPABLES;
use super::environment::Context;
use super::ops::{HOLE, is_hole};
use super::state::CodeInfo;
use super::{ErrorKind, Interpreter, JsResult, object_value};
use crate::bytecode::{Constant, GlobalDeclarations, Instruction as I};
use crate::{JsStr, ObjectKey, ObjectRef, Value, to_boolean};

/// The object part of a context record.
#[derive(Clone, Copy)]
pub(crate) enum Extension {
    /// Object Environment Record of a `with` statement (`withEnvironment` true).
    With(ObjectRef),
    /// Bindings that sloppy direct eval added to a var scope, created on demand.
    Vars(ObjectRef),
}

impl Trace for Extension {
    fn trace(&self, tracer: &mut Tracer) {
        match self {
            Extension::With(object) | Extension::Vars(object) => object.trace(tracer),
        }
    }
}

/// Where a dynamically looked-up name lives.
#[derive(Debug)]
pub(crate) enum Reference {
    /// Slot `slot` of the context cell `context`.
    Slot {
        context: Value,
        slot: u32,
    },
    Object {
        object: ObjectRef,
        with: bool,
    },
    /// A script-level lexical or a property of the global object.
    Global,
    Unresolvable,
}

impl Interpreter {
    /// Allocates a context cell; `CreateContext`, `CreateContextWithNames` and `PushWithContext` all go through it.
    pub(crate) fn new_context(
        &mut self,
        slots: Vec<Value>,
        parent: Value,
        names: Option<Rc<[(JsStr, bool)]>>,
        extension: Option<Extension>,
        is_var_scope: bool,
    ) -> JsResult<Value> {
        let context = Context {
            slots,
            parent,
            names,
            extension,
            is_var_scope,
        };
        Ok(Value::Object(self.alloc_cell(context)?))
    }

    pub(crate) fn step_lookup(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<()> {
        match instruction {
            I::CreateContextWithNames {
                count,
                names,
                var_scope,
            } => {
                let Constant::ContextNames(names) = &info.code.constants[names.0 as usize] else {
                    return Err(self.internal("CreateContextWithNames expects context names"));
                };
                let (names, parent) = (Some(names.clone()), self.top().context.clone());
                let slots = vec![HOLE; *count as usize];
                self.state.acc = self.new_context(slots, parent, names, None, *var_scope)?;
            }
            I::PushWithContext(r) => {
                let object = self.coerce_object(&self.reg(*r).clone())?;
                let parent = self.top().context.clone();
                let with = Some(Extension::With(object));
                self.state.acc = self.new_context(Vec::new(), parent, None, with, false)?;
            }
            I::LdaLookup(name) | I::LdaLookupForTypeof(name) => {
                let name = self.constant_str(info, *name);
                let for_typeof = matches!(instruction, I::LdaLookupForTypeof(_));
                let reference = self.resolve_frame_binding(&name)?;
                self.state.acc = self.read_reference(reference, &name, for_typeof)?;
            }
            I::LdaLookupCallee { name, receiver } => {
                let name = self.constant_str(info, *name);
                let reference = self.resolve_frame_binding(&name)?;
                let this = match reference {
                    Reference::Object { object, with: true } => object_value(object),
                    _ => Value::Undefined,
                };
                self.state.acc = self.read_reference(reference, &name, false)?;
                self.set_reg(*receiver, this);
            }
            I::StaLookup { name, strict } => {
                let name = self.constant_str(info, *name);
                let reference = self.resolve_frame_binding(&name)?;
                let value = self.state.acc.clone();
                self.write_reference(reference, &name, value, *strict)?;
            }
            I::DeleteLookup(name) => {
                let name = self.constant_str(info, *name);
                let deleted = match self.resolve_frame_binding(&name)? {
                    Reference::Slot { .. } => false,
                    Reference::Object { object, .. } => {
                        let key = self.string_key(&name);
                        self.delete_key(object, key)?
                    }
                    Reference::Global => self.delete_global(&name)?,
                    Reference::Unresolvable => true,
                };
                self.state.acc = Value::Bool(deleted);
            }
            I::DeclareEvalVar { name, function } => {
                let name = self.constant_str(info, *name);
                let context = self.top().context.clone();
                self.declare_eval_var(&context, &name, *function)?;
            }
            _ => return Err(self.internal("not a lookup instruction")),
        }
        Ok(())
    }

    fn resolve_frame_binding(&mut self, name: &JsStr) -> JsResult<Reference> {
        let context = self.top().context.clone();
        self.resolve_binding(&context, name)
    }

    /// GetIdentifierReference from `context` outwards, then the global environment.
    pub(crate) fn resolve_binding(&mut self, context: &Value, name: &JsStr) -> JsResult<Reference> {
        let key = self.string_key(name);
        let mut current = context.clone();
        while matches!(current, Value::Object(_)) {
            let cell = self.context_cell(&current);
            let (extension, slot, parent) =
                (cell.extension, slot_of(cell, name), cell.parent.clone());
            match (extension, slot) {
                (Some(Extension::With(object)), _) => {
                    if self.has_property(object, key)? && !self.is_unscopable(object, key)? {
                        return Ok(Reference::Object { object, with: true });
                    }
                }
                (_, Some((slot, _))) => {
                    return Ok(Reference::Slot {
                        context: current,
                        slot,
                    });
                }
                (Some(Extension::Vars(object)), None) => {
                    if self.own_property(object, key)?.is_some() {
                        return Ok(Reference::Object {
                            object,
                            with: false,
                        });
                    }
                }
                (None, None) => {}
            }
            current = parent;
        }
        let global = self.state.intrinsics.global;
        if self.state.lexicals.contains_key(name) || self.has_property(global, key)? {
            return Ok(Reference::Global);
        }
        Ok(Reference::Unresolvable)
    }

    /// 9.1.1.2.1 steps 5-7: a truthy `object[@@unscopables][key]` hides the binding.
    fn is_unscopable(&mut self, object: ObjectRef, key: ObjectKey) -> JsResult<bool> {
        let receiver = object_value(object);
        let unscopables = self.get(object, ObjectKey::Symbol(SYMBOL_UNSCOPABLES), &receiver)?;
        let Value::Object(blocked) = unscopables else {
            return Ok(false);
        };
        let blocked = ObjectRef::from_raw(blocked);
        Ok(to_boolean(&self.get(
            blocked,
            key,
            &object_value(blocked),
        )?))
    }

    fn read_reference(
        &mut self,
        reference: Reference,
        name: &JsStr,
        for_typeof: bool,
    ) -> JsResult<Value> {
        match reference {
            Reference::Slot { context, slot } => {
                let value = self.context_cell(&context).slots[slot as usize].clone();
                match is_hole(&value) {
                    true => Err(self.not_initialized(name)),
                    false => Ok(value),
                }
            }
            Reference::Object { object, .. } => {
                let key = self.string_key(name);
                self.get(object, key, &object_value(object))
            }
            Reference::Global => self.load_global_name(name, for_typeof),
            Reference::Unresolvable if for_typeof => Ok(Value::Undefined),
            Reference::Unresolvable => Err(self.not_defined(name)),
        }
    }

    /// shortcut: `names` carries no `const` flag, so a dynamic store to a `const` slot is not
    /// rejected here; add the flag when the compiler emits dynamic stores into `const` scopes.
    fn write_reference(
        &mut self,
        reference: Reference,
        name: &JsStr,
        value: Value,
        is_strict: bool,
    ) -> JsResult<()> {
        let object = match reference {
            Reference::Slot { context, slot } => {
                let Value::Object(cell) = context else {
                    return Err(self.internal("slot reference without a context"));
                };
                let cell = self.realm.heap.get_mut(Gc::<Context>::from_raw(cell));
                let slot = &mut cell.expect("context is live").slots[slot as usize];
                if is_hole(slot) {
                    return Err(self.not_initialized(name));
                }
                *slot = value;
                return Ok(());
            }
            Reference::Global => return self.store_global_name(name, value, is_strict),
            Reference::Unresolvable if is_strict => return Err(self.not_defined(name)),
            Reference::Unresolvable => self.state.intrinsics.global,
            Reference::Object { object, .. } => object,
        };
        let key = self.string_key(name);
        let done = self.set(object, key, value, &object_value(object))?;
        if !done && is_strict {
            let message = format!(
                "Cannot assign to read only property '{}'",
                name.to_string_lossy()
            );
            return Err(self.error(ErrorKind::Type, &message));
        }
        Ok(())
    }

    /// 19.2.1.3 steps 3 and 10-11 for one name: a lexical crossed on the way to the var scope
    /// is a SyntaxError; the var scope gets the binding unless it already has one.
    pub(crate) fn declare_eval_var(
        &mut self,
        context: &Value,
        name: &JsStr,
        function: bool,
    ) -> JsResult<()> {
        let mut current = context.clone();
        while matches!(current, Value::Object(_)) {
            let cell = self.context_cell(&current);
            let (slot, is_var_scope, parent) =
                (slot_of(cell, name), cell.is_var_scope, cell.parent.clone());
            match slot {
                Some((_, true)) => return Err(self.already_declared(name)),
                Some((_, false)) if is_var_scope => return Ok(()),
                _ if is_var_scope => return self.define_eval_var(&current, name),
                _ => current = parent,
            }
        }
        let (functions, vars) = match function {
            true => (vec![name.clone()], Vec::new()),
            false => (Vec::new(), vec![name.clone()]),
        };
        let declarations = GlobalDeclarations {
            functions,
            vars,
            configurable: true,
            ..GlobalDeclarations::default()
        };
        self.declare_globals(&declarations)
    }

    /// A deletable `undefined` binding on the var scope's `Vars` object, unless already there.
    fn define_eval_var(&mut self, context: &Value, name: &JsStr) -> JsResult<()> {
        let object = match self.context_cell(context).extension {
            Some(Extension::Vars(object)) => object,
            Some(Extension::With(_)) => {
                return Err(self.internal("a with context is not a var scope"));
            }
            None => {
                let object = self.new_object_with(None)?;
                let Value::Object(cell) = context else {
                    return Err(self.internal("var scope without a context"));
                };
                let cell = self.realm.heap.get_mut(Gc::<Context>::from_raw(*cell));
                cell.expect("context is live").extension = Some(Extension::Vars(object));
                object
            }
        };
        let key = self.string_key(name);
        if self.own_property(object, key)?.is_none() {
            self.create_data_property(object, key, Value::Undefined)?;
        }
        Ok(())
    }
}

/// The slot index and lexical flag of `name` in `context`'s names table.
fn slot_of(context: &Context, name: &JsStr) -> Option<(u32, bool)> {
    let names = context.names.as_deref()?;
    let index = names.iter().position(|(entry, _)| entry == name)?;
    Some((index as u32, names[index].1))
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::interpreter::builtins::symbol::SYMBOL_UNSCOPABLES;
    use crate::interpreter::ops::HOLE;
    use crate::interpreter::{Abrupt, Interpreter};
    use crate::{JsStr, ObjectKey, ObjectRef, Value};

    fn s(text: &str) -> JsStr {
        JsStr::from_str(text)
    }
    fn names(list: &[(&str, bool)]) -> Option<Rc<[(JsStr, bool)]>> {
        Some(
            list.iter()
                .map(|&(name, lexical)| (s(name), lexical))
                .collect(),
        )
    }
    fn put(vm: &mut Interpreter, object: ObjectRef, key: ObjectKey, value: Value) {
        assert!(vm.create_data_property(object, key, value).unwrap());
    }
    /// `with ({x: 1, y: 2, [Symbol.unscopables]: {y: true}})` above a declarative context `y` = 3.
    fn with_chain(vm: &mut Interpreter) -> Value {
        let declarative = vm
            .new_context(
                vec![Value::Number(3.0)],
                Value::Undefined,
                names(&[("y", true)]),
                None,
                false,
            )
            .unwrap();
        let object = vm.new_object().unwrap();
        for (name, value) in [("x", 1.0), ("y", 2.0)] {
            let key = vm.realm.key_str(name);
            put(vm, object, key, Value::Number(value));
        }
        let blocked = vm.new_object().unwrap();
        let key = vm.realm.key_str("y");
        put(vm, blocked, key, Value::Bool(true));
        put(
            vm,
            object,
            ObjectKey::Symbol(SYMBOL_UNSCOPABLES),
            Value::Object(blocked.raw()),
        );
        vm.new_context(
            Vec::new(),
            declarative,
            None,
            Some(Extension::With(object)),
            false,
        )
        .unwrap()
    }
    #[test]
    fn with_context_honours_unscopables() {
        let mut vm = Interpreter::new();
        let chain = with_chain(&mut vm);
        assert!(matches!(
            vm.resolve_binding(&chain, &s("x")).unwrap(),
            Reference::Object { with: true, .. }
        ));
        assert!(matches!(
            vm.resolve_binding(&chain, &s("y")).unwrap(),
            Reference::Slot { slot: 0, .. }
        ));
        assert!(matches!(
            vm.resolve_binding(&chain, &s("nope")).unwrap(),
            Reference::Unresolvable
        ));
    }
    #[test]
    fn eval_var_over_a_lexical_is_a_syntax_error() {
        let mut vm = Interpreter::new();
        let var_scope = vm
            .new_context(
                vec![HOLE],
                Value::Undefined,
                names(&[("v", true)]),
                None,
                true,
            )
            .unwrap();
        let block = vm
            .new_context(Vec::new(), var_scope, names(&[]), None, false)
            .unwrap();
        let Err(Abrupt::Throw(error)) = vm.declare_eval_var(&block, &s("v"), false) else {
            panic!("no SyntaxError")
        };
        assert!(vm.exception_message(&error).starts_with("SyntaxError"));
        assert!(vm.declare_eval_var(&block, &s("w"), false).is_ok());
    }
}
