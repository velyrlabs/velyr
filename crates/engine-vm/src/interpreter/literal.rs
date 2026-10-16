//! Object/array literals, template objects and the `arguments`/rest objects.

use std::rc::Rc;

use velyr_engine_gc::Gc;

use super::builtins::Intrinsic;
use super::function::Callable;
use super::state::CodeInfo;
use super::{Interpreter, JsResult, object_value};
use crate::bytecode::{ConstIndex, Constant, Instruction as I, Reg, RegRange};
use crate::object::ParameterMap;
use crate::{InternalMethods, JsStr, ObjectKey, ObjectRef, PropertyDescriptor, Value};

impl Interpreter {
    pub(crate) fn step_literal(&mut self, info: &CodeInfo, instruction: &I) -> JsResult<()> {
        let acc = self.state.acc.clone();
        match instruction {
            I::CreateObject => {
                let proto = Some(self.state.intrinsics.object_prototype);
                self.state.acc = object_value(self.alloc_object(proto)?);
            }
            I::CreateArray => self.state.acc = object_value(self.alloc_array(0)?),
            I::DefineNamedOwn { object, name } => {
                let key = self.name_key(info, *name);
                self.create_data_property(self.object_reg(*object), key, acc)?;
            }
            I::DefineKeyedOwn {
                object,
                key,
                set_function_name,
            } => {
                let key_value = self.reg(*key).clone();
                if *set_function_name {
                    self.name_anonymous_function(&acc, &key_value, "")?;
                }
                let key = self.object_key(&key_value)?;
                self.create_data_property(self.object_reg(*object), key, acc)?;
            }
            I::DefineGetter { object, key } => self.define_accessor(*object, *key, true)?,
            I::DefineSetter { object, key } => self.define_accessor(*object, *key, false)?,
            I::SetLiteralPrototype { object } => {
                if matches!(acc, Value::Object(_) | Value::Null) {
                    let proto = self.as_object(&acc);
                    let object = self.object_reg(*object);
                    let result = self.realm.set_prototype_of(object, proto);
                    result.map_err(|e| self.object_error(e))?;
                }
            }
            I::CopyDataProperties { target, excluded } => {
                self.copy_data_properties(*target, *excluded)?;
            }
            I::ArrayPush { array } => {
                let array = self.object_reg(*array);
                let length = self.array_length(array)?;
                self.create_data_property(array, ObjectKey::Index(length), acc)?;
            }
            I::ArrayPushHole { array } => {
                let array = self.object_reg(*array);
                let length = Value::Number(f64::from(self.array_length(array)? + 1));
                let desc = PropertyDescriptor {
                    value: Some(length),
                    ..PropertyDescriptor::default()
                };
                let key = self.state.intrinsics.length_key;
                self.define_property(array, key, &desc)?;
            }
            I::CreateRegExp { pattern, flags } => {
                let pattern = Value::String(self.constant_str(info, *pattern));
                let flags = Value::String(self.constant_str(info, *flags));
                self.state.acc = object_value(self.regexp_create(&pattern, &flags)?);
            }
            I::GetTemplateObject(c) => self.state.acc = self.template_object(info, *c)?,
            I::CreateUnmappedArguments => self.state.acc = self.create_arguments()?,
            I::MapArgument(index, slot) => self.map_argument(*index, *slot)?,
            I::CreateRestParameter(start) => {
                let rest: Vec<Value> = self
                    .top()
                    .arguments
                    .iter()
                    .skip(*start as usize)
                    .cloned()
                    .collect();
                self.state.acc = object_value(self.create_array_from(&rest)?);
            }
            other => return self.step_iteration(info, other),
        }
        Ok(())
    }

    fn object_reg(&self, r: Reg) -> ObjectRef {
        self.as_object(self.reg(r))
            .expect("literal register holds an object")
    }

    pub(crate) fn array_length(&mut self, array: ObjectRef) -> JsResult<u32> {
        let key = self.state.intrinsics.length_key;
        let length = self.get(array, key, &object_value(array))?;
        Ok(self.coerce_number(&length)? as u32)
    }

    /// Array of `values`; they must already be rooted by the caller.
    pub(crate) fn create_array_from(&mut self, values: &[Value]) -> JsResult<ObjectRef> {
        let array = self.alloc_array(0)?;
        for (index, value) in values.iter().enumerate() {
            self.create_data_property(array, ObjectKey::Index(index as u32), value.clone())?;
        }
        Ok(array)
    }

    /// SetFunctionName for an anonymous closure assigned to `key` (`prefix` for accessors).
    /// Private-name keys keep the compiled name (`#x`) as the base.
    pub(crate) fn name_anonymous_function(
        &mut self,
        function: &Value,
        key: &Value,
        prefix: &str,
    ) -> JsResult<()> {
        let Some(Callable::Closure(closure)) = self.callable(function) else {
            return Ok(());
        };
        if !closure.info.code.name.is_empty() && prefix.is_empty() {
            return Ok(());
        }
        let name = match key {
            Value::String(s) => s.clone(),
            _ => closure.info.code.name.clone(),
        };
        let name = JsStr::from_str(prefix).concat(&name);
        let function = self.as_object(function).expect("closures are objects");
        self.set_function_name(function, Value::String(name))
    }

