//! A trivial engine that implements the whole contract using only `velyr-engine-api` types.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use velyr_engine_api::*;
use velyr_runtime_errors::ErrorCode;

enum Entry {
    Object(HashMap<String, Value>),
    Array(Vec<Value>),
    Buffer(Vec<u8>),
    Function(HostFn),
    Promise(ResolverId),
    Module(Source),
}

struct Slot {
    generation: u32,
    entry: Option<Entry>,
}

#[derive(Default)]
struct Heap {
    slots: Vec<Slot>,
    free: Vec<u32>,
    scopes: Vec<Vec<Handle>>,
    promises: Vec<PromiseState>,
    pending_jobs: usize,
    slots_any: HashMap<TypeId, Box<dyn Any>>,
    gc_requests: Vec<GcReason>,
}

impl Heap {
    fn alloc(&mut self, entry: Entry) -> Handle {
        let handle = match self.free.pop() {
            Some(id) => {
                let slot = &mut self.slots[id as usize];
                slot.entry = Some(entry);
                Handle {
                    id,
                    generation: slot.generation,
                }
            }
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    entry: Some(entry),
                });
                Handle {
                    id: self.slots.len() as u32 - 1,
                    generation: 0,
                }
            }
        };
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(handle);
        }
        handle
    }

    fn get_mut(&mut self, handle: Handle) -> Option<&mut Entry> {
        let slot = self.slots.get_mut(handle.id as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.entry.as_mut()
    }

    fn release(&mut self, handle: Handle) {
        let Some(slot) = self.slots.get_mut(handle.id as usize) else {
            return;
        };
        if slot.generation != handle.generation || slot.entry.is_none() {
            return;
        }
        slot.entry = None;
        slot.generation += 1;
        self.free.push(handle.id);
    }

    fn live(&self) -> usize {
        self.slots.iter().filter(|s| s.entry.is_some()).count()
    }

    fn new_promise(&mut self) -> (Value, ResolverId) {
        let resolver = ResolverId(self.promises.len() as u64);
        self.promises.push(PromiseState::Pending);
        (
            Value::Object(self.alloc(Entry::Promise(resolver))),
            resolver,
        )
    }

    fn settle(&mut self, id: ResolverId, state: PromiseState) {
        let slot = &mut self.promises[id.0 as usize];
        if *slot == PromiseState::Pending {
            *slot = state;
            self.pending_jobs += 1;
        }
    }
}

struct Ctx<'a>(&'a mut Heap);

fn bad_handle() -> HostError {
    HostError::TypeError("not an object".into())
}

impl CallContext for Ctx<'_> {
    fn create_string(&mut self, value: JsString) -> Value {
        Value::String(value)
    }
    fn create_object(&mut self) -> Value {
        Value::Object(self.0.alloc(Entry::Object(HashMap::new())))
    }
    fn create_array(&mut self, items: &[Value]) -> Value {
        Value::Object(self.0.alloc(Entry::Array(items.to_vec())))
    }
    fn create_array_buffer(&mut self, bytes: &[u8]) -> Value {
        Value::Object(self.0.alloc(Entry::Buffer(bytes.to_vec())))
    }
    fn get_property(&mut self, object: &Value, key: &str) -> Result<Value, HostError> {
        let Value::Object(h) = object else {
            return Err(bad_handle());
        };
        match self.0.get_mut(*h) {
            Some(Entry::Object(map)) => Ok(map.get(key).cloned().unwrap_or(Value::Undefined)),
            Some(Entry::Array(items)) if key == "length" => Ok(Value::Number(items.len() as f64)),
            Some(Entry::Buffer(bytes)) if key == "byteLength" => {
                Ok(Value::Number(bytes.len() as f64))
            }
            _ => Err(bad_handle()),
        }
    }
    fn set_property(&mut self, object: &Value, key: &str, value: Value) -> Result<(), HostError> {
        let Value::Object(h) = object else {
            return Err(bad_handle());
        };
        match self.0.get_mut(*h) {
            Some(Entry::Object(map)) => {
                map.insert(key.to_owned(), value);
                Ok(())
            }
            _ => Err(bad_handle()),
        }
    }
    fn create_promise(&mut self) -> (Value, ResolverId) {
        self.0.new_promise()
    }
    fn resolve(&mut self, resolver: ResolverId, value: Value) {
        self.0.settle(resolver, PromiseState::Fulfilled(value));
    }
    fn reject(&mut self, resolver: ResolverId, reason: Value) {
        self.0.settle(resolver, PromiseState::Rejected(reason));
    }
    fn throw_type_error(&mut self, message: &str) -> HostError {
        HostError::TypeError(message.to_owned())
    }
    fn slot(&mut self, type_id: TypeId) -> Option<&mut dyn Any> {
        self.0.slots_any.get_mut(&type_id).map(|b| b.as_mut())
    }
}

