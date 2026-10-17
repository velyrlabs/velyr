use std::any::{Any, TypeId};
use std::sync::Arc;
use std::time::Duration;

use velyr_runtime_errors::VelyrError;

use crate::{
    CodeCacheStats, EngineError, EngineFeatures, Handle, HostError, JsString, NativeScopeFn,
    NativeTeardown, SerializedValue, TickFlags, Value,
};

/// Script or module source. `name` is the resolved specifier/URL used in stack traces
/// and as the module identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub text: String,
}

/// Options for [`Engine::create_isolate`].
#[derive(Debug, Clone, Default)]
pub struct IsolateOptions {
    /// Hard heap limit; `None` means the engine default.
    pub heap_limit_bytes: Option<usize>,
}

/// A JS exception: the engine-side error plus, when available, the thrown value.
#[derive(Debug, Clone, PartialEq)]
pub struct JsException {
    pub message: String,
    /// Engine-specific format; never parse it (ADR-009, no V8-only semantics).
    pub stack: Option<String>,
    /// The thrown value, valid in the isolate that produced it.
    pub value: Option<Value>,
}

impl JsException {
    /// Exception an adapter returns when the engine stopped the isolate for exceeding a
    /// resource quota (e.g. heap). The class travels in the message: the contract has no flag.
    pub fn quota_exceeded(class: &str) -> Self {
        Self {
            message: format!("{QUOTA_EXCEEDED_PREFIX}{class}"),
            stack: None,
            value: None,
        }
    }

    /// The quota class when this exception came from [`JsException::quota_exceeded`].
    pub fn quota_class(&self) -> Option<&str> {
        self.message.strip_prefix(QUOTA_EXCEEDED_PREFIX)
    }

    /// Exception for an operation the engine does not implement (e.g. `check_script`).
    pub fn unsupported(what: &str) -> Self {
        Self {
            message: format!("{UNSUPPORTED_PREFIX}{what} is not supported by this engine"),
            stack: None,
            value: None,
        }
    }

    /// True when this exception came from [`JsException::unsupported`].
    pub fn is_unsupported(&self) -> bool {
        self.message.starts_with(UNSUPPORTED_PREFIX)
    }
}

const QUOTA_EXCEEDED_PREFIX: &str = "VELYR_QUOTA_EXCEEDED: ";
const UNSUPPORTED_PREFIX: &str = "VELYR_UNSUPPORTED: ";

/// Opaque id of the settle side of a promise created by `create_promise`.
/// Resolving or rejecting twice is a no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolverId(pub u64);

/// Settlement state of a promise.
#[derive(Debug, Clone, PartialEq)]
pub enum PromiseState {
    Pending,
    Fulfilled(Value),
    Rejected(Value),
}

/// Result of [`Isolate::perform_microtask_checkpoint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrotaskProgress {
    /// The queue was empty.
    Idle,
    /// At least one microtask ran; the queue is now drained.
    Progressed,
}

/// Why the runtime asks for a GC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcReason {
    MemoryPressure,
    Idle,
    Explicit,
}

/// Heap accounting snapshot, in bytes. `None` where the engine has no such reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeapStats {
    pub used_bytes: usize,
    pub total_bytes: usize,
    pub limit_bytes: Option<usize>,
    /// Memory held outside the GC heap on the engine's behalf (strings, buffers, ...).
    pub external_bytes: Option<usize>,
    /// Live GC cells.
    pub cells: Option<usize>,
    /// Collections so far.
    pub gc_count: Option<u64>,
    /// Total time spent paused in collections.
    pub gc_time: Option<Duration>,
    /// Where the engine's memory goes, for engines that track it.
    pub breakdown: Option<MemoryBreakdown>,
}

/// Engine memory by kind, in bytes; `strings` and `storage` make up `external_bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemoryBreakdown {
    /// GC cells.
    pub cells: usize,
    /// The collector's own slot tables.
    pub heap_tables: usize,
    pub strings: usize,
    /// Object property and element storage.
    pub storage: usize,
    /// Bytecode, constant pools, line tables and inline caches.
    pub code: usize,
    /// Source text kept for `Function.prototype.toString` and stack traces.
    pub source: usize,
    pub shapes: usize,
    /// The property-name atom table.
    pub interner: usize,
    /// Engine side tables: weak maps, closure records, per-code info.
    pub tables: usize,
}