    fn define_accessor(&mut self, object: Reg, key: Reg, is_getter: bool) -> JsResult<()> {
        let function = self.state.acc.clone();
        let key_value = self.reg(key).clone();
        let prefix = if is_getter { "get " } else { "set " };
        self.name_anonymous_function(&function, &key_value, prefix)?;
        let key = self.object_key(&key_value)?;
        let (get, set) = match is_getter {
            true => (Some(function), None),
            false => (None, Some(function)),
        };
        let desc = PropertyDescriptor {
            get,
            set,
            enumerable: Some(true),
            configurable: Some(true),
            ..PropertyDescriptor::default()
        };
        self.define_property(self.object_reg(object), key, &desc)?;
        Ok(())
    }

    /// CopyDataProperties(target, acc, excluded keys).
    fn copy_data_properties(&mut self, target: Reg, excluded: RegRange) -> JsResult<()> {
        let source_value = self.state.acc.clone();
        if matches!(source_value, Value::Undefined | Value::Null) {
            return Ok(());
        }
        let source = self.coerce_object(&source_value)?;
        self.state.scratch.push(object_value(source));
        let result = self.copy_own_enumerable(source, target, excluded);
        self.state.scratch.pop();
        result
    }

    fn copy_own_enumerable(
        &mut self,
        source: ObjectRef,
        target: Reg,
        excluded: RegRange,
    ) -> JsResult<()> {
        let source_value = object_value(source);
        let excluded = self
            .reg_range(excluded)
            .iter()
            .map(|key| self.object_key(key))
            .collect::<JsResult<Vec<_>>>()?;
        let keys = self.own_keys(source)?;
        let target = self.object_reg(target);
        for key in keys.into_iter().filter(|key| !excluded.contains(key)) {
            if self
                .own_property(source, key)?
                .is_some_and(|p| p.enumerable)
            {
                let value = self.get(source, key, &source_value)?;
                self.create_data_property(target, key, value)?;
            }
        }
        Ok(())
    }

    /// GetTemplateObject: one frozen-shape array per site, with its `raw` strings.
    /// shortcut: the arrays are not frozen (no [[PreventExtensions]] on elements yet).
    fn template_object(&mut self, info: &CodeInfo, index: ConstIndex) -> JsResult<Value> {
        let site = (Rc::as_ptr(&info.code) as usize, index.0);
        if let Some(template) = self.state.templates.get(&site) {
            return Ok(template.clone());
        }
        let Constant::TemplateObject(strings) = &info.code.constants[index.0 as usize] else {
            return Err(self.internal("GetTemplateObject expects a template constant"));
        };
        let cooked: Vec<Value> = strings
            .cooked
            .iter()
            .map(|s| s.clone().map_or(Value::Undefined, Value::String))
            .collect();
        let raw: Vec<Value> = strings.raw.iter().cloned().map(Value::String).collect();
        let raw = self.create_array_from(&raw)?;
        self.state.scratch.push(object_value(raw));
        let template = self.create_array_from(&cooked);
        self.state.scratch.pop();
        let template = template?;
        let key = self.realm.key_str("raw");
        let desc = PropertyDescriptor::data(object_value(raw), false, false, false);
        self.define_property(template, key, &desc)?;
        self.state.templates.insert(site, object_value(template));
        Ok(object_value(template))
    }

    /// CreateMappedArgumentsObject step 15, one parameter at a time: argument `index` of the
    /// arguments object in `acc` aliases `slot` of the current context (no-op past argc).
    fn map_argument(&mut self, index: u32, slot: u32) -> JsResult<()> {
        if index as usize >= self.top().arguments.len() {
            return Ok(());
        }
        let Value::Object(arguments) = self.state.acc else {
            return Err(self.internal("MapArgument expects the arguments object"));
        };
        let arguments = Gc::from_raw(arguments);
        let map = match self.realm.parameter_map(arguments) {
            Some(map) => map,
            None => {
                let Value::Object(context) = self.top().context else {
                    return Err(self.internal("MapArgument needs the function context"));
                };
                let map = Gc::from_raw(self.alloc_cell(ParameterMap::new(context))?);
                self.realm.set_parameter_map(arguments, map);
                map
            }
        };
        self.realm.map_argument(map, index, slot);
        Ok(())
    }

    /// CreateUnmappedArgumentsObject.
    fn create_arguments(&mut self) -> JsResult<Value> {
        let proto = Some(self.state.intrinsics.object_prototype);
        let arguments = self.alloc_object(proto)?;
        self.set_slot(arguments, super::builtins::slots::Slot::Arguments);
        let values = self.top().arguments.clone();
        for (index, value) in values.into_iter().enumerate() {
            self.create_data_property(arguments, ObjectKey::Index(index as u32), value)?;
        }
        let length = Value::Number(self.top().arguments.len() as f64);
        let key = self.state.intrinsics.length_key;
        self.define_property(
            arguments,
            key,
            &PropertyDescriptor::data(length, true, false, true),
        )?;
        let values = self.intrinsic(Intrinsic::ArrayPrototypeValues);
        let key = ObjectKey::Symbol(super::builtins::symbol::SYMBOL_ITERATOR);
        let desc = PropertyDescriptor::data(object_value(values), true, false, true);
        self.define_property(arguments, key, &desc)?;
        let key = self.realm.key_str("callee");
        let desc = if self.top().info.code.is_strict {
            let thrower = object_value(self.intrinsic(Intrinsic::ThrowTypeError));
            PropertyDescriptor::accessor(thrower.clone(), thrower, false, false)
        } else {
            PropertyDescriptor::data(self.top().function.clone(), true, false, true)
        };
        self.define_property(arguments, key, &desc)?;
        Ok(object_value(arguments))
    }
}
