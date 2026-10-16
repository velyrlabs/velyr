//! The isolate's job queue (ECMA-262 §9.5, HostEnqueuePromiseJob), the promise rejection
//! tracker (HostPromiseRejectionTracker) and the embedder promise API (VELYR-108).

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use velyr_engine_gc::{Trace, Tracer};

use super::builtins::Intrinsic;
use super::builtins::promise_ops::{Job, PromiseCell, Status};
use super::{Abrupt, Interpreter};
use crate::Value;

#[derive(Default)]
pub(crate) struct JobQueue {
    pending: VecDeque<Job>,
    /// Jobs being run (a stack in case a job reenters the checkpoint), kept as roots.
    running: Vec<Job>,
    /// Rejected promises without a handler, in rejection order.
    unhandled: Vec<Rc<PromiseCell>>,
    /// Resolving functions of embedder promises, by resolver id, until settled.
    resolvers: HashMap<u64, [Value; 2]>,
    next_resolver: u64,
    /// The continuation context of the running code: captured by every reaction when it is
    /// registered and restored while its job runs (`AsyncLocalStorage` builds on it).
    pub(crate) continuation: Value,
}

impl Trace for JobQueue {
    fn trace(&self, tracer: &mut Tracer) {
        self.continuation.trace(tracer);
        for job in self.pending.iter().chain(&self.running) {
            job.trace(tracer);
        }
        for promise in &self.unhandled {
            promise.borrow().trace(tracer);
        }
        for functions in self.resolvers.values() {
            functions.as_slice().trace(tracer);
        }
    }
}

impl JobQueue {
    pub(crate) fn enqueue(&mut self, job: Job) {
        self.pending.push_back(job);
    }

    /// HostPromiseRejectionTracker(promise, "reject").
    pub(crate) fn track_rejected(&mut self, promise: Rc<PromiseCell>) {
        self.unhandled.push(promise);
    }

    /// HostPromiseRejectionTracker(promise, "handle").
    pub(crate) fn track_handled(&mut self, promise: &Rc<PromiseCell>) {
        self.unhandled.retain(|p| !Rc::ptr_eq(p, promise));
    }
}

/// Settlement state of a promise, as seen by the embedder.
#[derive(Debug, Clone)]
pub enum PromiseStatus {
    Pending,
    Fulfilled(Value),
    Rejected(Value),
}

impl Interpreter {
    /// Runs jobs until the queue is empty; `Ok(true)` if at least one ran. A job that
    /// throws is reported nowhere (HostReportErrors); termination and heap errors stop
    /// the drain and leave the remaining jobs queued.
    pub fn perform_microtask_checkpoint(&mut self) -> Result<bool, Abrupt> {
        let mut ran = false;
        loop {
            // ClearKeptObjects: the script or job that kept WeakRef targets has ended.
            self.state.kept.clear();
            let Some(job) = self.state.jobs.pending.pop_front() else {
                // Cleanup callbacks run once the queue is empty and may queue more jobs.
                if !self.run_finalizers()? {
                    return Ok(ran);
                }
                ran = true;
                continue;
            };
            ran = true;
            self.state.jobs.running.push(job.clone());
            let mark = self.state.scratch.len();
            let previous = std::mem::replace(&mut self.state.jobs.continuation, job.context());
            self.state.scratch.push(previous.clone());
            let result = self.run_job(job);
            self.state.jobs.continuation = previous;
            self.state.scratch.truncate(mark);
            self.state.jobs.running.pop();
            match result {
                Ok(()) | Err(Abrupt::Throw(_)) => {}
                Err(abrupt) => return Err(abrupt),
            }
        }
    }

    /// A pending `%Promise%` and the id that settles it (`settle_promise`). The promise is
    /// not rooted: root it (e.g. `create_handle`) before the next allocation.
    pub fn create_promise(&mut self) -> Result<(Value, u64), Abrupt> {
        let mark = self.state.scratch.len();
        let result = self.new_embedder_promise();
        self.state.scratch.truncate(mark);
        result
    }

    fn new_embedder_promise(&mut self) -> Result<(Value, u64), Abrupt> {
        let proto = self.intrinsic(Intrinsic::PromisePrototype);
        let (promise, record) = self.new_promise(proto)?;
        let (resolve, reject) = self.resolving_functions(&record)?;
        let id = self.state.jobs.next_resolver;
        self.state.jobs.next_resolver += 1;
        self.state.jobs.resolvers.insert(id, [resolve, reject]);
        Ok((promise, id))
    }

    /// Calls the resolve (or reject) function behind `id`; later calls and unknown ids are
    /// ignored. Resolving with a thenable follows it, like JS `resolve(value)`.
    pub fn settle_promise(&mut self, id: u64, value: &Value, rejects: bool) -> Result<(), Abrupt> {
        let Some([resolve, reject]) = self.state.jobs.resolvers.remove(&id) else {
            return Ok(());
        };
        let mark = self.state.scratch.len();
        let function = if rejects { reject } else { resolve };
        self.state.scratch.push(function.clone());
        let result = self.call_value(&function, &Value::Undefined, std::slice::from_ref(value));
        self.state.scratch.truncate(mark);
        result.map(drop)
    }

    /// Sets the continuation context: reactions registered from now on capture it and see
    /// it again while their jobs run.
    pub fn set_continuation_context(&mut self, value: Value) {
        self.state.jobs.continuation = value;
    }

    /// The continuation context of the code running now; `undefined` when unset.
    pub fn continuation_context(&self) -> Value {
        self.state.jobs.continuation.clone()
    }

    /// `None` when `value` is not a promise.
    pub fn promise_status(&self, value: &Value) -> Option<PromiseStatus> {
        let record = self.promise_record(value)?;
        let record = record.borrow();
        Some(match record.status {
            Status::Pending => PromiseStatus::Pending,
            Status::Fulfilled => PromiseStatus::Fulfilled(record.result.clone()),
            Status::Rejected => PromiseStatus::Rejected(record.result.clone()),
        })
    }

    /// Reasons of the promises rejected with no handler, as of now (not cleared by reading).
    pub fn unhandled_rejections(&self) -> Vec<Value> {
        let unhandled = &self.state.jobs.unhandled;
        unhandled
            .iter()
            .map(|p| p.borrow().result.clone())
            .collect()
    }
}
