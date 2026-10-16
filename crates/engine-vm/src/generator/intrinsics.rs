//! Generator, async function, async generator and async-from-sync iterator intrinsics.

use std::rc::Rc;

use super::{SYMBOL_ASYNC_ITERATOR, mode_of};
use crate::bytecode::CoroutineKind;
use crate::interpreter::builtins::function::create_dynamic_function;
use crate::interpreter::builtins::{Builtin, Intrinsic, arg};
use crate::interpreter::function::NativeFn;
use crate::interpreter::{Interpreter, JsResult, object_value};
use crate::{ObjectRef, PropertyDescriptor, Value};

impl Interpreter {
    /// Generator and async functions get their intrinsic [[Prototype]]; generators also a
    /// `prototype` object for their instances.
    pub(crate) fn init_coroutine_function(
        &mut self,
        function: ObjectRef,
        kind: CoroutineKind,
    ) -> JsResult<()> {
        let (function_proto, instance_proto) = match kind {
            CoroutineKind::Generator => (
                Intrinsic::GeneratorFunctionPrototype,
                Some(Intrinsic::GeneratorPrototype),
            ),
            CoroutineKind::Async => (Intrinsic::AsyncFunctionPrototype, None),
            CoroutineKind::AsyncGenerator => (
                Intrinsic::AsyncGeneratorFunctionPrototype,
                Some(Intrinsic::AsyncGeneratorPrototype),
            ),
        };
        let proto = Some(self.intrinsic(function_proto));
        self.set_prototype(function, proto)?;
        let Some(instance_proto) = instance_proto else {
            return Ok(());
        };
        self.scoped(|vm| {
            vm.keep(&object_value(function));
            let prototype = vm.new_object_with(Some(vm.intrinsic(instance_proto)))?;
            let desc = PropertyDescriptor::data(object_value(prototype), true, false, false);
            let key = vm.realm.key_str("prototype");
            vm.define_property(function, key, &desc)?;
            Ok(())
        })
    }
}

/// Installs the generator, async function, async generator and async-from-sync iterator
/// intrinsics (they have no global constructors).
pub(crate) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let intrinsics = vm.state.intrinsics;
    let iterator_proto = vm.intrinsic(Intrinsic::IteratorPrototype);
    let async_iterator = vm.new_object_with(Some(intrinsics.object_prototype))?;
    vm.symbol_method(async_iterator, SYMBOL_ASYNC_ITERATOR, 0, |_, this, _| {
        Ok(this.clone())
    })?;
    let families: [(Intrinsic, Intrinsic, ObjectRef, &str); 2] = [
        (
            Intrinsic::GeneratorFunctionPrototype,
            Intrinsic::GeneratorPrototype,
            iterator_proto,
            "Generator",
        ),
        (
            Intrinsic::AsyncGeneratorFunctionPrototype,
            Intrinsic::AsyncGeneratorPrototype,
            async_iterator,
            "AsyncGenerator",
        ),
    ];
    for (function_which, which, parent, tag) in families {
        let function_proto = vm.new_object_with(Some(intrinsics.function_prototype))?;
        let proto = vm.new_object_with(Some(parent))?;
        let link = |o| PropertyDescriptor::data(object_value(o), false, false, true);
        let (prototype, constructor) = (
            vm.realm.key_str("prototype"),
            vm.realm.key_str("constructor"),
        );
        vm.define_property(function_proto, prototype, &link(proto))?;
        vm.define_property(proto, constructor, &link(function_proto))?;
        vm.define_to_string_tag(function_proto, &format!("{tag}Function"))?;
        vm.define_to_string_tag(proto, tag)?;
        vm.state.intrinsics.table[function_which as usize] = function_proto;
        vm.state.intrinsics.table[which as usize] = proto;
    }
    let methods: [(&str, Builtin, Builtin, Builtin); 3] = [
        (
            "next",
            coroutine_method::<0, 0>,
            coroutine_method::<1, 0>,
            coroutine_method::<2, 0>,
        ),
        (
            "return",
            coroutine_method::<0, 2>,
            coroutine_method::<1, 2>,
            coroutine_method::<2, 2>,
        ),
        (
            "throw",
            coroutine_method::<0, 1>,
            coroutine_method::<1, 1>,
            coroutine_method::<2, 1>,
        ),
    ];
    let async_from_sync = vm.new_object_with(Some(async_iterator))?;
    let targets = [
        Intrinsic::GeneratorPrototype,
        Intrinsic::AsyncGeneratorPrototype,
    ];
    let [generator, async_generator] = targets.map(|which| vm.intrinsic(which));
    for (name, sync, asynchronous, from_sync) in methods {
        vm.method(generator, name, 1, sync)?;
        vm.method(async_generator, name, 1, asynchronous)?;
        vm.method(async_from_sync, name, 1, from_sync)?;
    }
    let async_function = vm.new_object_with(Some(intrinsics.function_prototype))?;
    vm.define_to_string_tag(async_function, "AsyncFunction")?;
    vm.state.intrinsics.table[Intrinsic::AsyncFunctionPrototype as usize] = async_function;
    vm.state.intrinsics.table[Intrinsic::AsyncFromSyncIteratorPrototype as usize] = async_from_sync;
    install_constructors(vm)
}

/// %GeneratorFunction%, %AsyncFunction% and %AsyncGeneratorFunction%: not globals, reached
/// through `Object.getPrototypeOf(fn).constructor`.
fn install_constructors(vm: &mut Interpreter) -> JsResult<()> {
    let constructors = [
        (
            "GeneratorFunction",
            "function*",
            Intrinsic::GeneratorFunctionPrototype,
        ),
        (
            "AsyncFunction",
            "async function",
            Intrinsic::AsyncFunctionPrototype,
        ),
        (
            "AsyncGeneratorFunction",
            "async function*",
            Intrinsic::AsyncGeneratorFunctionPrototype,
        ),
    ];
    let function_constructor = vm.intrinsic(Intrinsic::FunctionConstructor);
    for (name, keyword, which) in constructors {
        let call: NativeFn = Rc::new(move |vm, _, args| {
            let fallback = vm.intrinsic(which);
            create_dynamic_function(vm, args, keyword, fallback)
        });
        vm.scoped(|vm| {
            let constructor = vm.create_native_function(name, 1, call, true)?;
            vm.keep(&object_value(constructor));
            vm.set_prototype(constructor, Some(function_constructor))?;
            let proto = vm.intrinsic(which);
            let (prototype, constructor_key) = (
                vm.realm.key_str("prototype"),
                vm.realm.key_str("constructor"),
            );
            let desc = PropertyDescriptor::data(object_value(proto), false, false, false);
            vm.define_property(constructor, prototype, &desc)?;
            let desc = PropertyDescriptor::data(object_value(constructor), false, false, true);
            vm.define_property(proto, constructor_key, &desc)?;
            Ok(())
        })?;
    }
    Ok(())
}

/// `next`/`throw`/`return` (`MODE`) of generators (`FAMILY` 0), async generators (1) and
/// async-from-sync iterators (2).
fn coroutine_method<const FAMILY: u8, const MODE: u8>(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let mode = mode_of(&Value::Number(f64::from(MODE)));
    match FAMILY {
        0 => vm.generator_resume(this, mode, arg(args, 0)),
        1 => vm.async_generator_enqueue(this, mode, arg(args, 0)),
        _ => vm.async_from_sync(this, mode, args),
    }
}
