//! The `WeakRef` and `FinalizationRegistry` constructors and prototypes (ECMA-262 §26.1,
//! §26.2). Their weak state lives in heap cells of its own, so the collector clears dead
//! targets and queues registries with pending cleanups; the callbacks run later as jobs
//! (`run_finalizers`), never inside the GC.

use velyr_engine_gc::{FinalizationRegistry, Gc, Liveness, Trace, Tracer, WeakRef};

use super::slots::Slot;
use super::weak::{WeakKey, weak_key};
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::interpreter::Abrupt;
use crate::{SymbolId, Value, same_value};

/// `[[WeakRefTarget]]`. Symbols are not heap cells, so a symbol target never dies.
#[derive(Clone, Copy)]
pub(crate) enum WeakTarget {
    Object(Gc<WeakRef>),
    Symbol(SymbolId),
}

/// A `[[Cells]]` record minus its target. The token is held weakly: it is never traced,
/// and a dead object token cannot alias a new object (handles carry a generation).
pub(crate) struct Registration {
    held: Value,
    token: Value,
}

impl Trace for Registration {
    fn trace(&self, tracer: &mut Tracer) {
        self.held.trace(tracer);
    }
}

pub(crate) struct RegistryCell {
    callback: Value,
    objects: FinalizationRegistry<Registration>,
    /// Registrations with a symbol target, which only `unregister` removes.
    symbols: Vec<Registration>,
}

impl Trace for RegistryCell {
    fn trace(&self, tracer: &mut Tracer) {
        self.callback.trace(tracer);
        self.objects.trace(tracer);
        self.symbols.trace(tracer);
    }

    fn clear_dead_refs(&mut self, liveness: &Liveness) -> bool {
        self.objects.clear_dead_refs(liveness)
    }
}

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::WeakRefPrototype);
    vm.constructor("WeakRef", 1, weak_ref_constructor, proto)?;
    vm.method(proto, "deref", 0, weak_ref_deref)?;
    vm.define_to_string_tag(proto, "WeakRef")?;
    let proto = vm.intrinsic(Intrinsic::FinalizationRegistryPrototype);
    vm.constructor("FinalizationRegistry", 1, registry_constructor, proto)?;
    vm.method(proto, "register", 2, register)?;
    vm.method(proto, "unregister", 1, unregister)?;
    vm.define_to_string_tag(proto, "FinalizationRegistry")
}

/// AddToKeptObjects: the target survives until the running job ends.
fn keep_until_job_ends(vm: &mut Interpreter, target: &Value) {
    if let Value::Object(target) = target {
        vm.state.kept.insert(*target);
    }
}

fn weak_ref_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        return vm.throw_type("Constructor WeakRef requires 'new'");
    }
    let target = arg(args, 0);
    let Some(key) = weak_key(vm, &target) else {
        return vm.throw_type("WeakRef: invalid target");
    };
    let proto = vm.intrinsic(Intrinsic::WeakRefPrototype);
    let object = vm.create_from_new_target(&new_target, proto, None)?;
    let slot = match key {
        WeakKey::Object(target) => {
            WeakTarget::Object(Gc::from_raw(vm.alloc_cell(WeakRef::new(target))?))
        }
        WeakKey::Symbol(symbol) => WeakTarget::Symbol(symbol),
    };
    vm.set_slot(object, Slot::WeakRef(slot));
    keep_until_job_ends(vm, &target);
    Ok(object_value(object))
}

fn weak_ref_deref(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let Some(Slot::WeakRef(target)) = vm.value_slot(this) else {
        return vm.throw_type("Method WeakRef.prototype.deref called on incompatible receiver");
    };
    let target = match target {
        WeakTarget::Object(cell) => match vm.realm.heap.get(cell).and_then(WeakRef::deref) {
            Some(target) => Value::Object(target),
            None => return Ok(Value::Undefined),
        },
        WeakTarget::Symbol(symbol) => Value::Symbol(symbol),
    };
    keep_until_job_ends(vm, &target);
    Ok(target)
}

