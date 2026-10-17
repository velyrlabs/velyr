//! Timer phase: runs the timers due at the start of a tick, grouping consecutive host timers
//! into one dispatcher call.

use tokio::time::Instant;
use velyr_engine_api::{Isolate, Value};

use crate::EventLoop;
use crate::timers::{TimerEntry, TimerId};

/// Most host timers handed to the dispatcher in one call.
const TIMER_BATCH_MAX: usize = 128;

impl<I: Isolate> EventLoop<I> {
    pub(crate) fn run_due_timers(&mut self) {
        let mut chunk = Vec::new();
        for id in self.timers.due(Instant::now()) {
            match self.timers.is_host_timer(id) {
                None => {}
                Some(true) => {
                    chunk.push(id);
                    if chunk.len() == TIMER_BATCH_MAX {
                        self.run_host_chunk(&mut chunk);
                    }
                }
                // A Rust-callback timer ends the chunk so execution order is kept.
                Some(false) => {
                    self.run_host_chunk(&mut chunk);
                    self.run_timer(id);
                }
            }
        }
        self.run_host_chunk(&mut chunk);
    }

    fn run_host_chunk(&mut self, chunk: &mut Vec<TimerId>) {
        let ids = std::mem::take(chunk);
        let dispatcher = self.host_timers.0.borrow().dispatcher.clone();
        match (ids.as_slice(), dispatcher) {
            ([], _) => {}
            ([_], _) | (_, None) => ids.into_iter().for_each(|id| self.run_timer(id)),
            (_, Some(dispatcher)) => self.run_batch(&ids, &dispatcher),
        }
    }

    fn run_timer(&mut self, id: TimerId) {
        let Some(entry) = self.timers.begin_run(id) else {
            return;
        };
        if entry.repeat.is_none() {
            // Spent once it starts: its callback may schedule a replacement at the limit.
            self.host_timers.0.borrow_mut().leases.remove(&id);
        }
        let (func, args) = self.call_target(&entry);
        if let Err(err) = self.isolate.call(&func, &Value::Undefined, &args) {
            tracing::warn!(message = %err.message, "timer callback threw");
        }
        // Before `end_run`: timers set by the callback inherit its nesting level, and a
        // `clear` of the running interval must stop the reschedule.
        self.apply_host_timer_commands();
        if let Some(callback) = self.timers.end_run(Instant::now(), entry) {
            self.host_timers.0.borrow_mut().leases.remove(&id);
            self.release(&callback);
        }
        self.isolate.perform_microtask_checkpoint();
        self.apply_host_timer_commands();
    }

    /// One dispatcher call `dispatcher(id0, level0, id1, level1, ...)` for several host timers.
    /// The dispatcher runs the microtask checkpoint itself between callbacks.
    fn run_batch(&mut self, ids: &[TimerId], dispatcher: &Value) {
        let entries = self.timers.begin_batch(ids);
        {
            // One-shots are spent once the batch starts, as in `run_timer`; the quota can
            // therefore be exceeded by at most `TIMER_BATCH_MAX` while the batch runs.
            let mut shared = self.host_timers.0.borrow_mut();
            for entry in entries.iter().filter(|e| e.repeat.is_none()) {
                shared.leases.remove(&entry.id);
            }
        }
        let args: Vec<Value> = entries
            .iter()
            .flat_map(|e| {
                [
                    Value::Number(e.id.0 as f64),
                    Value::Number(f64::from(e.level)),
                ]
            })
            .collect();
        if let Err(err) = self.isolate.call(dispatcher, &Value::Undefined, &args) {
            tracing::warn!(message = %err.message, "timer callback threw");
        }
        self.apply_host_timer_commands();
        self.timers.end_batch(Instant::now(), entries);
        self.isolate.perform_microtask_checkpoint();
        self.apply_host_timer_commands();
    }

    /// Host timers have no callback of their own (`Undefined`): they go through the dispatcher.
    fn call_target(&self, entry: &TimerEntry) -> (Value, Vec<Value>) {
        let dispatcher = self.host_timers.0.borrow().dispatcher.clone();
        match (&entry.callback, dispatcher) {
            (Value::Undefined, Some(dispatcher)) => {
                (dispatcher, vec![Value::Number(entry.id.0 as f64)])
            }
            _ => (entry.callback.clone(), Vec::new()),
        }
    }
}
