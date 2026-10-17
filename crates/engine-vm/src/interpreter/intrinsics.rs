//! The realm of the bytecode VM: allocates the intrinsic objects the interpreter core
//! refers to, then hands the global object to `builtins::install_all`.

use velyr_engine_gc::GcRef;

use super::builtins::slots::{Slot, SlotTable};
use super::builtins::{self, Intrinsic};
use super::state::{FunctionTable, Intrinsics, State};
use super::{ErrorKind, Interpreter, JsResult, object_value};
use crate::{JsStr, ObjectRef, PropertyDescriptor, Realm, Value};

/// Allocates the intrinsic objects; properties are installed by `install_globals`.
pub(crate) fn create_state(realm: &mut Realm) -> State {
    let mut roots: Vec<GcRef> = Vec::new();
    let object = |realm: &mut Realm, proto: Option<ObjectRef>, roots: &mut Vec<GcRef>| {
        let object = realm.create_object(proto, &*roots).expect("fresh heap");
        roots.push(object.raw());
        object
    };
    let object_prototype = object(realm, None, &mut roots);
    let function_prototype = object(realm, Some(object_prototype), &mut roots);
    let iterator_prototype = object(realm, Some(object_prototype), &mut roots);
    let array_iterator_prototype = object(realm, Some(iterator_prototype), &mut roots);
    let function_call = object(realm, Some(function_prototype), &mut roots);
    let function_apply = object(realm, Some(function_prototype), &mut roots);
    let global = object(realm, Some(object_prototype), &mut roots);
    let error_base = object(realm, Some(object_prototype), &mut roots);
    let mut error_prototypes = [error_base; ErrorKind::ALL.len()];
    for prototype in error_prototypes.iter_mut().skip(1) {
        *prototype = object(realm, Some(error_base), &mut roots);
    }
    let mut table = [object_prototype; Intrinsic::COUNT];
    for which in PROTOTYPES {
        let proto = match which {
            Intrinsic::StringIteratorPrototype
            | Intrinsic::MapIteratorPrototype
            | Intrinsic::SetIteratorPrototype
            | Intrinsic::RegExpStringIteratorPrototype => iterator_prototype,
            _ => object_prototype,
        };
        table[which as usize] = object(realm, Some(proto), &mut roots);
    }
    table[Intrinsic::IteratorPrototype as usize] = iterator_prototype;
    let array_prototype = realm
        .create_array(Some(object_prototype), 0, &roots)
        .expect("fresh heap");
    roots.push(array_prototype.raw());
    let functions = realm
        .heap
        .alloc(FunctionTable::default(), &roots)
        .expect("fresh heap");
    roots.push(functions.raw());
    let slots = realm
        .heap
        .alloc(SlotTable::default(), &roots)
        .expect("fresh heap");
    let length_key = realm.key_str("length");
    State {
        registers: Vec::new(),
        frames: Vec::new(),
        acc: Value::Undefined,
        scratch: Vec::new(),
        lexicals: Default::default(),
        functions,
        intrinsics: Intrinsics {
            global,
            object_prototype,
            function_prototype,
            array_prototype,
            array_iterator_prototype,
            function_call,
            function_apply,
            error_prototypes,
            // Replaced by the real constructor in `builtins::error`.
            error_constructor: error_base,
            length_key,
            table,
        },
        templates: Default::default(),
        handles: Default::default(),
        next_private_name: super::class::PRIVATE_NAME_BASE,
        new_target: Value::Undefined,
        slots,
        symbols: Default::default(),
        jobs: Default::default(),
        regexps: Default::default(),
        modules: Vec::new(),
        entry_module: None,
        kept: Default::default(),
        lazy_pending: 0,
        frame_mapper: None,
    }
}