fn registry_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        return vm.throw_type("Constructor FinalizationRegistry requires 'new'");
    }
    let callback = arg(args, 0);
    if !vm.is_callable(&callback) {
        return vm.throw_type("FinalizationRegistry: cleanup must be callable");
    }
    let proto = vm.intrinsic(Intrinsic::FinalizationRegistryPrototype);
    let object = vm.create_from_new_target(&new_target, proto, None)?;
    let cell = RegistryCell {
        callback,
        objects: FinalizationRegistry::new(),
        symbols: Vec::new(),
    };
    let cell = Gc::from_raw(vm.alloc_cell(cell)?);
    vm.set_slot(object, Slot::FinalizationRegistry(cell));
    Ok(object_value(object))
}

fn this_registry(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Gc<RegistryCell>> {
    match vm.value_slot(this) {
        Some(Slot::FinalizationRegistry(cell)) => Ok(cell),
        _ => vm.throw_type(&format!(
            "Method FinalizationRegistry.prototype.{method} called on incompatible receiver"
        )),
    }
}

fn register(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let cell = this_registry(vm, this, "register")?;
    let (target, held, token) = (arg(args, 0), arg(args, 1), arg(args, 2));
    let Some(key) = weak_key(vm, &target) else {
        return vm.throw_type("FinalizationRegistry.prototype.register: invalid target");
    };
    if same_value(&target, &held) {
        return vm.throw_type(
            "FinalizationRegistry.prototype.register: target and holdings must not be same",
        );
    }
    if weak_key(vm, &token).is_none() && !matches!(token, Value::Undefined) {
        return vm.throw_type("FinalizationRegistry.prototype.register: invalid unregister token");
    }
    let registration = Registration { held, token };
    let registry = vm.realm.heap.get_mut(cell).expect("registry is live");
    match key {
        WeakKey::Object(target) => registry.objects.register(target, registration),
        WeakKey::Symbol(_) => registry.symbols.push(registration),
    }
    Ok(Value::Undefined)
}

fn unregister(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let cell = this_registry(vm, this, "unregister")?;
    let token = arg(args, 0);
    if weak_key(vm, &token).is_none() {
        return vm
            .throw_type("FinalizationRegistry.prototype.unregister: invalid unregister token");
    }
    let registry = vm.realm.heap.get_mut(cell).expect("registry is live");
    let mut removed = false;
    let mut keep = |registration: &Registration| {
        let hit = same_value(&registration.token, &token);
        removed |= hit;
        !hit
    };
    registry.objects.retain(&mut keep);
    registry.symbols.retain(|registration| keep(registration));
    Ok(Value::Bool(removed))
}

impl Interpreter {
    /// HostCleanupFinalizationRegistry for every registry the collector queued; returns
    /// whether a callback ran. A callback's throw is dropped, like a job's.
    /// shortcut: cleanups still pending when a callback terminates the isolate are lost,
    /// which is fine while termination ends the isolate.
    pub(crate) fn run_finalizers(&mut self) -> Result<bool, Abrupt> {
        let mut ran = false;
        for registry in self.realm.heap.take_finalization_queue() {
            // Dead if an earlier callback dropped the last reference to its registry.
            let registry = Gc::<RegistryCell>::from_raw(registry);
            let Some(cell) = self.realm.heap.get_mut(registry) else {
                continue;
            };
            let callback = cell.callback.clone();
            let cleanups = cell.objects.take_cleanups();
            let held: Vec<Value> = cleanups.into_iter().map(|r| r.held).collect();
            let mark = self.state.scratch.len();
            self.state.scratch.push(callback.clone());
            self.state.scratch.extend(held.iter().cloned());
            let mut result = Ok(());
            for value in held {
                ran = true;
                match self.call_value(&callback, &Value::Undefined, &[value]) {
                    Ok(_) | Err(Abrupt::Throw(_)) => {}
                    Err(abrupt) => {
                        result = Err(abrupt);
                        break;
                    }
                }
            }
            self.state.scratch.truncate(mark);
            result?;
        }
        Ok(ran)
    }
}
