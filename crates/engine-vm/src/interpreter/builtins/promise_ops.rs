//! Promise abstract operations (ECMA-262 §27.2.1-27.2.2): promise records, capabilities,
//! resolving functions, reactions and the two promise jobs.
//!
//! Native functions do not see their own function object, so the state a spec closure
//! captures (`Captures`) is shared by the Rust closure and an internal slot of the function
//! object; the slot is what keeps that state traced while the function is alive.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use velyr_engine_gc::{Trace, Tracer};

use super::promise_combinators::ElementState;
use super::slots::Slot;
use super::symbol::SYMBOL_SPECIES;
use super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::interpreter::function::NativeFn;
use crate::interpreter::{Abrupt, ErrorKind};
use crate::{ObjectKey, ObjectRef, Value, same_value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Pending,
    Fulfilled,
    Rejected,
}

/// [[PromiseState]], [[PromiseResult]], the reaction lists and [[PromiseIsHandled]].
pub(crate) struct PromiseRecord {
    pub status: Status,
    pub result: Value,
    pub fulfill_reactions: Vec<Reaction>,
    pub reject_reactions: Vec<Reaction>,
    pub is_handled: bool,
}

/// Never borrowed across an allocation or a call: the GC borrows it while tracing.
pub(crate) type PromiseCell = RefCell<PromiseRecord>;

impl Trace for PromiseRecord {
    fn trace(&self, tracer: &mut Tracer) {
        self.result.trace(tracer);
        for reaction in self.fulfill_reactions.iter().chain(&self.reject_reactions) {
            reaction.trace(tracer);
        }
    }
}

/// PromiseCapability Record.
#[derive(Clone)]
pub(crate) struct Capability {
    pub promise: Value,
    pub resolve: Value,
    pub reject: Value,
}

impl Trace for Capability {
    fn trace(&self, tracer: &mut Tracer) {
        self.promise.trace(tracer);
        self.resolve.trace(tracer);
        self.reject.trace(tracer);
    }
}

/// PromiseReaction Record; an `Undefined` handler is the spec's ~empty~.
#[derive(Clone)]
pub(crate) struct Reaction {
    pub capability: Capability,
    pub handler: Value,
    pub rejects: bool,
}

impl Trace for Reaction {
    fn trace(&self, tracer: &mut Tracer) {
        self.capability.trace(tracer);
        self.handler.trace(tracer);
    }
}

#[derive(Clone)]
pub(crate) enum Job {
    /// NewPromiseReactionJob.
    Reaction { reaction: Reaction, argument: Value },
    /// NewPromiseResolveThenableJob.
    ResolveThenable {
        promise: Rc<PromiseCell>,
        thenable: Value,
        then: Value,
    },
}

impl Trace for Job {
    fn trace(&self, tracer: &mut Tracer) {
        match self {
            Job::Reaction { reaction, argument } => {
                reaction.trace(tracer);
                argument.trace(tracer);
            }
            Job::ResolveThenable {
                promise,
                thenable,
                then,
            } => {
                promise.borrow().trace(tracer);
                thenable.trace(tracer);
                then.trace(tracer);
            }
        }
    }
}

/// What the spec's abstract closures of the promise built-ins capture.
pub(crate) enum Captures {
    /// Promise Resolve/Reject Functions (both share `already`).
    Resolving {
        promise: Rc<PromiseCell>,
        already: Rc<Cell<bool>>,
        rejects: bool,
    },
    /// GetCapabilitiesExecutor: `[resolve, reject]`.
    Executor(Rc<RefCell<[Value; 2]>>),
    /// Element functions of `Promise.all/allSettled/any`.
    Element(ElementState),
    /// Then Finally / Catch Finally functions.
    Finally {
        on_finally: Value,
        constructor: Value,
        rejects: bool,
    },
    /// The value thunk / thrower of `finally`.
    Thunk { value: Value, throws: bool },
}