impl MemoryBreakdown {
    /// Label and bytes of every kind, in declaration order.
    pub fn kinds(&self) -> [(&'static str, usize); 9] {
        [
            ("cells", self.cells),
            ("heap_tables", self.heap_tables),
            ("strings", self.strings),
            ("storage", self.storage),
            ("code", self.code),
            ("source", self.source),
            ("shapes", self.shapes),
            ("interner", self.interner),
            ("tables", self.tables),
        ]
    }
}

/// Native function exposed to JS. Called on the isolate thread, synchronously.
///
/// Invariants: `args` and any [`Value::Object`] it holds are valid only for the
/// duration of the call unless the callee keeps them alive through the engine.
/// Returned handles are owned by the caller scope.
pub type HostFn = fn(&mut dyn CallContext, &[Value]) -> Result<Value, HostError>;

/// Registration record for [`Isolate::register_host_function`].
#[derive(Debug, Clone, Copy)]
pub struct HostFunctionDef {
    /// Unique name (`op_<area>_<verb>` per ADR-010).
    pub name: &'static str,
    /// Declared parameter count (`Function.length`).
    pub arity: u8,
    pub callback: HostFn,
}

/// Resolves module specifiers during [`Isolate::instantiate_module`].
pub trait ModuleResolveHook {
    /// Called once per import of `referrer`. Returns the source of the resolved
    /// module; `Source::name` is its identity, so the engine compiles each
    /// distinct name once. An error aborts instantiation with a JS exception.
    fn resolve(&mut self, specifier: &str, referrer: &str) -> Result<Source, HostError>;

