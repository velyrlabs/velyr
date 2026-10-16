//! Class definition, class elements, private names and `super`.
//!
//! Private names are symbols from a range JS cannot otherwise obtain. Their properties are
//! own-only (no prototype walk, no proxies), non-enumerable, and every access checks the
//! brand: a missing private property is a `TypeError`.
//! shortcut: private properties live in the ordinary property table, so a future
//! `Object.getOwnPropertySymbols`/`Reflect.ownKeys` must filter `is_private` keys.

use std::rc::Rc;

use super::function::{Callable, Closure, Element, FunctionEnv};
use super::ops::is_hole;
use super::state::CodeInfo;
use super::{ErrorKind, Interpreter, JsResult, object_value};
use crate::bytecode::{ConstIndex, Constant, FunctionKind, Instruction as I, Reg};
use crate::{
    InternalMethods, ObjectKey, ObjectRef, Property, PropertyDescriptor, PropertyValue, SymbolId,
    Value,
};

/// First private-name symbol id; `u32::MAX` is the TDZ hole.
pub(crate) const PRIVATE_NAME_BASE: u32 = 1 << 31;

pub(crate) fn is_private(key: ObjectKey) -> bool {
    matches!(key, ObjectKey::Symbol(SymbolId(id)) if (PRIVATE_NAME_BASE..u32::MAX).contains(&id))
}

