//! Core built-ins of the bytecode VM realm (VELYR-107): one module per built-in, each with
//! an `install` that defines its constructor and prototype with the spec's property
//! attributes. Shared abstract operations live in `abstract_ops` and `object_ops`.

mod abstract_ops;
mod aggregate_error;
mod array;
mod bigint;
mod boolean;
pub(crate) mod collections;
pub(crate) mod date;
mod error;
pub(crate) mod function;
mod iterator;
mod json;
mod map;
mod math;
mod number;
mod number_format;
mod object;
mod object_ops;
mod promise;
mod promise_combinators;
pub(crate) mod promise_ops;
mod proxy;
mod reflect;
pub(crate) mod regexp;
mod set;
pub(crate) mod slots;
mod string;
pub(crate) mod suppressed_error;
pub(crate) mod symbol;
pub(crate) mod typed_arrays;
mod uri;
mod weak;
pub(crate) mod weak_ref;

use std::rc::Rc;

use super::{ErrorKind, Interpreter, JsResult, object_value};
use crate::{ObjectKey, ObjectRef, PropertyDescriptor, Value};

pub(crate) use object_ops::ToPropertyDescriptor;

/// Native function body (same shape as `NativeFn`, as a plain `fn`).
pub(crate) type Builtin = fn(&mut Interpreter, &Value, &[Value]) -> JsResult<Value>;

/// Built-in objects reachable from Rust, stored in `Intrinsics::table`.
#[derive(Clone, Copy)]
pub(crate) enum Intrinsic {
    ObjectConstructor,
    FunctionConstructor,
    ArrayConstructor,
    StringPrototype,
    NumberPrototype,
    BooleanPrototype,
    SymbolPrototype,
    BigIntPrototype,
    IteratorPrototype,
    StringIteratorPrototype,
    MapPrototype,
    SetPrototype,
    WeakMapPrototype,
    WeakSetPrototype,
    WeakRefPrototype,
    FinalizationRegistryPrototype,
    MapIteratorPrototype,
    SetIteratorPrototype,
    ThrowTypeError,
    ArrayPrototypeValues,
    ProxyConstructor,
    PromisePrototype,
    PromiseConstructor,
    AggregateErrorPrototype,
    SuppressedErrorPrototype,
    RegExpPrototype,
    RegExpConstructor,
    RegExpStringIteratorPrototype,
    GeneratorFunctionPrototype,
    GeneratorPrototype,
    AsyncFunctionPrototype,
    AsyncGeneratorFunctionPrototype,
    AsyncGeneratorPrototype,
    DatePrototype,
    AsyncFromSyncIteratorPrototype,
    ArrayBufferConstructor,
    ArrayBufferPrototype,
    SharedArrayBufferConstructor,
    SharedArrayBufferPrototype,
    DataViewPrototype,
    TypedArrayPrototype,
    Int8Array,
    Uint8Array,
    Uint8ClampedArray,
    Int16Array,
    Uint16Array,
    Int32Array,
    Uint32Array,
    Float16Array,
    Float32Array,
    Float64Array,
    BigInt64Array,
    BigUint64Array,
}

impl Intrinsic {
    pub(crate) const COUNT: usize = Intrinsic::BigUint64Array as usize + 1;
}

/// Elements a native loop processes between heap limit checks.
const HEAP_CHECK_INTERVAL: u32 = 4096;

/// `args[index]`, or `undefined` when absent.
pub(crate) fn arg(args: &[Value], index: usize) -> Value {
    args.get(index).cloned().unwrap_or(Value::Undefined)
}

impl Interpreter {
    pub(crate) fn intrinsic(&self, which: Intrinsic) -> ObjectRef {
        self.state.intrinsics.table[which as usize]
    }

    fn set_intrinsic(&mut self, which: Intrinsic, object: ObjectRef) {
        self.state.intrinsics.table[which as usize] = object;
    }

    /// Roots `value` until the running native function returns.
    pub(crate) fn keep(&mut self, value: &Value) {
        self.state.scratch.push(value.clone());
    }

    /// Roots `roots` for the duration of `body`, which may collect the heap.
    pub(crate) fn rooted<T>(
        &mut self,
        roots: &[Value],
        body: impl FnOnce(&mut Self) -> JsResult<T>,
    ) -> JsResult<T> {
        let base = self.state.scratch.len();
        self.state.scratch.extend_from_slice(roots);
        let result = body(self);
        self.state.scratch.truncate(base);
        result
    }

    /// Call once per element of a native loop that allocates in proportion to its input:
    /// every `HEAP_CHECK_INTERVAL` calls it checks the heap limit for `bytes` of
    /// not-yet-accounted storage. Everything live must be rooted (see `rooted`).
    #[inline]
    pub(crate) fn heap_tick(&mut self, since_check: &mut u32, bytes: usize) -> JsResult<()> {
        *since_check += 1;
        if *since_check < HEAP_CHECK_INTERVAL {
            return Ok(());
        }
        *since_check = 0;
        self.reserve_heap(bytes)
    }