    /// Sealed code cache (see [`seal_code_cache`](crate::seal_code_cache)) for the module that
    /// `resolve` just returned as `name`, if the host has one. The engine falls back to a normal
    /// compilation when the bytes are missing or rejected.
    fn code_cache(&mut self, _name: &str) -> Option<Arc<[u8]>> {
        None
    }
}

/// What a [`HostFn`] may do to the isolate that is calling it.
///
/// Object-safe on purpose: ops receive `&mut dyn CallContext` (ADR-010). Only
/// the owning isolate thread ever holds one.
pub trait CallContext {
    fn create_string(&mut self, value: JsString) -> Value;
    fn create_object(&mut self) -> Value;
    fn create_array(&mut self, items: &[Value]) -> Value;
    /// Copies `bytes` into a new `ArrayBuffer`.
    fn create_array_buffer(&mut self, bytes: &[u8]) -> Value;
    /// Copies the bytes of an `ArrayBuffer` or any `ArrayBufferView` (`Uint8Array`, `DataView`,
    /// ...; a view yields only its `byteOffset..byteOffset + byteLength` window). `None` for any
    /// other value. Adapters without buffers keep the default.
    fn read_bytes(&mut self, _value: &Value) -> Option<Vec<u8>> {
        None
    }
    /// Appends `text` to `out` as UTF-8, lone surrogates as U+FFFD (what `TextEncoder` yields).
    /// Engines override it with a faster transcoder; the default goes through a `String`.
    fn write_string_utf8(&mut self, text: &JsString, out: &mut Vec<u8>) {
        out.extend_from_slice(text.to_string_lossy().as_bytes());
    }
    /// Fails if `object` is not an object or the handle is stale.
    fn get_property(&mut self, object: &Value, key: &str) -> Result<Value, HostError>;
    fn set_property(&mut self, object: &Value, key: &str, value: Value) -> Result<(), HostError>;
    fn create_promise(&mut self) -> (Value, ResolverId);
    fn resolve(&mut self, resolver: ResolverId, value: Value);
    fn reject(&mut self, resolver: ResolverId, reason: Value);
    /// Builds the error a host function returns to throw a JS `TypeError`:
    /// `return Err(ctx.throw_type_error("..."))`.
    fn throw_type_error(&mut self, message: &str) -> HostError;
    /// See [`Isolate::serialize`]. Errors surface as [`HostError::Thrown`].
    fn serialize(
        &mut self,
        _value: &Value,
        _transfer: &[Value],
    ) -> Result<SerializedValue, HostError> {
        Err(HostError::TypeError(
            JsException::serialize_unsupported().message,
        ))
    }
    /// See [`Isolate::deserialize`]. Errors surface as [`HostError::Thrown`].
    fn deserialize(&mut self, _serialized: &SerializedValue) -> Result<Value, HostError> {
        Err(HostError::TypeError(
            JsException::serialize_unsupported().message,
        ))
    }
    /// See [`Isolate::compile_module`]. Lets a host function load a module synchronously
    /// (`require(esm)`). Adapters without module support keep the default.
    fn compile_module(&mut self, _source: &Source) -> Result<Handle, HostError> {
        Err(unsupported_modules())
    }
    /// See [`Isolate::instantiate_module`]. No JS runs while linking, only `hook`.
    fn instantiate_module(
        &mut self,
        _module: Handle,
        _hook: &mut dyn ModuleResolveHook,
    ) -> Result<(), HostError> {
        Err(unsupported_modules())
    }
    /// See [`Isolate::evaluate_module`]. Returns the evaluation promise; it is already settled
    /// when the graph has no top-level await. No microtask checkpoint runs here.
    fn evaluate_module(&mut self, _module: Handle) -> Result<Value, HostError> {
        Err(unsupported_modules())
    }
    /// Namespace object of an instantiated module.
    fn module_namespace(&mut self, _module: Handle) -> Result<Value, HostError> {
        Err(unsupported_modules())
    }
    /// Compiles and runs `source` as a classic script in the current realm (shares the global).
    /// Errors surface as [`HostError::Thrown`].
    fn eval_script(&mut self, _source: &Source) -> Result<Value, HostError> {
        Err(unsupported_hook("script evaluation"))
    }
    /// Queues `func` (a function) on the engine's own microtask queue; a throw inside it is the
    /// callback's problem, not a promise rejection. Returns `false` when the engine has no such
    /// queue (the default), so the caller can fall back.
    fn enqueue_microtask(&mut self, _func: &Value) -> Result<bool, HostError> {
        Ok(false)
    }
    /// Drains the engine's microtask queue. A no-op when one is already draining.
    fn run_microtask_checkpoint(&mut self) {}
    /// Asks the engine for a garbage collection (same intent as [`Isolate::request_gc`]).
    fn request_gc(&mut self) -> Result<(), HostError> {
        Err(unsupported_hook("garbage collection requests"))
    }
    /// See [`Isolate::promise_state`].
    fn promise_state(&mut self, _promise: &Value) -> Option<PromiseState> {
        None
    }
    /// See [`Isolate::set_continuation_context`].
    fn set_continuation_context(&mut self, _value: Value) {}
    /// See [`Isolate::get_continuation_context`].
    fn get_continuation_context(&mut self) -> Value {
        Value::Undefined
    }
    /// Runs `f` with the engine's own scope and converts the raw value it returns, for FFI
    /// layers (Node-API) that work on engine handles. Engines without raw access keep the
    /// default, which fails without calling `f`.
    fn with_native_scope(&mut self, _f: &mut NativeScopeFn<'_>) -> Result<Value, HostError> {
        Err(unsupported_native())
    }
    /// Registers `teardown` to run in a native scope when the isolate is disposed, before its
    /// handles are freed. Registering the same function again is a no-op.
    fn on_native_teardown(&mut self, _teardown: NativeTeardown) -> Result<(), HostError> {
        Err(unsupported_native())
    }
    /// Exposes `flags` to JS as a two-byte `Uint8Array` sharing its memory; `None` when the
    /// engine cannot (the default), so the loop keeps calling the tick hook unconditionally.
    fn bind_tick_flags(&mut self, _flags: &TickFlags) -> Option<Value> {
        None
    }
    /// Raw slot lookup by type id, as registered with [`Isolate::set_slot`].
    /// Prefer the typed `isolate_slot::<T>()`.
    fn slot(&mut self, type_id: TypeId) -> Option<&mut dyn Any>;
}

impl dyn CallContext + '_ {
    /// Runtime state stored on the isolate with [`Isolate::set_slot`].
    pub fn isolate_slot<T: Any>(&mut self) -> Option<&mut T> {
        self.slot(TypeId::of::<T>())?.downcast_mut()
    }
}

/// Cross-thread kill switch for one isolate. The only `Send + Sync` piece of the contract.
#[derive(Clone)]
pub struct TerminationHandle(Arc<dyn Fn() + Send + Sync>);

