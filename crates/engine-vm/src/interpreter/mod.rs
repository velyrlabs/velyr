//! Bytecode interpreter (VELYR-104): one `match` dispatch loop over a contiguous register
//! stack. JS-to-JS calls push a frame instead of recursing in Rust (trampoline); only
//! calls that start in Rust (accessors, iterator protocol, ToPrimitive, embedder calls)
//! nest a new `execute` run, bounded by `MAX_REENTRY`. Back-edges and calls poll the
//! termination flag, which is how `terminate_execution` and the CPU budget stop JS.

mod builtins;
pub(crate) mod class;
mod construct;
mod dispatch;
pub(crate) mod environment;
pub(crate) mod function;
#[path = "../generator.rs"]
pub(crate) mod generator;
mod handles;
pub(crate) mod intrinsics;
mod iteration;
mod jobs;
mod literal;
#[path = "../module.rs"]
pub(crate) mod module;
pub(crate) mod operators;
mod ops;
mod property;
pub(crate) mod stack_trace;
mod state;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use velyr_engine_gc::{GcRef, Heap};
use velyr_runtime_errors::VelyrError;

use crate::bytecode::FunctionCode;
use crate::{JsStr, ObjectRef, Realm, Value};
pub use builtins::date::LocalTimeZone;
pub use builtins::regexp::validate as validate_regexp;
pub use function::NativeFn;
pub use jobs::PromiseStatus;
pub use module::{ExportTarget, ImportName, ModuleCode, ModuleId, ModuleImport};
use state::{CodeInfo, State};

/// JS frames before a call throws `RangeError`.
const MAX_FRAMES: usize = 10_000;
/// Nested `execute` runs (Rust recursion) before a call throws `RangeError`.
const MAX_REENTRY: usize = 64;

/// Abrupt completion seen by the embedder.
#[derive(Debug)]
pub enum Abrupt {
    Throw(Value),
    /// The termination flag was raised; JS cannot catch it.
    Terminated,
    /// The heap could not satisfy an allocation (e.g. the isolate quota).
    Heap(VelyrError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Error,
    Type,
    Range,
    Reference,
    Syntax,
    Eval,
    Uri,
}

impl ErrorKind {
    pub(crate) const ALL: [ErrorKind; 7] = [
        ErrorKind::Error,
        ErrorKind::Type,
        ErrorKind::Range,
        ErrorKind::Reference,
        ErrorKind::Syntax,
        ErrorKind::Eval,
        ErrorKind::Uri,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            ErrorKind::Error => "Error",
            ErrorKind::Type => "TypeError",
            ErrorKind::Range => "RangeError",
            ErrorKind::Reference => "ReferenceError",
            ErrorKind::Syntax => "SyntaxError",
            ErrorKind::Eval => "EvalError",
            ErrorKind::Uri => "URIError",
        }
    }
}

pub(crate) type JsResult<T> = Result<T, Abrupt>;

/// Compiles script source at run time (`Function`, indirect `eval`); `Err` is the
/// SyntaxError message. Supplied by the embedder that owns the compiler.
pub type CompileHook = Rc<dyn Fn(&str) -> Result<Rc<FunctionCode>, String>>;