struct MockEngine;

impl Engine for MockEngine {
    type Isolate = MockIsolate;

    fn features(&self) -> EngineFeatures {
        EngineFeatures::PROMISES | EngineFeatures::MODULES | EngineFeatures::BIGINT
    }

    fn create_isolate(&self, opts: IsolateOptions) -> Result<MockIsolate, EngineError> {
        if opts.heap_limit_bytes == Some(0) {
            return Err(VelyrErrorExt::invalid("heap limit must be positive"));
        }
        Ok(MockIsolate {
            heap: Heap::default(),
            host_fns: HashMap::new(),
            terminated: Arc::new(AtomicBool::new(false)),
        })
    }
}

struct VelyrErrorExt;
impl VelyrErrorExt {
    fn invalid(msg: &str) -> EngineError {
        velyr_runtime_errors::VelyrError::invalid_data(msg).into()
    }
}

struct MockIsolate {
    heap: Heap,
    host_fns: HashMap<&'static str, HostFn>,
    terminated: Arc<AtomicBool>,
}

fn exception(message: &str) -> JsException {
    JsException {
        message: message.to_owned(),
        stack: None,
        value: None,
    }
}

impl MockIsolate {
    fn invoke(&mut self, f: HostFn, args: &[Value]) -> Result<Value, JsException> {
        f(&mut Ctx(&mut self.heap), args).map_err(|e| match e {
            HostError::TypeError(m) => exception(&format!("TypeError: {m}")),
            HostError::Thrown(v) => JsException {
                message: "thrown".into(),
                stack: None,
                value: Some(v),
            },
            other => exception(&velyr_runtime_errors::VelyrError::from(other).to_string()),
        })
    }

    fn eval_expr(&mut self, src: &str) -> Result<Value, JsException> {
        let src = src.trim().trim_end_matches(';').trim();
        match src {
            "undefined" => return Ok(Value::Undefined),
            "null" => return Ok(Value::Null),
            _ => {}
        }
        if let Ok(n) = src.parse::<f64>() {
            return Ok(Value::Number(n));
        }
        if let Some(open) = src.find('(').filter(|_| src.ends_with(')')) {
            let f = self.lookup(&src[..open])?;
            let args = src[open + 1..src.len() - 1]
                .split(',')
                .filter(|a| !a.trim().is_empty())
                .map(|a| self.eval_expr(a))
                .collect::<Result<Vec<_>, _>>()?;
            return self.invoke(f, &args);
        }
        let f = self.lookup(src)?;
        Ok(Value::Object(self.heap.alloc(Entry::Function(f))))
    }

    fn lookup(&self, name: &str) -> Result<HostFn, JsException> {
        self.host_fns
            .get(name.trim())
            .copied()
            .ok_or_else(|| exception(&format!("ReferenceError: {} is not defined", name.trim())))
    }
}