    pub(crate) fn throw_type<T>(&mut self, message: &str) -> JsResult<T> {
        Err(self.error(ErrorKind::Type, message))
    }

    pub(crate) fn throw_range<T>(&mut self, message: &str) -> JsResult<T> {
        Err(self.error(ErrorKind::Range, message))
    }

    /// A fresh ordinary object with `Object.prototype`, rooted until the native returns.
    pub(crate) fn new_object(&mut self) -> JsResult<ObjectRef> {
        let object = self.alloc_object(Some(self.state.intrinsics.object_prototype))?;
        self.keep(&object_value(object));
        Ok(object)
    }

    /// An ordinary object with `proto`, rooted until the native returns.
    pub(crate) fn new_object_with(&mut self, proto: Option<ObjectRef>) -> JsResult<ObjectRef> {
        let object = self.alloc_object(proto)?;
        self.keep(&object_value(object));
        Ok(object)
    }

    /// A built-in function object, rooted until the native returns.
    pub(crate) fn new_builtin(&mut self, name: &str, arity: u32, f: Builtin) -> JsResult<Value> {
        let function = self.create_native_function(name, arity, Rc::new(f), false)?;
        let value = object_value(function);
        self.keep(&value);
        Ok(value)
    }

    /// Built-in method: writable, non-enumerable, configurable.
    pub(crate) fn method(
        &mut self,
        object: ObjectRef,
        name: &str,
        arity: u32,
        f: Builtin,
    ) -> JsResult<()> {
        let function = self.new_builtin(name, arity, f)?;
        self.define_method(object, name, function)
    }

    /// Built-in method under a well-known symbol, named `[Symbol.<description>]`.
    pub(crate) fn symbol_method(
        &mut self,
        object: ObjectRef,
        symbol: crate::SymbolId,
        arity: u32,
        f: Builtin,
    ) -> JsResult<Value> {
        let name = format!("[{}]", self.symbol_description_text(symbol));
        let function = self.new_builtin(&name, arity, f)?;
        let desc = PropertyDescriptor::data(function.clone(), true, false, true);
        self.define_property(object, ObjectKey::Symbol(symbol), &desc)?;
        Ok(function)
    }

    /// Accessor property with a built-in getter (`get <name>`) and optional setter.
    pub(crate) fn accessor(
        &mut self,
        object: ObjectRef,
        key: ObjectKey,
        name: &str,
        get: Builtin,
        set: Option<Builtin>,
    ) -> JsResult<()> {
        let getter = self.new_builtin(&format!("get {name}"), 0, get)?;
        let setter = match set {
            Some(set) => self.new_builtin(&format!("set {name}"), 1, set)?,
            None => Value::Undefined,
        };
        let desc = PropertyDescriptor::accessor(getter, setter, false, true);
        self.define_property(object, key, &desc)?;
        Ok(())
    }

    /// Non-writable, non-enumerable, non-configurable data property.
    pub(crate) fn constant(&mut self, object: ObjectRef, name: &str, value: Value) -> JsResult<()> {
        let key = self.realm.key_str(name);
        let desc = PropertyDescriptor::data(value, false, false, false);
        self.define_property(object, key, &desc)?;
        Ok(())
    }

    /// A constructor `name` whose `prototype` is `proto` (non-writable) and whose
    /// `proto.constructor` points back; also bound on the global object.
    pub(crate) fn constructor(
        &mut self,
        name: &str,
        arity: u32,
        f: Builtin,
        proto: ObjectRef,
    ) -> JsResult<ObjectRef> {
        let function = self.create_native_function(name, arity, Rc::new(f), true)?;
        self.keep(&object_value(function));
        self.constant(function, "prototype", object_value(proto))?;
        self.define_method(proto, "constructor", object_value(function))?;
        let global = self.state.intrinsics.global;
        self.define_method(global, name, object_value(function))?;
        Ok(function)
    }

    /// `NewTarget` of the running native, read before it calls anything else.
    pub(crate) fn new_target(&self) -> Value {
        self.state.new_target.clone()
    }
}

/// Installs every built-in on the realm, in dependency order.
pub(crate) fn install_all(vm: &mut Interpreter) -> JsResult<()> {
    symbol::install(vm)?;
    object::install(vm)?;
    function::install(vm)?;
    iterator::install(vm)?;
    array::install(vm)?;
    string::install(vm)?;
    regexp::install(vm)?;
    number::install(vm)?;
    uri::install(vm)?;
    bigint::install(vm)?;
    boolean::install(vm)?;
    date::install(vm)?;
    math::install(vm)?;
    error::install(vm)?;
    aggregate_error::install(vm)?;
    suppressed_error::install(vm)?;
    json::install(vm)?;
    reflect::install(vm)?;
    map::install(vm)?;
    set::install(vm)?;
    weak::install(vm)?;
    weak_ref::install(vm)?;
    proxy::install(vm)?;
    promise::install(vm)?;
    typed_arrays::install(vm)?;
    super::generator::install(vm)
}