impl Interpreter {
    pub(crate) fn step_class(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<()> {
        let acc = self.state.acc.clone();
        match instruction {
            I::CreateClass(code, r) => self.create_class(info, *code, *r)?,
            I::SetHomeObject(r) => {
                let Some(Callable::Closure(closure)) = self.callable(&acc) else {
                    return Err(self.internal("SetHomeObject expects a closure"));
                };
                let mut closure = Closure::clone(&closure);
                closure.home = self.reg(*r).clone();
                let function = self.as_object(&acc).expect("closures are objects");
                self.replace_callable(function, Callable::Closure(Rc::new(closure)));
            }
            I::DefineElement(object, key, kind) => {
                let (object, key) = (self.reg(*object).clone(), self.reg(*key).clone());
                self.name_element(&acc, &key, *kind)?;
                self.define_element(&object, &key, *kind, acc)?;
            }
            I::AddInstanceElement(class, key, kind) => {
                let key = self.reg(*key).clone();
                self.name_element(&acc, &key, *kind)?;
                let Some(Callable::Closure(closure)) = self.callable(&self.reg(*class).clone())
                else {
                    return Err(self.internal("AddInstanceElement expects a class"));
                };
                let elements = closure.elements.as_ref().expect("classes have elements");
                elements.borrow_mut().push(Element {
                    key,
                    kind: *kind,
                    value: acc,
                });
            }
            I::NameFunction(key) => {
                let key = self.reg(*key).clone();
                self.name_anonymous_function(&acc, &key, "")?;
            }
            I::CreatePrivateName => {
                let id = self.state.next_private_name;
                self.state.next_private_name = match id.checked_add(1) {
                    Some(next) if next < u32::MAX => next,
                    _ => return Err(self.error(ErrorKind::Range, "Too many private names")),
                };
                self.state.acc = Value::Symbol(SymbolId(id));
            }
            I::SuperCall(args) => self.super_call(*args)?,
            I::GetSuper => {
                let (base, this) = self.super_base()?;
                let key = self.object_key(&acc)?;
                self.state.acc = match self.as_object(&base) {
                    Some(base) => self.get(base, key, &this)?,
                    None => self.get_value(&base, key, None)?,
                };
            }
            I::SetSuper(key) => {
                let (base, this) = self.super_base()?;
                let key = self.object_key(&self.reg(*key).clone())?;
                let Some(base) = self.as_object(&base) else {
                    return Err(self.error(ErrorKind::Type, "Cannot set properties of null"));
                };
                if !self.set(base, key, acc, &this)? && self.top().info.code.is_strict {
                    let message = "Cannot assign to read only property of super";
                    return Err(self.error(ErrorKind::Type, message));
                }
            }
            other => return Err(self.internal(&format!("unexpected instruction {other:?}"))),
        }
        Ok(())
    }

    /// The frame's `this`; a derived constructor's is a ReferenceError until `super()`.
    pub(crate) fn this_value(&mut self) -> JsResult<Value> {
        let env = &self.top().env;
        let Some(cell) = &env.this_cell else {
            return Ok(env.this.clone());
        };
        let value = cell.borrow().clone();
        if is_hole(&value) {
            return Err(self.error(
                ErrorKind::Reference,
                "Must call super constructor in derived class before accessing 'this' or \
                 returning from derived constructor",
            ));
        }
        Ok(value)
    }

    fn create_class(&mut self, info: &CodeInfo, index: ConstIndex, r: Reg) -> JsResult<()> {
        let Constant::Function(code) = &info.code.constants[index.0 as usize] else {
            return Err(self.internal("CreateClass expects a function constant"));
        };
        let code = code.clone();
        let heritage = self.reg(r).clone();
        let (proto_parent, constructor_parent) = self.class_parents(&heritage)?;
        let proto = self.alloc_object(proto_parent)?;
        self.set_reg(Reg(r.0 + 1), object_value(proto));
        let closure = Closure {
            info: self.code_info(&code),
            context: self.top().context.clone(),
            env: FunctionEnv::new(Value::Undefined),
            home: object_value(proto),
            elements: Some(Default::default()),
            _charge: Closure::charge(),
        };
        let callable = Callable::Closure(Rc::new(closure));
        let class = self.create_function_object(callable, &code.name, code.length)?;
        self.set_reg(r, object_value(class));
        if let Some(parent) = constructor_parent {
            let result = self.realm.set_prototype_of(class, Some(parent));
            result.map_err(|e| self.object_error(e))?;
        }
        let key = self.realm.key_str("prototype");
        let desc = PropertyDescriptor::data(object_value(proto), false, false, false);
        self.define_property(class, key, &desc)?;
        self.define_method(proto, "constructor", object_value(class))?;
        self.state.acc = object_value(class);
        Ok(())
    }

    /// (prototype's parent, constructor's parent) for the heritage (the hole: no `extends`).
    fn class_parents(
        &mut self,
        heritage: &Value,
    ) -> JsResult<(Option<ObjectRef>, Option<ObjectRef>)> {
        if is_hole(heritage) {
            return Ok((Some(self.state.intrinsics.object_prototype), None));
        }
        if matches!(heritage, Value::Null) {
            return Ok((None, None));
        }
        if !self.is_constructor(heritage) {
            let what = self.describe_callee(heritage);
            let message = format!("Class extends value {what} is not a constructor or null");
            return Err(self.error(ErrorKind::Type, &message));
        }
        let parent = self.as_object(heritage).expect("constructors are objects");
        let key = self.realm.key_str("prototype");
        match self.get(parent, key, heritage)? {
            Value::Object(proto) => Ok((Some(ObjectRef::from_raw(proto)), Some(parent))),
            Value::Null => Ok((None, Some(parent))),
            _ => Err(self.error(
                ErrorKind::Type,
                "Class extends value does not have valid prototype property",
            )),
        }
    }

    /// SetFunctionName for a method/accessor closure at `key` (private keys keep the
    /// compiled `#name`).
    fn name_element(&mut self, closure: &Value, key: &Value, kind: FunctionKind) -> JsResult<()> {
        let prefix = match kind {
            FunctionKind::ClassField => return Ok(()),
            FunctionKind::Getter => "get ",
            FunctionKind::Setter => "set ",
            _ => "",
        };
        self.name_anonymous_function(closure, key, prefix)
    }

    /// DefineField / DefineMethodProperty / PrivateFieldAdd / PrivateMethodOrAccessorAdd.
    fn define_element(
        &mut self,
        object: &Value,
        key: &Value,
        kind: FunctionKind,
        value: Value,
    ) -> JsResult<()> {
        let object = self
            .as_object(object)
            .expect("class elements go on objects");
        let key = self.object_key(key)?;
        if is_private(key) {
            return self.private_add(object, key, kind, value);
        }
        let desc = match kind {
            FunctionKind::ClassField => PropertyDescriptor::data(value, true, true, true),
            FunctionKind::Getter | FunctionKind::Setter => accessor(kind, value),
            _ => PropertyDescriptor::data(value, true, false, true),
        };
        if !self.define_property(object, key, &desc)? {
            let message = format!("Cannot redefine property: {}", self.key_text(key));
            return Err(self.error(ErrorKind::Type, &message));
        }
        Ok(())
    }

    fn private_add(
        &mut self,
        object: ObjectRef,
        key: ObjectKey,
        kind: FunctionKind,
        value: Value,
    ) -> JsResult<()> {
        let existing = self.raw_own_property(object, key)?;
        let desc = match (kind, existing.map(|p| p.value)) {
            (FunctionKind::ClassField, None) => PropertyDescriptor::data(value, true, false, false),
            (
                FunctionKind::Getter,
                None
                | Some(PropertyValue::Accessor {
                    get: Value::Undefined,
                    ..
                }),
            )
            | (
                FunctionKind::Setter,
                None
                | Some(PropertyValue::Accessor {
                    set: Value::Undefined,
                    ..
                }),
            ) => accessor(kind, value),
            (FunctionKind::Method, None) => PropertyDescriptor::data(value, false, false, false),
            _ => {
                let message = "Cannot initialize private member twice on the same object";
                return Err(self.error(ErrorKind::Type, message));
            }
        };
        if !self.define_property(object, key, &desc)? {
            let message = "Cannot define private member on a non-extensible object";
            return Err(self.error(ErrorKind::Type, message));
        }
        Ok(())
    }

    pub(crate) fn raw_own_property(
        &mut self,
        object: ObjectRef,
        key: ObjectKey,
    ) -> JsResult<Option<Property>> {
        let own = self.realm.get_own_property(object, key);
        own.map_err(|e| self.object_error(e))
    }

    /// The private property of `base`, or a brand-check TypeError.
    fn private_property(&mut self, base: &Value, key: ObjectKey, verb: &str) -> JsResult<Property> {
        if let Some(object) = self.as_object(base)
            && let Some(property) = self.raw_own_property(object, key)?
        {
            return Ok(property);
        }
        let message =
            format!("Cannot {verb} private member from an object whose class did not declare it");
        Err(self.error(ErrorKind::Type, &message))
    }

    pub(crate) fn private_get(&mut self, base: &Value, key: ObjectKey) -> JsResult<Value> {
        match self.private_property(base, key, "read")?.value {
            PropertyValue::Data { value, .. } => Ok(value),
            PropertyValue::Accessor {
                get: Value::Undefined,
                ..
            } => Err(self.error(
                ErrorKind::Type,
                "Private accessor was defined without a getter",
            )),
            PropertyValue::Accessor { get, .. } => self.call_value(&get, base, &[]),
        }
    }

    pub(crate) fn private_set(
        &mut self,
        base: &Value,
        key: ObjectKey,
        value: Value,
    ) -> JsResult<()> {
        match self.private_property(base, key, "write")?.value {
            PropertyValue::Data { writable: true, .. } => {
                let object = self
                    .as_object(base)
                    .expect("private properties are on objects");
                let desc = PropertyDescriptor {
                    value: Some(value),
                    ..PropertyDescriptor::default()
                };
                self.define_property(object, key, &desc)?;
                Ok(())
            }
            PropertyValue::Data { .. } => {
                Err(self.error(ErrorKind::Type, "Private method is not writable"))
            }
            PropertyValue::Accessor {
                set: Value::Undefined,
                ..
            } => Err(self.error(
                ErrorKind::Type,
                "Private accessor was defined without a setter",
            )),
            PropertyValue::Accessor { set, .. } => {
                self.call_value(&set, base, &[value])?;
                Ok(())
            }
        }
    }

    /// `#x in object`.
    pub(crate) fn private_has(&mut self, object: ObjectRef, key: ObjectKey) -> JsResult<bool> {
        Ok(self.raw_own_property(object, key)?.is_some())
    }

    /// InitializeInstanceElements(object, constructor): private methods, then fields.
    pub(crate) fn initialize_instance_elements(
        &mut self,
        object: &Value,
        constructor: &Value,
    ) -> JsResult<()> {
        let Some(Callable::Closure(closure)) = self.callable(constructor) else {
            return Ok(());
        };
        let Some(elements) = &closure.elements else {
            return Ok(());
        };
        let elements: Vec<Element> = elements.borrow().clone();
        let (fields, methods): (Vec<_>, Vec<_>) = elements
            .into_iter()
            .partition(|element| element.kind == FunctionKind::ClassField);
        for element in methods {
            self.define_element(object, &element.key, element.kind, element.value)?;
        }
        for field in fields {
            let value = match field.value {
                Value::Undefined => Value::Undefined,
                initializer => self.call_value(&initializer, object, &[field.key.clone()])?,
            };
            self.define_element(object, &field.key, FunctionKind::ClassField, value)?;
        }
        Ok(())
    }

    fn super_call(&mut self, args: Reg) -> JsResult<()> {
        let env = self.top().env.clone();
        let Some(cell) = env.this_cell.clone() else {
            return Err(self.internal("super() outside a derived constructor"));
        };
        let args = self.array_elements(self.reg(args).clone())?;
        let constructor = self
            .as_object(&env.function)
            .expect("functions are objects");
        let parent = self
            .prototype_of(constructor)?
            .map_or(Value::Null, object_value);
        if !self.is_constructor(&parent) {
            return Err(self.error(ErrorKind::Type, "Super constructor is not a constructor"));
        }
        let result = self.construct_value(&parent, &args, &env.new_target)?;
        if !is_hole(&cell.borrow()) {
            let message = "Super constructor may only be called once";
            return Err(self.error(ErrorKind::Reference, message));
        }
        *cell.borrow_mut() = result.clone();
        self.initialize_instance_elements(&result, &env.function)?;
        self.state.acc = result;
        Ok(())
    }

    /// (HomeObject.[[Prototype]], this) for a `super` property reference.
    fn super_base(&mut self) -> JsResult<(Value, Value)> {
        let this = self.this_value()?;
        let function = self.top().env.function.clone();
        let home = match self.callable(&function) {
            Some(Callable::Closure(closure)) => closure.home.clone(),
            _ => Value::Undefined,
        };
        let Some(home) = self.as_object(&home) else {
            return Err(self.internal("super property outside a method"));
        };
        let base = self.prototype_of(home)?.map_or(Value::Null, object_value);
        Ok((base, this))
    }
}

fn accessor(kind: FunctionKind, function: Value) -> PropertyDescriptor {
    let (get, set) = match kind {
        FunctionKind::Getter => (Some(function), None),
        _ => (None, Some(function)),
    };
    PropertyDescriptor {
        get,
        set,
        enumerable: Some(false),
        configurable: Some(true),
        ..PropertyDescriptor::default()
    }
}