impl Trace for Captures {
    fn trace(&self, tracer: &mut Tracer) {
        match self {
            Captures::Resolving { promise, .. } => promise.borrow().trace(tracer),
            Captures::Executor(slots) => slots.borrow().as_slice().trace(tracer),
            Captures::Element(element) => element.trace(tracer),
            Captures::Finally {
                on_finally,
                constructor,
                ..
            } => {
                on_finally.trace(tracer);
                constructor.trace(tracer);
            }
            Captures::Thunk { value, .. } => value.trace(tracer),
        }
    }
}

impl PromiseRecord {
    pub(crate) fn pending() -> Rc<PromiseCell> {
        Rc::new(RefCell::new(PromiseRecord {
            status: Status::Pending,
            result: Value::Undefined,
            fulfill_reactions: Vec::new(),
            reject_reactions: Vec::new(),
            is_handled: false,
        }))
    }
}

impl Interpreter {
    pub(crate) fn promise_record(&self, value: &Value) -> Option<Rc<PromiseCell>> {
        match self.value_slot(value)? {
            Slot::Promise(record) => Some(record),
            _ => None,
        }
    }

    /// A pending promise object with `proto`, rooted until the native returns.
    pub(crate) fn new_promise(&mut self, proto: ObjectRef) -> JsResult<(Value, Rc<PromiseCell>)> {
        let record = PromiseRecord::pending();
        let promise = self.new_slotted(proto, Slot::Promise(record.clone()))?;
        Ok((object_value(promise), record))
    }

    /// An anonymous built-in function running `captures`, rooted until the native returns.
    pub(crate) fn promise_function(
        &mut self,
        arity: u32,
        captures: Rc<Captures>,
    ) -> JsResult<Value> {
        let inner = captures.clone();
        let call: NativeFn = Rc::new(move |vm, _, args| vm.run_captures(&inner, args));
        let function = self.create_native_function("", arity, call, false)?;
        self.set_slot(function, Slot::PromiseFunction(captures));
        let function = object_value(function);
        self.keep(&function);
        Ok(function)
    }

    fn run_captures(&mut self, captures: &Captures, args: &[Value]) -> JsResult<Value> {
        let first = arg(args, 0);
        match captures {
            Captures::Resolving {
                promise,
                already,
                rejects,
            } => {
                if !already.replace(true) {
                    if *rejects {
                        self.reject_promise(promise, first);
                    } else {
                        self.resolve_promise(promise, first)?;
                    }
                }
                Ok(Value::Undefined)
            }
            Captures::Executor(slots) => {
                let occupied = slots
                    .borrow()
                    .iter()
                    .any(|v| !matches!(v, Value::Undefined));
                if occupied {
                    return self.throw_type("Promise executor has already been invoked");
                }
                *slots.borrow_mut() = [first, arg(args, 1)];
                Ok(Value::Undefined)
            }
            Captures::Element(element) => self.run_element(element, first),
            Captures::Finally {
                on_finally,
                constructor,
                rejects,
            } => {
                let result = self.call_value(on_finally, &Value::Undefined, &[])?;
                let promise = self.promise_resolve(constructor, result)?;
                self.keep(&promise);
                let thunk = Captures::Thunk {
                    value: first,
                    throws: *rejects,
                };
                let thunk = self.promise_function(0, Rc::new(thunk))?;
                self.invoke(&promise, "then", &[thunk])
            }
            Captures::Thunk { value, throws } if *throws => Err(Abrupt::Throw(value.clone())),
            Captures::Thunk { value, .. } => Ok(value.clone()),
        }
    }

    /// CreateResolvingFunctions; both are rooted until the native returns.
    pub(crate) fn resolving_functions(
        &mut self,
        promise: &Rc<PromiseCell>,
    ) -> JsResult<(Value, Value)> {
        let already = Rc::new(Cell::new(false));
        let make = |vm: &mut Self, rejects| {
            let captures = Captures::Resolving {
                promise: promise.clone(),
                already: already.clone(),
                rejects,
            };
            vm.promise_function(1, Rc::new(captures))
        };
        Ok((make(self, false)?, make(self, true)?))
    }

