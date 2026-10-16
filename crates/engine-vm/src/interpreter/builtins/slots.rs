//! Internal slots of built-in objects (`[[NumberData]]`, `[[MapData]]`, iterator state,
//! `[[ProxyTarget]]`...). Kept in an ephemeron table keyed by the object, like the
//! function table, so ordinary objects stay slot-free and entries die with their object.

use std::cell::RefCell;
use std::rc::Rc;

use velyr_engine_api::JsBigInt;
use velyr_engine_gc::{Liveness, Trace, Tracer, WeakMap};

use super::collections::OrderedTable;
use super::promise_ops::{Captures, PromiseCell};
use super::regexp::RegExpData;
use super::typed_arrays::TypedArray;
use super::typed_arrays::store::{Store, View};
use crate::interpreter::generator::GeneratorCell;
use crate::{JsStr, ObjectRef, SymbolId, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IterationKind {
    Keys,
    Values,
    Entries,
}

pub(crate) struct ArrayIteratorState {
    /// `Undefined` once exhausted.
    pub target: Value,
    pub index: u64,
    pub kind: IterationKind,
}

pub(crate) struct TableIteratorState {
    /// `None` once exhausted.
    pub table: Option<Rc<RefCell<OrderedTable>>>,
    pub position: usize,
    pub kind: IterationKind,
}

pub(crate) struct ProxyState {
    /// Both `None` once revoked.
    pub target: Option<ObjectRef>,
    pub handler: Option<ObjectRef>,
}

/// %RegExpStringIterator% state (CreateRegExpStringIterator).
pub(crate) struct RegExpIteratorState {
    pub matcher: Value,
    pub text: JsStr,
    pub global: bool,
    pub unicode: bool,
    pub done: bool,
}

/// Weakly keyed collection data: object keys are ephemerons, symbol keys are strong
/// (symbols are not heap cells).
/// shortcut: entries of dead keys are never pruned (their values are not traced, so only
/// the entry leaks); prune them once the GC visits nested tables.
#[derive(Default)]
pub(crate) struct WeakTable {
    pub objects: WeakMap<Value>,
    pub symbols: Vec<(SymbolId, Value)>,
}

#[derive(Clone)]
pub(crate) enum Slot {
    Boolean(bool),
    Number(f64),
    /// `[[DateValue]]`.
    Date(f64),
    String(JsStr),
    Symbol(SymbolId),
    BigInt(Rc<JsBigInt>),
    Error,
    Arguments,
    Map(Rc<RefCell<OrderedTable>>),
    Set(Rc<RefCell<OrderedTable>>),
    WeakMap(Rc<RefCell<WeakTable>>),
    WeakSet(Rc<RefCell<WeakTable>>),
    ArrayIterator(Rc<RefCell<ArrayIteratorState>>),
    MapIterator(Rc<RefCell<TableIteratorState>>),
    SetIterator(Rc<RefCell<TableIteratorState>>),
    /// `None` once exhausted.
    StringIterator(Rc<RefCell<Option<(JsStr, usize)>>>),
    Proxy(Rc<RefCell<ProxyState>>),
    RegExp(Rc<RegExpData>),
    RegExpStringIterator(Rc<RefCell<RegExpIteratorState>>),
    Promise(Rc<PromiseCell>),
    /// State captured by a promise built-in closure (see `promise_ops`).
    PromiseFunction(Rc<Captures>),
    /// Generator object, or a promise reaction resuming one (see `generator`).
    Generator(Rc<GeneratorCell>),
    /// %AsyncFromSyncIteratorPrototype% object: `[sync iterator, its next method]`.
    AsyncFromSyncIterator(Rc<[Value; 2]>),
    /// `[[ArrayBufferData]]` of an ArrayBuffer or a SharedArrayBuffer.
    ArrayBuffer(Store),
    TypedArray(Rc<TypedArray>),
    DataView(Rc<View>),
}

impl Trace for Slot {
    fn trace(&self, tracer: &mut Tracer) {
        match self {
            Slot::Map(table) | Slot::Set(table) => table.borrow().trace(tracer),
            Slot::WeakMap(table) | Slot::WeakSet(table) => {
                let table = table.borrow();
                table.objects.trace(tracer);
                for (_, value) in &table.symbols {
                    value.trace(tracer);
                }
            }
            Slot::ArrayIterator(state) => state.borrow().target.trace(tracer),
            Slot::MapIterator(state) | Slot::SetIterator(state) => {
                if let Some(table) = &state.borrow().table {
                    table.borrow().trace(tracer);
                }
            }
            Slot::Proxy(state) => {
                let state = state.borrow();
                state.target.trace(tracer);
                state.handler.trace(tracer);
            }
            Slot::Promise(promise) => promise.borrow().trace(tracer),
            Slot::RegExpStringIterator(state) => state.borrow().matcher.trace(tracer),
            Slot::PromiseFunction(captures) => captures.trace(tracer),
            Slot::Generator(state) => state.borrow().trace(tracer),
            Slot::AsyncFromSyncIterator(pair) => pair.as_slice().trace(tracer),
            Slot::TypedArray(array) => array.view.buffer.trace(tracer),
            Slot::DataView(view) => view.buffer.trace(tracer),
            _ => {}
        }
    }
}

#[derive(Default)]
pub(crate) struct SlotTable(pub WeakMap<Slot>);

impl Trace for SlotTable {
    fn trace(&self, tracer: &mut Tracer) {
        self.0.trace(tracer);
    }

    fn clear_dead_refs(&mut self, liveness: &Liveness) -> bool {
        self.0.clear_dead_refs(liveness)
    }
}
