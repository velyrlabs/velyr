//! Everything the interpreter keeps alive: the register stack, frames, accumulator and the
//! realm-wide tables. Its `Trace` impl is the root set handed to every allocation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use velyr_engine_gc::{Gc, Trace, Tracer, WeakMap};

use super::ErrorKind;
use super::builtins::Intrinsic;
use super::builtins::regexp::RegExpCache;
use super::builtins::slots::SlotTable;
use super::builtins::symbol::SymbolTable;
use super::function::{Callable, FunctionEnv};
use super::handles::HandleTable;
use super::jobs::JobQueue;
use crate::bytecode::FunctionCode;
use crate::{InlineCache, JsStr, ObjectKey, ObjectRef, Value};

/// Per-`FunctionCode` data shared by every closure of it: inline caches indexed by
/// instruction offset and whether frames must keep their arguments.
pub(crate) struct CodeInfo {
    pub code: Rc<FunctionCode>,
    pub caches: RefCell<Vec<InlineCache>>,
    pub keeps_arguments: bool,
    /// Generator or async function: its kind and the register of its generator object.
    pub coroutine: Option<(crate::bytecode::CoroutineKind, crate::bytecode::Reg)>,
}

impl CodeInfo {
    pub fn new(code: Rc<FunctionCode>) -> Self {
        use crate::bytecode::Instruction as I;
        let keeps_arguments = code.instructions.iter().any(|instruction| {
            matches!(
                instruction,
                I::CreateUnmappedArguments | I::CreateRestParameter(_)
            )
        });
        let coroutine = code
            .instructions
            .iter()
            .find_map(|instruction| match instruction {
                I::CreateGenerator { generator, kind } => Some((*kind, *generator)),
                _ => None,
            });
        let caches = (0..code.instructions.len())
            .map(|_| InlineCache::default())
            .collect();
        Self {
            code,
            caches: RefCell::new(caches),
            keeps_arguments,
            coroutine,
        }
    }
}

pub(crate) struct Frame {
    pub info: Rc<CodeInfo>,
    /// Offset of the next instruction; the running (or calling) one is `pc - 1`.
    pub pc: u32,
    /// Index of `r0` in the register stack.
    pub base: usize,
    /// Current context cell, `Undefined` when the function has none.
    pub context: Value,
    pub env: FunctionEnv,
    /// The running function object (`LdaCurrentClosure`).
    pub function: Value,
    pub arguments: Vec<Value>,
    /// The object `new` allocated; returned unless the body returns an object.
    pub constructed: Option<Value>,
    /// Bottom frame of one `execute` run: returning from it leaves the dispatch loop.
    pub is_entry: bool,
}

pub(crate) struct Lexical {
    pub value: Value,
    pub is_const: bool,
}

/// Function objects are ordinary objects; their behavior lives in this ephemeron table
/// (a heap cell, so entries die with their function object).
#[derive(Default)]
pub(crate) struct FunctionTable(pub WeakMap<Callable>);

impl Trace for FunctionTable {
    fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
    }

    fn clear_dead_refs(&mut self, liveness: &velyr_engine_gc::Liveness) -> bool {
        self.0.clear_dead_refs(liveness)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Intrinsics {
    pub global: ObjectRef,
    pub object_prototype: ObjectRef,
    pub function_prototype: ObjectRef,
    pub array_prototype: ObjectRef,
    pub array_iterator_prototype: ObjectRef,
    /// `Function.prototype.call/apply`, recognized by the dispatch loop.
    pub function_call: ObjectRef,
    pub function_apply: ObjectRef,
    /// Indexed by `ErrorKind`.
    pub error_prototypes: [ObjectRef; ErrorKind::ALL.len()],
    /// `Error`, which holds `stackTraceLimit`.
    pub error_constructor: ObjectRef,
    pub length_key: ObjectKey,
    /// Built-in prototypes and constructors installed by `builtins`, indexed by `Intrinsic`.
    pub table: [ObjectRef; Intrinsic::COUNT],
}

impl Trace for Intrinsics {
    fn trace(&self, tracer: &mut Tracer) {
        for object in [
            self.global,
            self.object_prototype,
            self.function_prototype,
            self.array_prototype,
            self.array_iterator_prototype,
            self.function_call,
            self.function_apply,
            self.error_constructor,
        ] {
            tracer.mark(object.raw());
        }
        self.error_prototypes.as_slice().trace(tracer);
        self.table.as_slice().trace(tracer);
    }
}

pub(crate) struct State {
    pub registers: Vec<Value>,
    pub frames: Vec<Frame>,
    pub acc: Value,
    /// Rust-side temporaries that must survive an allocation or a call into JS.
    pub scratch: Vec<Value>,
    pub lexicals: HashMap<JsStr, Lexical>,
    pub functions: Gc<FunctionTable>,
    pub intrinsics: Intrinsics,
    /// Template objects per (code address, constant index); the code stays alive in
    /// `CodeInfo`, so addresses are never reused.
    pub templates: HashMap<(usize, u32), Value>,
    pub handles: HandleTable,
    /// Next private-name symbol id (see `class::PRIVATE_NAME_BASE`).
    pub next_private_name: u32,
    /// `NewTarget` of the running native constructor; `Undefined` for plain calls.
    pub new_target: Value,
    /// Internal slots of built-in objects (wrapper values, collections, iterators, proxies).
    pub slots: Gc<SlotTable>,
    pub symbols: SymbolTable,
    /// Promise jobs, the rejection tracker and embedder resolvers.
    pub jobs: JobQueue,
    /// Compiled RegExp matchers by (source, flags).
    pub regexps: RegExpCache,
    /// Module records, indexed by `ModuleId`.
    pub modules: Vec<super::module::ModuleRecord>,
}

impl Trace for State {
    fn trace(&self, tracer: &mut Tracer) {
        self.registers.trace(tracer);
        for frame in &self.frames {
            frame.context.trace(tracer);
            frame.env.trace(tracer);
            frame.function.trace(tracer);
            frame.arguments.trace(tracer);
            frame.constructed.trace(tracer);
        }
        self.acc.trace(tracer);
        self.scratch.trace(tracer);
        for lexical in self.lexicals.values() {
            lexical.value.trace(tracer);
        }
        tracer.mark(self.functions.raw());
        self.intrinsics.trace(tracer);
        for template in self.templates.values() {
            template.trace(tracer);
        }
        self.handles.trace(tracer);
        self.new_target.trace(tracer);
        tracer.mark(self.slots.raw());
        self.jobs.trace(tracer);
        self.modules.trace(tracer);
    }
}