    /// The body of a promise resolve function once it was not already resolved.
    fn resolve_promise(&mut self, promise: &Rc<PromiseCell>, resolution: Value) -> JsResult<()> {
        if let Some(record) = self.promise_record(&resolution)
            && Rc::ptr_eq(&record, promise)
        {
            let error =
                self.create_error(ErrorKind::Type, "Chaining cycle detected for promise")?;
            self.reject_promise(promise, error);
            return Ok(());
        }
        let Some(object) = self.as_object(&resolution) else {
            self.fulfill_promise(promise, resolution);
            return Ok(());
        };
        let then = match self.get_named(object, "then") {
            Ok(then) => then,
            Err(Abrupt::Throw(error)) => {
                self.reject_promise(promise, error);
                return Ok(());
            }
            Err(abrupt) => return Err(abrupt),
        };
        if !self.is_callable(&then) {
            self.fulfill_promise(promise, resolution);
            return Ok(());
        }
        self.state.jobs.enqueue(Job::ResolveThenable {
            promise: promise.clone(),
            thenable: resolution,
            then,
        });
        Ok(())
    }

    /// FulfillPromise / RejectPromise (with HostPromiseRejectionTracker "reject").
    fn settle(&mut self, promise: &Rc<PromiseCell>, value: Value, rejects: bool) {
        let (reactions, unhandled) = {
            let mut record = promise.borrow_mut();
            if record.status != Status::Pending {
                return;
            }
            record.status = if rejects {
                Status::Rejected
            } else {
                Status::Fulfilled
            };
            record.result = value.clone();
            let fulfill = std::mem::take(&mut record.fulfill_reactions);
            let reject = std::mem::take(&mut record.reject_reactions);
            let reactions = if rejects { reject } else { fulfill };
            (reactions, rejects && !record.is_handled)
        };
        if unhandled {
            self.state.jobs.track_rejected(promise.clone());
        }
        for reaction in reactions {
            let argument = value.clone();
            self.state
                .jobs
                .enqueue(Job::Reaction { reaction, argument });
        }
    }

    pub(crate) fn fulfill_promise(&mut self, promise: &Rc<PromiseCell>, value: Value) {
        self.settle(promise, value, false);
    }

    pub(crate) fn reject_promise(&mut self, promise: &Rc<PromiseCell>, reason: Value) {
        self.settle(promise, reason, true);
    }

    /// PerformPromiseThen with a result capability.
    pub(crate) fn perform_then(
        &mut self,
        promise: &Rc<PromiseCell>,
        on_fulfilled: &Value,
        on_rejected: &Value,
        capability: Capability,
    ) {
        let handler = |vm: &Self, f: &Value| {
            if vm.is_callable(f) {
                f.clone()
            } else {
                Value::Undefined
            }
        };
        let fulfill = Reaction {
            capability: capability.clone(),
            handler: handler(self, on_fulfilled),
            rejects: false,
        };
        let reject = Reaction {
            capability,
            handler: handler(self, on_rejected),
            rejects: true,
        };
        let (status, argument, was_handled) = {
            let mut record = promise.borrow_mut();
            let was_handled = std::mem::replace(&mut record.is_handled, true);
            (record.status, record.result.clone(), was_handled)
        };
        let reaction = match status {
            Status::Pending => {
                let mut record = promise.borrow_mut();
                record.fulfill_reactions.push(fulfill);
                record.reject_reactions.push(reject);
                return;
            }
            Status::Fulfilled => fulfill,
            Status::Rejected => {
                if !was_handled {
                    self.state.jobs.track_handled(promise);
                }
                reject
            }
        };
        self.state
            .jobs
            .enqueue(Job::Reaction { reaction, argument });
    }