/// Prototype objects allocated up front (constructors are made by their installers).
const PROTOTYPES: [Intrinsic; 17] = [
    Intrinsic::DatePrototype,
    Intrinsic::StringPrototype,
    Intrinsic::NumberPrototype,
    Intrinsic::BooleanPrototype,
    Intrinsic::SymbolPrototype,
    Intrinsic::BigIntPrototype,
    Intrinsic::StringIteratorPrototype,
    Intrinsic::MapPrototype,
    Intrinsic::SetPrototype,
    Intrinsic::WeakMapPrototype,
    Intrinsic::WeakSetPrototype,
    Intrinsic::WeakRefPrototype,
    Intrinsic::FinalizationRegistryPrototype,
    Intrinsic::MapIteratorPrototype,
    Intrinsic::SetIteratorPrototype,
    Intrinsic::RegExpPrototype,
    Intrinsic::RegExpStringIteratorPrototype,
];

impl Interpreter {
    pub(crate) fn install_globals(&mut self) -> JsResult<()> {
        let intrinsics = self.state.intrinsics;
        let global = intrinsics.global;
        self.define_method(global, "globalThis", object_value(global))?;
        for (name, value) in [
            ("undefined", Value::Undefined),
            ("NaN", Value::Number(f64::NAN)),
            ("Infinity", Value::Number(f64::INFINITY)),
        ] {
            let key = self.realm.key_str(name);
            self.define_property(
                global,
                key,
                &PropertyDescriptor::data(value, false, false, false),
            )?;
        }
        self.install_function_prototype()?;
        let base = self.state.scratch.len();
        let result = builtins::install_all(self);
        self.state.scratch.truncate(base);
        result
    }

    /// Writable, non-enumerable, configurable data property (built-in method layout).
    pub(crate) fn define_method(
        &mut self,
        object: ObjectRef,
        name: &str,
        value: Value,
    ) -> JsResult<()> {
        let key = self.realm.key_str(name);
        self.define_property(
            object,
            key,
            &PropertyDescriptor::data(value, true, false, true),
        )?;
        Ok(())
    }

    pub(crate) fn create_error(&mut self, kind: ErrorKind, message: &str) -> JsResult<Value> {
        self.create_error_object(kind, Some(JsStr::from_str(message)))
    }

    /// An error object of `kind` with an optional own `message` and a captured `stack`.
    pub(crate) fn create_error_object(
        &mut self,
        kind: ErrorKind,
        message: Option<JsStr>,
    ) -> JsResult<Value> {
        let prototype = self.state.intrinsics.error_prototypes[kind as usize];
        self.create_error_with_proto(prototype, message)
    }

    pub(crate) fn create_error_with_proto(
        &mut self,
        prototype: ObjectRef,
        message: Option<JsStr>,
    ) -> JsResult<Value> {
        let error = self.alloc_object(Some(prototype))?;
        self.set_slot(error, Slot::Error);
        if let Some(message) = message {
            self.define_method(error, "message", Value::String(message))?;
        }
        self.state.scratch.push(object_value(error));
        let captured = self.capture_stack(error, None);
        self.state.scratch.pop();
        captured?;
        Ok(object_value(error))
    }
}

pub(crate) fn describe_error(
    vm: &mut Interpreter,
    error: ObjectRef,
    this: &Value,
) -> JsResult<String> {
    let field = |vm: &mut Interpreter, name: &str, default: &str| -> JsResult<String> {
        let key = vm.realm.key_str(name);
        match vm.get(error, key, this)? {
            Value::Undefined => Ok(default.to_owned()),
            value => Ok(vm.coerce_string(&value)?.to_string_lossy()),
        }
    };
    let name = field(vm, "name", "Error")?;
    let message = field(vm, "message", "")?;
    Ok(match (name.is_empty(), message.is_empty()) {
        (_, true) => name,
        (true, false) => message,
        (false, false) => format!("{name}: {message}"),
    })
}

/// Message of an uncaught exception, for the embedder: `ToString(value)`, like engine-v8.
pub(crate) fn describe_exception(vm: &mut Interpreter, value: &Value) -> String {
    match vm.coerce_string(value) {
        Ok(text) => text.to_string_lossy(),
        Err(_) => "uncaught exception".into(),
    }
}