impl TerminationHandle {
    /// Adapters wrap their engine's thread-safe terminate call.
    pub fn new(terminate: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(terminate))
    }

    /// Asks the isolate to stop running JS as soon as possible. Callable from any
    /// thread, any number of times; running JS observes an uncatchable termination.
    pub fn terminate(&self) {
        (self.0)();
    }
}

impl std::fmt::Debug for TerminationHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TerminationHandle")
    }
}

/// Factory for isolates; one per engine adapter (`engine-v8`, `engine-native`).
pub trait Engine: 'static {
    type Isolate: Isolate;

    /// Optional capabilities this adapter supports.
    fn features(&self) -> EngineFeatures;

    /// Creates an isolate with its own heap and realm. The caller keeps it on
    /// the creating thread.
    fn create_isolate(&self, opts: IsolateOptions) -> Result<Self::Isolate, EngineError>;

    /// `true` when isolates boot from a startup snapshot that already holds the host functions
    /// and bootstrap scripts: the host must not register those again, only its isolate slots.
    fn boots_from_snapshot(&self) -> bool {
        false
    }
}

/// One JS heap and realm, owned by one thread. `!Send` by contract: adapters
/// must not implement `Send` for it, and all methods run on the owner thread.
///
/// Every [`Handle`] returned is isolate-scoped and stays live until passed to
/// [`release`](Isolate::release) or until the enclosing [`scope`](Isolate::scope) ends.
pub trait Isolate {
    /// Runs a classic script and returns its completion value.
    fn eval_script(&mut self, source: &Source) -> Result<Value, JsException>;

    /// Parses a module; syntax errors surface here. Returns a module handle.
    fn compile_module(&mut self, source: &Source) -> Result<Handle, JsException>;

    /// Like [`compile_module`](Isolate::compile_module), consuming a sealed code cache when
    /// `cache` is valid for `source`. Invalid bytes are ignored and counted as rejected.
    /// Engines without code caches keep the default.
    fn compile_module_cached(
        &mut self,
        source: &Source,
        _cache: Option<&[u8]>,
    ) -> Result<Handle, JsException> {
        self.compile_module(source)
    }

    /// Sealed code cache of the compiled module `name` (a `Source::name`); best taken after the
    /// module ran, so lazily compiled functions are included. `None` if unsupported or unknown.
    fn create_code_cache(&mut self, _name: &str) -> Option<Vec<u8>> {
        None
    }

    /// Code caches this isolate accepted and rejected, hook-supplied ones included.
    fn code_cache_stats(&self) -> CodeCacheStats {
        CodeCacheStats::default()
    }

    /// Links `module` and its imports, asking `hook` for each specifier.
    fn instantiate_module(
        &mut self,
        module: Handle,
        hook: &mut dyn ModuleResolveHook,
    ) -> Result<(), JsException>;

    /// Evaluates an instantiated module. Returns a promise that settles when
    /// evaluation (including top-level await) finishes.
    fn evaluate_module(&mut self, module: Handle) -> Result<Value, JsException>;

    /// Calls `func` with `this` and `args`.
    fn call(&mut self, func: &Value, this: &Value, args: &[Value]) -> Result<Value, JsException>;

    /// Exposes a native function; fails on duplicate names. Called during bootstrap.
    fn register_host_function(&mut self, def: HostFunctionDef) -> Result<(), EngineError>;

    /// Stores runtime state, retrievable from host functions via
    /// `CallContext::isolate_slot::<T>()`. One value per type; replaces any previous one.
    fn set_slot<T: Any>(&mut self, value: T)
    where
        Self: Sized;

    /// Creates a pending promise and the id used to settle it.
    fn create_promise(&mut self) -> (Value, ResolverId);
    fn resolve(&mut self, resolver: ResolverId, value: Value);
    fn reject(&mut self, resolver: ResolverId, reason: Value);

    /// Copies `bytes` into a new `ArrayBuffer`. Adapters without buffers return `undefined`.
    fn create_array_buffer(&mut self, _bytes: &[u8]) -> Value {
        Value::Undefined
    }

    /// Builds the JS `Error` for `err`, with `.code` set to its `VELYR_*` string (the same shape
    /// a failed host function throws). The default is the `"CODE: message"` string.
    fn create_velyr_error(&mut self, err: &VelyrError) -> Value {
        Value::String(JsString::from_str(&err.to_string()))
    }