impl Isolate for MockIsolate {
    fn eval_script(&mut self, source: &Source) -> Result<Value, JsException> {
        if self.terminated.load(Ordering::SeqCst) {
            return Err(exception("terminated"));
        }
        self.eval_expr(&source.text)
    }

    fn compile_module(&mut self, source: &Source) -> Result<Handle, JsException> {
        Ok(self.heap.alloc(Entry::Module(source.clone())))
    }

    fn instantiate_module(
        &mut self,
        module: Handle,
        hook: &mut dyn ModuleResolveHook,
    ) -> Result<(), JsException> {
        let Some(Entry::Module(src)) = self.heap.get_mut(module) else {
            return Err(exception("not a module"));
        };
        let referrer = src.name.clone();
        let specifiers: Vec<String> = src
            .text
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix("import \"")?
                    .strip_suffix("\";")
                    .map(str::to_owned)
            })
            .collect();
        for specifier in specifiers {
            hook.resolve(&specifier, &referrer)
                .map_err(|e| exception(&e.to_string()))?;
        }
        Ok(())
    }

    fn evaluate_module(&mut self, module: Handle) -> Result<Value, JsException> {
        let Some(Entry::Module(src)) = self.heap.get_mut(module) else {
            return Err(exception("not a module"));
        };
        let body = src.text.clone();
        let (promise, resolver) = self.heap.new_promise();
        let mut last = Value::Undefined;
        for line in body.lines().filter(|l| !l.trim().starts_with("import ")) {
            match self.eval_expr(line) {
                Ok(v) => last = v,
                Err(e) => {
                    self.heap.settle(
                        resolver,
                        PromiseState::Rejected(Value::String(e.message.as_str().into())),
                    );
                    return Ok(promise);
                }
            }
        }
        self.heap.settle(resolver, PromiseState::Fulfilled(last));
        Ok(promise)
    }

    fn call(&mut self, func: &Value, _this: &Value, args: &[Value]) -> Result<Value, JsException> {
        let Value::Object(h) = func else {
            return Err(exception("TypeError: not a function"));
        };
        match self.heap.get_mut(*h) {
            Some(Entry::Function(f)) => {
                let f = *f;
                self.invoke(f, args)
            }
            _ => Err(exception("TypeError: not a function")),
        }
    }

    fn register_host_function(&mut self, def: HostFunctionDef) -> Result<(), EngineError> {
        if self.host_fns.insert(def.name, def.callback).is_some() {
            return Err(velyr_runtime_errors::VelyrError::already_exists(def.name).into());
        }
        Ok(())
    }

    fn set_slot<T: Any>(&mut self, value: T) {
        self.heap
            .slots_any
            .insert(TypeId::of::<T>(), Box::new(value));
    }

    fn create_promise(&mut self) -> (Value, ResolverId) {
        self.heap.new_promise()
    }
    fn resolve(&mut self, resolver: ResolverId, value: Value) {
        self.heap.settle(resolver, PromiseState::Fulfilled(value));
    }
    fn reject(&mut self, resolver: ResolverId, reason: Value) {
        self.heap.settle(resolver, PromiseState::Rejected(reason));
    }
    fn promise_state(&mut self, promise: &Value) -> Option<PromiseState> {
        let Value::Object(h) = promise else {
            return None;
        };
        match self.heap.get_mut(*h)? {
            Entry::Promise(r) => {
                let r = *r;
                Some(self.heap.promises[r.0 as usize].clone())
            }
            _ => None,
        }
    }

    fn perform_microtask_checkpoint(&mut self) -> MicrotaskProgress {
        if std::mem::take(&mut self.heap.pending_jobs) == 0 {
            MicrotaskProgress::Idle
        } else {
            MicrotaskProgress::Progressed
        }
    }

    fn heap_stats(&self) -> HeapStats {
        let used = self.heap.live() * 64;
        HeapStats {
            used_bytes: used,
            total_bytes: used.max(4096),
            limit_bytes: None,
            ..HeapStats::default()
        }
    }

    fn request_gc(&mut self, reason: GcReason) {
        self.heap.gc_requests.push(reason);
    }

    fn termination_handle(&self) -> TerminationHandle {
        let flag = Arc::clone(&self.terminated);
        TerminationHandle::new(move || flag.store(true, Ordering::SeqCst))
    }

    fn release(&mut self, handle: Handle) {
        self.heap.release(handle);
    }

    fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.heap.scopes.push(Vec::new());
        let out = f(self);
        for h in self.heap.scopes.pop().unwrap_or_default() {
            self.heap.release(h);
        }
        out
    }
}