pub struct Interpreter {
    pub(crate) realm: Realm,
    pub(crate) state: State,
    terminated: Arc<AtomicBool>,
    code_infos: HashMap<usize, Rc<CodeInfo>>,
    reentry: usize,
    compile: Option<CompileHook>,
    /// Receives `import()` requests as `(specifier, referrer, resolver id)`.
    import_hook: Option<NativeFn>,
    /// Local time zone of `Date`; UTC when unset.
    local_time_zone: Option<Rc<dyn LocalTimeZone>>,
    /// Largest BigInt result, in bits; past it an operation is a RangeError.
    bigint_max_bits: u64,
    /// Bytes held by ArrayBuffer backing stores, which live outside the GC heap.
    buffer_budget: Arc<builtins::typed_arrays::store::Budget>,
    /// Whether `Atomics.wait` may block this thread.
    atomics_wait_allowed: bool,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        Self::with_heap(Heap::default())
    }

    pub fn with_heap(heap: Heap) -> Self {
        let mut realm = Realm::new(heap);
        realm.call = property::accessor_needs_interpreter;
        let state = intrinsics::create_state(&mut realm);
        let mut interpreter = Self {
            realm,
            state,
            terminated: Arc::new(AtomicBool::new(false)),
            code_infos: HashMap::new(),
            reentry: 0,
            compile: None,
            import_hook: None,
            local_time_zone: None,
            bigint_max_bits: crate::bigint_ops::DEFAULT_MAX_BITS,
            buffer_budget: builtins::typed_arrays::store::Budget::unlimited(),
            atomics_wait_allowed: false,
        };
        interpreter
            .install_globals()
            .expect("a fresh heap holds the intrinsics");
        interpreter
    }

    /// Enables `Function(...)` and indirect `eval` (they throw a SyntaxError without it).
    pub fn set_compile_hook(&mut self, hook: CompileHook) {
        self.compile = Some(hook);
    }

    /// Raising this flag (from any thread) terminates running JS at the next back-edge
    /// or call; it stays raised, so later runs fail fast too.
    pub fn termination_flag(&self) -> Arc<AtomicBool> {
        self.terminated.clone()
    }

    /// Caps BigInt results at the isolate heap budget (one bit per budget bit), so a
    /// huge `**` or `<<` is a RangeError before it allocates. BigInt digits live outside
    /// the GC heap, so the heap quota alone would not see them.
    pub fn limit_bigints_to_heap_budget(&mut self, heap_limit_bytes: usize) {
        let budget_bits = (heap_limit_bytes as u64).saturating_mul(8);
        self.bigint_max_bits = budget_bits.min(crate::bigint_ops::DEFAULT_MAX_BITS);
    }

    /// Caps the bytes all live ArrayBuffer/SharedArrayBuffer backing stores may hold;
    /// past it `new ArrayBuffer` (or a resize) is a RangeError.
    pub fn limit_buffers_to_heap_budget(&mut self, heap_limit_bytes: usize) {
        self.buffer_budget.set_limit(heap_limit_bytes);
    }

    /// Lets `Atomics.wait` block; for worker threads only, never the main thread.
    pub fn allow_atomics_wait(&mut self, allowed: bool) {
        self.atomics_wait_allowed = allowed;
    }

    /// Runs a script's top-level code and returns its completion value.
    pub fn run_script(&mut self, code: &Rc<FunctionCode>) -> Result<Value, Abrupt> {
        let info = self.code_info(code);
        let global = Value::Object(self.state.intrinsics.global.raw());
        let script = function::Closure {
            info,
            context: Value::Undefined,
            env: function::FunctionEnv::new(global.clone()),
            home: Value::Undefined,
            elements: None,
        };
        let env = self.frame_env(&script, &Value::Undefined, &global, Value::Undefined);
        self.run_closure(&script, &Value::Undefined, env, &[], None)
    }

    /// `Call(f, this, args)` from the embedder.
    pub fn call(&mut self, f: &Value, this: &Value, args: &[Value]) -> Result<Value, Abrupt> {
        let base = self.state.scratch.len();
        self.state.scratch.extend([f.clone(), this.clone()]);
        self.state.scratch.extend(args.iter().cloned());
        let result = self.call_value(f, this, args);
        self.state.scratch.truncate(base);
        result
    }

    /// Defines a writable, configurable global function backed by `f`.
    pub fn define_global_function(
        &mut self,
        name: &str,
        arity: u32,
        f: NativeFn,
    ) -> Result<(), Abrupt> {
        let function = self.create_native_function(name, arity, f, false)?;
        let global = self.state.intrinsics.global;
        self.define_method(global, name, object_value(function))
    }

    /// The `ErrorKind` error object as an abrupt completion, for native functions.
    pub fn error(&mut self, kind: ErrorKind, message: &str) -> Abrupt {
        match self.create_error(kind, message) {
            Ok(error) => Abrupt::Throw(error),
            Err(abrupt) => abrupt,
        }
    }

    /// ECMAScript `ToString`, running `toString`/`valueOf` for objects.
    pub fn to_js_string(&mut self, value: &Value) -> Result<JsStr, Abrupt> {
        self.coerce_string(value)
    }

    /// `ToString(value)` (`"<name>: <message>"` for errors), the message engine-v8 reports.
    pub fn exception_message(&mut self, value: &Value) -> String {
        intrinsics::describe_exception(self, value)
    }

    /// Whether `r` is a live JS object of this heap (validates embedder handles).
    pub fn is_object(&self, r: GcRef) -> bool {
        self.realm.heap.get(ObjectRef::from_raw(r)).is_some()
    }

    fn code_info(&mut self, code: &Rc<FunctionCode>) -> Rc<CodeInfo> {
        let key = Rc::as_ptr(code) as usize;
        self.code_infos
            .entry(key)
            .or_insert_with(|| Rc::new(CodeInfo::new(code.clone())))
            .clone()
    }

    fn check_termination(&self) -> JsResult<()> {
        if self.terminated.load(Ordering::Relaxed) {
            return Err(Abrupt::Terminated);
        }
        Ok(())
    }

    fn alloc_object(&mut self, proto: Option<ObjectRef>) -> JsResult<ObjectRef> {
        self.realm
            .create_object(proto, &self.state)
            .map_err(Abrupt::Heap)
    }

    fn alloc_array(&mut self, length: u32) -> JsResult<ObjectRef> {
        let proto = Some(self.state.intrinsics.array_prototype);
        self.realm
            .create_array(proto, length, &self.state)
            .map_err(Abrupt::Heap)
    }

    fn alloc_cell<T: velyr_engine_gc::Trace + 'static>(&mut self, cell: T) -> JsResult<GcRef> {
        let cell = self.realm.heap.alloc(cell, &self.state);
        cell.map(|gc| gc.raw()).map_err(Abrupt::Heap)
    }
}

fn object_value(o: ObjectRef) -> Value {
    Value::Object(o.raw())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::{ConstIndex, Constant, FunctionKind, Instruction as I, Reg};

    fn script(instructions: Vec<I>, constants: Vec<Constant>) -> Rc<FunctionCode> {
        Rc::new(FunctionCode {
            name: JsStr::from_str(""),
            kind: FunctionKind::Script,
            is_strict: false,
            parameter_count: 0,
            length: 0,
            register_count: 1,
            instructions,
            constants,
            handlers: Vec::new(),
            positions: Vec::new(),
            source_start: 0,
            source_end: 0,
            script: None,
        })
    }

    #[test]
    fn named_loads_hit_the_inline_cache_after_the_first_miss() {
        let x = ConstIndex(0);
        let code = script(
            vec![
                I::CreateObject,
                I::Star(Reg(0)),
                I::LdaSmi(7),
                I::DefineNamedOwn {
                    object: Reg(0),
                    name: x,
                },
                I::GetNamed {
                    object: Reg(0),
                    name: x,
                },
                I::Return,
            ],
            vec![Constant::String(JsStr::from_str("x"))],
        );
        let mut vm = Interpreter::new();
        for _ in 0..3 {
            assert!(matches!(vm.run_script(&code), Ok(Value::Number(n)) if n == 7.0));
        }
        let info = vm.code_info(&code);
        let caches = info.caches.borrow();
        assert_eq!(caches[4].misses(), 1);
        assert!(caches[4].cached_shape().is_some());
    }
}
