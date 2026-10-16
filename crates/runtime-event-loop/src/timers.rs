//! Timer queue with HTML timer semantics: ordered by (deadline, id), nesting clamp.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;
use std::time::Duration;

use tokio::time::Instant;
use velyr_engine_api::Value;

use crate::immediate::ImmediateHandle;

/// Nesting levels above this clamp the delay to [`MIN_NESTED_DELAY`] (HTML timer nesting).
const MAX_UNCLAMPED_NESTING: u32 = 5;
const MIN_NESTED_DELAY: Duration = Duration::from_millis(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimerId(pub u64);

pub(crate) struct TimerEntry {
    pub id: TimerId,
    pub callback: Value,
    pub repeat: Option<Duration>,
    pub level: u32,
    pub refd: bool,
}

#[derive(Default)]
pub(crate) struct Timers {
    queue: BTreeMap<(Instant, TimerId), TimerEntry>,
    deadlines: HashMap<TimerId, Instant>,
    /// Nesting level of the callback being run (0 outside callbacks).
    running_level: u32,
    running: Option<TimerId>,
    running_cleared: bool,
}

impl Timers {
    pub fn add(
        &mut self,
        id: TimerId,
        now: Instant,
        delay: Duration,
        repeat: bool,
        callback: Value,
    ) -> TimerId {
        let entry = TimerEntry {
            id,
            callback,
            repeat: repeat.then_some(delay),
            level: self.running_level + 1,
            refd: true,
        };
        self.insert(now, delay, entry);
        id
    }

    fn insert(&mut self, now: Instant, delay: Duration, entry: TimerEntry) {
        let delay = if entry.level > MAX_UNCLAMPED_NESTING {
            delay.max(MIN_NESTED_DELAY)
        } else {
            delay
        };
        let deadline = now + delay;
        self.deadlines.insert(entry.id, deadline);
        self.queue.insert((deadline, entry.id), entry);
    }

    /// Removes the timer; returns its callback so the caller can release it.
    pub fn clear(&mut self, id: TimerId) -> Option<Value> {
        if self.running == Some(id) {
            self.running_cleared = true;
        }
        let deadline = self.deadlines.remove(&id)?;
        self.queue.remove(&(deadline, id)).map(|e| e.callback)
    }

    pub fn set_ref(&mut self, id: TimerId, refd: bool) {
        let Some(deadline) = self.deadlines.get(&id) else {
            return;
        };
        if let Some(entry) = self.queue.get_mut(&(*deadline, id)) {
            entry.refd = refd;
        }
    }

    pub fn has_ref(&self) -> bool {
        self.queue.values().any(|e| e.refd)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.queue.keys().next().map(|(deadline, _)| *deadline)
    }

    /// Ids due at `now`, in execution order. Timers added later wait for the next tick.
    pub fn due(&self, now: Instant) -> Vec<TimerId> {
        self.queue
            .range(..=(now, TimerId(u64::MAX)))
            .map(|((_, id), _)| *id)
            .collect()
    }

    /// Takes a due timer out of the queue and marks it as running.
    pub fn begin_run(&mut self, id: TimerId) -> Option<TimerEntry> {
        let deadline = self.deadlines.remove(&id)?;
        let entry = self.queue.remove(&(deadline, id))?;
        self.running_level = entry.level;
        self.running = Some(id);
        self.running_cleared = false;
        Some(entry)
    }

    /// Ends a run: intervals are rescheduled after the callback unless cleared.
    /// Returns the callback when the timer is finished and should be released.
    pub fn end_run(&mut self, now: Instant, mut entry: TimerEntry) -> Option<Value> {
        self.running_level = 0;
        self.running = None;
        let Some(delay) = entry.repeat.filter(|_| !self.running_cleared) else {
            return Some(entry.callback);
        };
        entry.level += 1;
        self.insert(now, delay, entry);
        None
    }
}

pub(crate) enum TimerCommand {
    Set {
        id: TimerId,
        at: Instant,
        delay: Duration,
        repeat: bool,
    },
    Clear(TimerId),
    SetRef(TimerId, bool),
}

#[derive(Default)]
pub(crate) struct HostTimers {
    next_id: u64,
    pub commands: Vec<TimerCommand>,
    pub dispatcher: Option<Value>,
    pub immediates: ImmediateHandle,
}

impl HostTimers {
    pub fn alloc_id(&mut self) -> TimerId {
        self.next_id += 1;
        TimerId(self.next_id)
    }
}

/// Isolate-thread handle that lets host functions schedule timers on the loop that owns the
/// isolate. Calls are queued and applied by the loop right after the JS that issued them, so
/// ids are returned synchronously and `clear` before the first tick is honored.
///
/// Timers created here carry no callback: when they fire the loop calls the dispatcher set
/// with [`set_dispatcher`](Self::set_dispatcher) as `dispatcher(id)`.
#[derive(Clone, Default)]
pub struct TimerHandle(pub(crate) Rc<RefCell<HostTimers>>);

impl TimerHandle {
    pub fn set(&self, delay: Duration, repeat: bool) -> TimerId {
        let mut shared = self.0.borrow_mut();
        let id = shared.alloc_id();
        shared.commands.push(TimerCommand::Set {
            id,
            at: Instant::now(),
            delay,
            repeat,
        });
        id
    }

    pub fn clear(&self, id: TimerId) {
        self.0.borrow_mut().commands.push(TimerCommand::Clear(id));
    }

    pub fn set_ref(&self, id: TimerId, refd: bool) {
        let command = TimerCommand::SetRef(id, refd);
        self.0.borrow_mut().commands.push(command);
    }

    /// Queue for `setImmediate`-style callbacks, run in the loop's check phase.
    pub fn immediates(&self) -> ImmediateHandle {
        self.0.borrow().immediates.clone()
    }

    /// The function the loop calls with `Number(id)` when a timer made through this handle fires.
    pub fn set_dispatcher(&self, dispatcher: Value) {
        self.0.borrow_mut().dispatcher = Some(dispatcher);
    }
}
