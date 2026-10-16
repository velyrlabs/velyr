//! Module namespace exotic objects (ECMA-262 §10.4.6), built as a proxy whose native
//! handler reads the live bindings. The target carries the exports as non-configurable,
//! writable data properties plus `@@toStringTag`, and is non-extensible, so the proxy
//! invariants hold and the traps that forward to it (prototype, extensibility, keys)
//! already give the namespace answers.

use std::rc::Rc;

use super::{Interpreter, JsResult, Resolution};
use crate::interpreter::builtins::symbol::SYMBOL_TO_STRING_TAG;
use crate::interpreter::builtins::{ToPropertyDescriptor, arg};
use crate::interpreter::function::NativeFn;
use crate::interpreter::ops::is_hole;
use crate::interpreter::{ErrorKind, object_value};
use crate::{JsStr, ObjectKey, ObjectRef, PropertyDescriptor, Value, same_value};

#[derive(Clone, Copy)]
enum Trap {
    Get,
    Has,
    GetOwnPropertyDescriptor,
    DefineProperty,
    Set,
    DeleteProperty,
}

const TRAPS: [(&str, Trap); 6] = [
    ("get", Trap::Get),
    ("has", Trap::Has),
    ("getOwnPropertyDescriptor", Trap::GetOwnPropertyDescriptor),
    ("defineProperty", Trap::DefineProperty),
    ("set", Trap::Set),
    ("deleteProperty", Trap::DeleteProperty),
];

type Exports = Rc<Vec<(JsStr, Resolution)>>;

impl Interpreter {
    /// GetModuleNamespace(module), rooted in the module record.
    pub(super) fn namespace_of(&mut self, module: usize) -> JsResult<Value> {
        let existing = self.state.modules[module].namespace.clone();
        if !matches!(existing, Value::Undefined) {
            return Ok(existing);
        }
        let mut exports: Vec<(JsStr, Resolution)> = self
            .exported_names(module, &mut Vec::new())
            .into_iter()
            .map(|name| {
                let resolution = self.resolve_export(module, &name, &mut Vec::new());
                (name, resolution)
            })
            .filter(|(_, r)| matches!(r, Resolution::Binding { .. } | Resolution::Namespace(_)))
            .collect();
        exports.sort_by_key(|(name, _)| name.to_utf16());
        let exports: Exports = Rc::new(exports);
        let target = self.new_object_with(None)?;
        for (name, _) in exports.iter() {
            let key = self.string_key(name);
            let desc = PropertyDescriptor::data(Value::Undefined, true, true, false);
            self.define_property(target, key, &desc)?;
        }
        let tag = PropertyDescriptor::data(Value::str("Module"), false, false, false);
        self.define_property(target, ObjectKey::Symbol(SYMBOL_TO_STRING_TAG), &tag)?;
        self.prevent_extensions(target)?;
        let handler = self.new_object_with(None)?;
        for (name, trap) in TRAPS {
            let exports = exports.clone();
            let call: NativeFn =
                Rc::new(move |vm, _, args| vm.namespace_trap(trap, &exports, args));
            let function = object_value(self.create_native_function(name, 3, call, false)?);
            self.keep(&function);
            let key = self.realm.key_str(name);
            let desc = PropertyDescriptor::data(function, true, true, true);
            self.define_property(handler, key, &desc)?;
        }
        let (proxy, _) = self.proxy_create(&object_value(target), &object_value(handler))?;
        let namespace = object_value(proxy);
        self.state.modules[module].namespace = namespace.clone();
        Ok(namespace)
    }

    fn namespace_trap(&mut self, trap: Trap, exports: &Exports, args: &[Value]) -> JsResult<Value> {
        let Some(target) = self.as_object(&arg(args, 0)) else {
            return Err(self.internal("namespace trap without its target"));
        };
        let key = arg(args, 1);
        let Value::String(name) = &key else {
            let key = self.object_key(&key)?;
            return self.namespace_symbol_trap(trap, target, key, args);
        };
        let export = exports.iter().find(|(export, _)| export == name);
        let found = export.is_some();
        Ok(match (trap, export) {
            (Trap::Get, Some((name, resolution))) => self.binding_value(name, *resolution)?,
            (Trap::GetOwnPropertyDescriptor, Some((name, resolution))) => {
                let value = self.binding_value(name, *resolution)?;
                self.descriptor_object(&PropertyDescriptor::data(value, true, true, false))?
            }
            (Trap::DefineProperty, Some((name, resolution))) => {
                let desc = arg(args, 2).to_descriptor(self)?;
                let current = self.binding_value(name, *resolution)?;
                let compatible = desc.configurable != Some(true)
                    && desc.enumerable != Some(false)
                    && desc.get.is_none()
                    && desc.set.is_none()
                    && desc.writable != Some(false)
                    && desc.value.is_none_or(|value| same_value(&value, &current));
                Value::Bool(compatible)
            }
            (Trap::Get | Trap::GetOwnPropertyDescriptor, None) => Value::Undefined,
            (Trap::DefineProperty | Trap::Set, _) => Value::Bool(false),
            (Trap::Has, _) => Value::Bool(found),
            (Trap::DeleteProperty, _) => Value::Bool(!found),
        })
    }

    /// Symbol keys: the ordinary behavior of the target (only `@@toStringTag` exists).
    fn namespace_symbol_trap(
        &mut self,
        trap: Trap,
        target: ObjectRef,
        key: ObjectKey,
        args: &[Value],
    ) -> JsResult<Value> {
        Ok(match trap {
            Trap::Get => self.get(target, key, &object_value(target))?,
            Trap::Has => Value::Bool(self.has_property(target, key)?),
            Trap::GetOwnPropertyDescriptor if key == ObjectKey::Symbol(SYMBOL_TO_STRING_TAG) => {
                let desc = PropertyDescriptor::data(Value::str("Module"), false, false, false);
                self.descriptor_object(&desc)?
            }
            Trap::GetOwnPropertyDescriptor => Value::Undefined,
            Trap::DefineProperty => {
                let desc = arg(args, 2).to_descriptor(self)?;
                Value::Bool(self.define_own(target, key, &desc)?)
            }
            Trap::Set => Value::Bool(false),
            Trap::DeleteProperty => Value::Bool(self.delete_key(target, key)?),
        })
    }

    /// The current value of a resolved export; ReferenceError while it is in its TDZ.
    fn binding_value(&mut self, name: &JsStr, resolution: Resolution) -> JsResult<Value> {
        match resolution {
            Resolution::Binding { module, slot } => {
                let value = self.module_context(module).slots[slot as usize].clone();
                if is_hole(&value) {
                    let message = format!("{} is not initialized", name.to_string_lossy());
                    return Err(self.error(ErrorKind::Reference, &message));
                }
                Ok(value)
            }
            Resolution::Namespace(module) => self.namespace_of(module),
            Resolution::Ambiguous | Resolution::NotFound => Ok(Value::Undefined),
        }
    }
}