    /// NewPromiseCapability(C); every part is rooted until the native returns.
    pub(crate) fn new_capability(&mut self, constructor: &Value) -> JsResult<Capability> {
        if !self.is_constructor(constructor) {
            return self.throw_type("Promise capability constructor is not a constructor");
        }
        let slots = Rc::new(RefCell::new([Value::Undefined, Value::Undefined]));
        let executor = self.promise_function(2, Rc::new(Captures::Executor(slots.clone())))?;
        let promise = self.construct_value(constructor, &[executor], constructor)?;
        self.keep(&promise);
        let [resolve, reject] = slots.borrow().clone();
        self.keep(&resolve);
        self.keep(&reject);
        if !self.is_callable(&resolve) {
            return self.throw_type("Promise resolve function is not callable");
        }
        if !self.is_callable(&reject) {
            return self.throw_type("Promise reject function is not callable");
        }
        Ok(Capability {
            promise,
            resolve,
            reject,
        })
    }

    /// PromiseResolve(C, x).
    pub(crate) fn promise_resolve(&mut self, constructor: &Value, x: Value) -> JsResult<Value> {
        if self.promise_record(&x).is_some() {
            let object = self.as_object(&x).expect("promises are objects");
            let x_constructor = self.get_named(object, "constructor")?;
            if same_value(&x_constructor, constructor) {
                return Ok(x);
            }
        }
        let capability = self.new_capability(constructor)?;
        self.call_value(&capability.resolve, &Value::Undefined, &[x])?;
        Ok(capability.promise)
    }

    /// SpeciesConstructor(O, %Promise%).
    pub(crate) fn promise_species(&mut self, object: ObjectRef) -> JsResult<Value> {
        let default = object_value(self.intrinsic(Intrinsic::PromiseConstructor));
        self.species_constructor(object, default)
    }

    /// SpeciesConstructor(O, defaultConstructor).
    pub(crate) fn species_constructor(
        &mut self,
        object: ObjectRef,
        default: Value,
    ) -> JsResult<Value> {
        let constructor = self.get_named(object, "constructor")?;
        if matches!(constructor, Value::Undefined) {
            return Ok(default);
        }
        let Some(constructor) = self.as_object(&constructor) else {
            return self.throw_type("object.constructor is not an object");
        };
        let species = self.get_key(constructor, ObjectKey::Symbol(SYMBOL_SPECIES))?;
        if matches!(species, Value::Undefined | Value::Null) {
            return Ok(default);
        }
        if !self.is_constructor(&species) {
            return self.throw_type("object.constructor[Symbol.species] is not a constructor");
        }
        Ok(species)
    }

    /// Runs one job; the caller roots it (see `JobQueue::running`).
    pub(crate) fn run_job(&mut self, job: Job) -> JsResult<()> {
        match job {
            Job::Reaction { reaction, argument } => {
                let outcome = match (&reaction.handler, reaction.rejects) {
                    (Value::Undefined, false) => Ok(argument),
                    (Value::Undefined, true) => Err(Abrupt::Throw(argument)),
                    (handler, _) => self.call_value(handler, &Value::Undefined, &[argument]),
                };
                let capability = reaction.capability;
                let (settle, value) = match outcome {
                    Ok(value) => (capability.resolve, value),
                    Err(Abrupt::Throw(error)) => (capability.reject, error),
                    Err(abrupt) => return Err(abrupt),
                };
                self.call_value(&settle, &Value::Undefined, &[value])?;
            }
            Job::ResolveThenable {
                promise,
                thenable,
                then,
            } => {
                let (resolve, reject) = self.resolving_functions(&promise)?;
                let result = self.call_value(&then, &thenable, &[resolve, reject.clone()]);
                if let Err(Abrupt::Throw(error)) = result {
                    self.call_value(&reject, &Value::Undefined, &[error])?;
                } else {
                    result?;
                }
            }
        }
        Ok(())
    }

    /// IfAbruptRejectPromise(result, capability).
    pub(crate) fn reject_if_abrupt(
        &mut self,
        result: JsResult<Value>,
        capability: &Capability,
    ) -> JsResult<Value> {
        match result {
            Err(Abrupt::Throw(error)) => {
                self.call_value(&capability.reject, &Value::Undefined, &[error])?;
                Ok(capability.promise.clone())
            }
            other => other,
        }
    }
}