    /// Settlement state of `promise`, or `None` if it is not a promise. Needed to
    /// observe the result of [`evaluate_module`](Isolate::evaluate_module).
    fn promise_state(&mut self, promise: &Value) -> Option<PromiseState>;

    /// Drains the microtask queue. Called by the event loop on the owner thread
    /// after each macrotask.
    fn perform_microtask_checkpoint(&mut self) -> MicrotaskProgress;

    /// Reasons of promises rejected with no handler attached, as of now. A promise leaves
    /// the set once a handler is added. Call after a microtask checkpoint; each call
    /// returns the current set. Engines without rejection tracking report none.
    fn take_unhandled_rejections(&mut self) -> Vec<Value> {
        Vec::new()
    }

    fn heap_stats(&self) -> HeapStats;

    /// Remaps the frame positions of every error stack through `mapper`. A user-defined
    /// `Error.prepareStackTrace` still runs when present. Engines without the hook ignore it.
    fn set_prepare_stack_trace(&mut self, _mapper: crate::StackFrameMapper) {}

    /// A hint; the engine may ignore it.
    fn request_gc(&mut self, reason: GcReason);

    /// Runs engine work that background threads made ready (e.g. finishing an async
    /// WebAssembly compile). `true` while such work is still outstanding: the loop must stay
    /// alive and call again soon, since nothing else wakes it. Engines without any: no-op.
    fn run_platform_tasks(&mut self) -> bool {
        false
    }

    /// May be handed to other threads (watchdogs, the supervisor).
    fn termination_handle(&self) -> TerminationHandle;

    /// Structured-clones `value` to bytes ([`EngineFeatures::SERIALIZE`]). Each `transfer`
    /// entry must be an `ArrayBuffer`; on success it is detached and its backing store
    /// moves into the result. Uncloneable input (functions, symbols, ...) throws a
    /// `DataCloneError`.
    fn serialize(
        &mut self,
        _value: &Value,
        _transfer: &[Value],
    ) -> Result<SerializedValue, JsException> {
        Err(JsException::serialize_unsupported())
    }

    /// Rebuilds the value `serialized` describes in this isolate, attaching its transferred buffers.
    fn deserialize(&mut self, _serialized: &SerializedValue) -> Result<Value, JsException> {
        Err(JsException::serialize_unsupported())
    }

    /// Sets the continuation context ([`EngineFeatures::CONTINUATION_CONTEXT`]): a value the
    /// engine captures when a promise reaction (or `await`) is registered and restores while that
    /// reaction runs. Engines without the feature ignore the call and
    /// [`get_continuation_context`](Self::get_continuation_context) returns `undefined`.
    fn set_continuation_context(&mut self, _value: Value) {}

    /// The continuation context of the code running now; `undefined` when unset or unsupported.
    fn get_continuation_context(&mut self) -> Value {
        Value::Undefined
    }

    /// Frees `handle`. Stale or already released handles are ignored.
    fn release(&mut self, handle: Handle);

    /// Runs `f`; every handle created inside is released on exit. Handles that
    /// must outlive the scope have to be converted to owned data first.
    fn scope<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R
    where
        Self: Sized;

    /// Names the entry module (the compile name `import.meta.url` reports); `import.meta.main`
    /// is `true` only there. Call before compiling it. Engines without the feature ignore the
    /// call and leave `import.meta.main` undefined.
    fn set_entry_module(&mut self, _name: &str) {}

    /// Parses `source` as a classic script (early errors included) without running it. The
    /// default is an "unsupported" error, never `Ok`: callers must be able to tell an engine
    /// without a parse-only path from a script that parses (see [`JsException::is_unsupported`]).
    fn check_script(&mut self, _source: &Source) -> Result<(), JsException> {
        Err(JsException::unsupported("check_script"))
    }
}

fn unsupported_hook(what: &str) -> HostError {
    HostError::Velyr(VelyrError::unsupported(format!(
        "{what} is not supported by this engine"
    )))
}

fn unsupported_modules() -> HostError {
    HostError::Velyr(VelyrError::unsupported(
        "synchronous module loading is not supported by this engine",
    ))
}

fn unsupported_native() -> HostError {
    HostError::Velyr(VelyrError::unsupported(
        "native addons are not supported by this engine",
    ))
}