// ---- helpers and tests ----

fn native_add(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    match args {
        [Value::Number(a), Value::Number(b)] => {
            if let Some(calls) = ctx.isolate_slot::<u32>() {
                *calls += 1;
            }
            Ok(Value::Number(a + b))
        }
        _ => Err(ctx.throw_type_error("nativeAdd expects two numbers")),
    }
}

fn make_object(ctx: &mut dyn CallContext, _args: &[Value]) -> Result<Value, HostError> {
    let obj = ctx.create_object();
    ctx.set_property(&obj, "answer", Value::Number(42.0))?;
    Ok(obj)
}

fn isolate() -> MockIsolate {
    let mut iso = MockEngine
        .create_isolate(IsolateOptions::default())
        .unwrap();
    iso.register_host_function(HostFunctionDef {
        name: "nativeAdd",
        arity: 2,
        callback: native_add,
    })
    .unwrap();
    iso.register_host_function(HostFunctionDef {
        name: "makeObject",
        arity: 0,
        callback: make_object,
    })
    .unwrap();
    iso
}

fn src(text: &str) -> Source {
    Source {
        name: "test.js".into(),
        text: text.into(),
    }
}

#[test]
fn call_context_is_object_safe() {
    fn _assert(_: &mut dyn CallContext) {}
    fn _assert_hook(_: &mut dyn ModuleResolveHook) {}
}

#[test]
fn termination_handle_is_send_sync() {
    fn assert_send_sync<T: Send + Sync + Clone>() {}
    assert_send_sync::<TerminationHandle>();
}

#[test]
fn eval_literal_and_native_add() {
    let mut iso = isolate();
    assert_eq!(iso.eval_script(&src("42")).unwrap(), Value::Number(42.0));
    assert_eq!(
        iso.eval_script(&src("nativeAdd(1, 2)")).unwrap(),
        Value::Number(3.0)
    );
    let err = iso.eval_script(&src("nativeAdd(1, null)")).unwrap_err();
    assert!(err.message.contains("TypeError"));
    assert!(iso.eval_script(&src("nope")).is_err());
}

#[test]
fn engine_declares_features_and_rejects_bad_options() {
    let f = MockEngine.features();
    assert!(f.contains(EngineFeatures::MODULES | EngineFeatures::PROMISES));
    assert!(!f.contains(EngineFeatures::SERIALIZE));
    let err = MockEngine
        .create_isolate(IsolateOptions {
            heap_limit_bytes: Some(0),
        })
        .err()
        .unwrap();
    assert_eq!(err.code(), ErrorCode::InvalidData);
}

#[test]
fn default_write_string_utf8_appends_lossy_utf8() {
    let mut heap = Heap::default();
    let mut ctx = Ctx(&mut heap);
    let ctx: &mut dyn CallContext = &mut ctx;
    let mut out = b"<".to_vec();
    ctx.write_string_utf8(&JsString::from_str("hi"), &mut out);
    ctx.write_string_utf8(&JsString::from_str("\u{e9}\u{1F600}"), &mut out);
    ctx.write_string_utf8(&JsString::from_utf16(vec![0x61, 0xD800, 0x62]), &mut out);
    assert_eq!(out, "<hi\u{e9}\u{1F600}a\u{FFFD}b".as_bytes());
}

#[test]
fn host_function_reads_isolate_slot() {
    let mut iso = isolate();
    iso.set_slot(0u32);
    iso.eval_script(&src("nativeAdd(1, 2)")).unwrap();
    iso.eval_script(&src("nativeAdd(3, 4)")).unwrap();
    let mut ctx = Ctx(&mut iso.heap);
    let ctx: &mut dyn CallContext = &mut ctx;
    assert_eq!(ctx.isolate_slot::<u32>().copied(), Some(2));
    assert!(ctx.isolate_slot::<String>().is_none());
}

#[test]
fn duplicate_host_function_is_already_exists() {
    let mut iso = isolate();
    let err = iso
        .register_host_function(HostFunctionDef {
            name: "nativeAdd",
            arity: 2,
            callback: native_add,
        })
        .unwrap_err();
    assert_eq!(err.code(), ErrorCode::AlreadyExists);
}

#[test]
fn call_invokes_function_value() {
    let mut iso = isolate();
    let f = iso.eval_script(&src("nativeAdd")).unwrap();
    let out = iso
        .call(
            &f,
            &Value::Undefined,
            &[Value::Number(2.0), Value::Number(5.0)],
        )
        .unwrap();
    assert_eq!(out, Value::Number(7.0));
    assert!(
        iso.call(&Value::Number(1.0), &Value::Undefined, &[])
            .is_err()
    );
}

#[test]
fn scope_releases_handles_and_stale_handles_are_rejected() {
    let mut iso = isolate();
    let kept = iso.eval_script(&src("nativeAdd")).unwrap();
    let before = iso.heap_stats().used_bytes;
    let leaked = iso.scope(|iso| {
        let f = iso.eval_script(&src("nativeAdd")).unwrap();
        let obj = iso.eval_script(&src("makeObject()")).unwrap();
        assert!(iso.heap_stats().used_bytes > before);
        (f, obj)
    });
    assert_eq!(iso.heap_stats().used_bytes, before);
    assert!(
        iso.call(
            &leaked.0,
            &Value::Undefined,
            &[Value::Number(1.0), Value::Number(1.0)]
        )
        .is_err()
    );
    assert!(
        iso.call(
            &kept,
            &Value::Undefined,
            &[Value::Number(1.0), Value::Number(1.0)]
        )
        .is_ok()
    );
    let Value::Object(h) = kept else {
        unreachable!()
    };
    iso.release(h);
    assert!(iso.call(&kept, &Value::Undefined, &[]).is_err());
}

#[test]
fn promises_resolve_reject_and_drain() {
    let mut iso = isolate();
    let (p, r) = iso.create_promise();
    assert_eq!(iso.promise_state(&p), Some(PromiseState::Pending));
    assert_eq!(iso.perform_microtask_checkpoint(), MicrotaskProgress::Idle);
    iso.resolve(r, Value::Number(1.0));
    iso.reject(r, Value::Null); // settling twice is ignored
    assert_eq!(
        iso.promise_state(&p),
        Some(PromiseState::Fulfilled(Value::Number(1.0)))
    );
    assert_eq!(
        iso.perform_microtask_checkpoint(),
        MicrotaskProgress::Progressed
    );
    assert_eq!(iso.perform_microtask_checkpoint(), MicrotaskProgress::Idle);
    let (q, r2) = iso.create_promise();
    iso.reject(r2, Value::Bool(true));
    assert_eq!(
        iso.promise_state(&q),
        Some(PromiseState::Rejected(Value::Bool(true)))
    );
}

struct Resolver(Vec<(String, String)>);
impl ModuleResolveHook for Resolver {
    fn resolve(&mut self, specifier: &str, referrer: &str) -> Result<Source, HostError> {
        self.0.push((specifier.to_owned(), referrer.to_owned()));
        if specifier == "missing" {
            return Err(velyr_runtime_errors::VelyrError::not_found("missing").into());
        }
        Ok(Source {
            name: specifier.to_owned(),
            text: String::new(),
        })
    }
}

#[test]
fn module_lifecycle_through_hook() {
    let mut iso = isolate();
    let m = iso
        .compile_module(&Source {
            name: "main.mjs".into(),
            text: "import \"dep\";\nnativeAdd(20, 22)".into(),
        })
        .unwrap();
    let mut hook = Resolver(Vec::new());
    iso.instantiate_module(m, &mut hook).unwrap();
    assert_eq!(hook.0, vec![("dep".to_owned(), "main.mjs".to_owned())]);
    let p = iso.evaluate_module(m).unwrap();
    assert_eq!(
        iso.promise_state(&p),
        Some(PromiseState::Fulfilled(Value::Number(42.0)))
    );

    let bad = iso
        .compile_module(&Source {
            name: "b.mjs".into(),
            text: "import \"missing\";".into(),
        })
        .unwrap();
    assert!(
        iso.instantiate_module(bad, &mut Resolver(Vec::new()))
            .is_err()
    );
}

#[test]
fn host_object_properties_and_gc_and_termination() {
    let mut iso = isolate();
    let obj = iso.eval_script(&src("makeObject()")).unwrap();
    let mut ctx = Ctx(&mut iso.heap);
    assert_eq!(
        ctx.get_property(&obj, "answer").unwrap(),
        Value::Number(42.0)
    );
    assert_eq!(ctx.get_property(&obj, "other").unwrap(), Value::Undefined);
    assert!(ctx.get_property(&Value::Null, "x").is_err());
    let buf = ctx.create_array_buffer(&[1, 2, 3]);
    assert_eq!(
        ctx.get_property(&buf, "byteLength").unwrap(),
        Value::Number(3.0)
    );

    iso.request_gc(GcReason::Explicit);
    assert_eq!(iso.heap.gc_requests, vec![GcReason::Explicit]);

    let handle = iso.termination_handle();
    std::thread::spawn(move || handle.terminate())
        .join()
        .unwrap();
    assert!(iso.eval_script(&src("1")).is_err());
}

#[test]
fn errors_convert_to_velyr_error() {
    let e: velyr_runtime_errors::VelyrError = HostError::TypeError("x".into()).into();
    assert_eq!(e.code(), ErrorCode::InvalidData);
    let inner = velyr_runtime_errors::VelyrError::quota_exceeded("heap");
    let host: HostError = inner.into();
    assert_eq!(
        velyr_runtime_errors::VelyrError::from(host).code(),
        ErrorCode::QuotaExceeded
    );
    let engine: EngineError = velyr_runtime_errors::VelyrError::unsupported("x").into();
    assert_eq!(engine.code(), ErrorCode::Unsupported);
    assert_eq!(
        velyr_runtime_errors::VelyrError::from(engine).code(),
        ErrorCode::Unsupported
    );
}

#[test]
fn continuation_context_is_unsupported_unless_an_engine_opts_in() {
    assert!(
        !MockEngine
            .features()
            .contains(EngineFeatures::CONTINUATION_CONTEXT)
    );
    let mut iso = MockEngine
        .create_isolate(IsolateOptions::default())
        .unwrap();
    iso.set_continuation_context(Value::Number(1.0));
    assert_eq!(iso.get_continuation_context(), Value::Undefined);
}

#[test]
fn features_bitset_ops() {
    let mut f = EngineFeatures::empty();
    assert!(f.is_empty());
    f |= EngineFeatures::BIGINT;
    assert!(f.contains(EngineFeatures::BIGINT));
    assert!(!f.contains(EngineFeatures::BIGINT | EngineFeatures::MODULES));
    assert_eq!(
        (f | EngineFeatures::MODULES) & EngineFeatures::MODULES,
        EngineFeatures::MODULES
    );
}
