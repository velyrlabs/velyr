//! Timer queue with HTML timer semantics: ordered by (deadline, id), nesting clamp.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use tokio::time::Instant;
use velyr_engine_api::Value;
use velyr_runtime_errors::VelyrError;

use crate::immediate::ImmediateHandle;

/// Nesting levels above this clamp the delay to [`MIN_NESTED_DELAY`] (HTML timer nesting).
const MAX_UNCLAMPED_NESTING: u32 = 5;
const MIN_NESTED_DELAY: Duration = Duration::from_millis(4);

/// Type-erased quota reservation held by a live timer; dropping it releases the quota.
pub type TimerLease = Box<dyn Any>;
/// Reserves one unit of the host's timer quota, or fails with the quota error.
pub type TimerQuotaFn = Box<dyn Fn() -> Result<TimerLease, VelyrError>>;

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
    /// Queue entries with `refd` set, so `has_ref` does not scan the queue.
    refd_count: usize,
    /// Ids taken out of the queue by `begin_batch` and not yet handed back by `end_batch`.
    batch: HashSet<TimerId>,
    batch_cleared: HashSet<TimerId>,
}

impl Timers {
    pub fn add(
        &mut self,
        id: TimerId,
        now: Instant,
        delay: Duration,
        repeat: bool,
        callback: Value,
        level: Option<u32>,
    ) -> TimerId {
        let entry = TimerEntry {
            id,
            callback,
            repeat: repeat.then_some(delay),
            level: level.unwrap_or(self.running_level + 1),
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
        self.refd_count += usize::from(entry.refd);
        self.queue.insert((deadline, entry.id), entry);
    }

    fn remove(&mut self, id: TimerId) -> Option<TimerEntry> {
        let deadline = self.deadlines.remove(&id)?;
        let entry = self.queue.remove(&(deadline, id))?;
        self.refd_count -= usize::from(entry.refd);
        Some(entry)
    }

    /// Removes the timer; returns its callback so the caller can release it.
    pub fn clear(&mut self, id: TimerId) -> Option<Value> {
        if self.running == Some(id) {
            self.running_cleared = true;
        }
        if self.batch.contains(&id) {
            self.batch_cleared.insert(id);
        }
        self.remove(id).map(|e| e.callback)
    }

    pub fn set_ref(&mut self, id: TimerId, refd: bool) {
        let Some(deadline) = self.deadlines.get(&id) else {
            return;
        };
        let Some(entry) = self.queue.get_mut(&(*deadline, id)) else {
            return;
        };
        if entry.refd != refd {
            entry.refd = refd;
            if refd {
                self.refd_count += 1;
            } else {
                self.refd_count -= 1;
            }
        }
    }

    pub fn has_ref(&self) -> bool {
        debug_assert_eq!(
            self.refd_count,
            self.queue.values().filter(|e| e.refd).count()
        );
        self.refd_count > 0
    }

    /// `Some(true)` when `id` is queued and has no callback of its own (a host timer).
    pub fn is_host_timer(&self, id: TimerId) -> Option<bool> {
        let deadline = self.deadlines.get(&id)?;
        let entry = self.queue.get(&(*deadline, id))?;
        Some(matches!(entry.callback, Value::Undefined))
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
        let entry = self.remove(id)?;
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

    /// Takes the queued `ids` out of the queue for one grouped dispatch; ids gone are skipped.
    pub fn begin_batch(&mut self, ids: &[TimerId]) -> Vec<TimerEntry> {
        self.batch_cleared.clear();
        self.batch.extend(ids);
        ids.iter().filter_map(|id| self.remove(*id)).collect()
    }

    /// Ends a batch: intervals not cleared meanwhile are rescheduled one nesting level deeper.
    pub fn end_batch(&mut self, now: Instant, entries: Vec<TimerEntry>) {
        self.batch.clear();
        for mut entry in entries {
            let Some(delay) = entry
                .repeat
                .filter(|_| !self.batch_cleared.contains(&entry.id))
            else {
                continue;
            };
            entry.level += 1;
            self.insert(now, delay, entry);
        }
        self.batch_cleared.clear();
    }
}

pub(crate) enum TimerCommand {
    Set {
        id: TimerId,
        at: Instant,
        delay: Duration,
        repeat: bool,
        /// Nesting level for a timer created inside a batched callback; `None` uses the running one.
        level: Option<u32>,
    },
    Clear(TimerId),
    SetRef(TimerId, bool),
}

#[derive(Default)]
pub(crate) struct HostTimers {
    next_id: u64,
    pub commands: Vec<TimerCommand>,
    pub dispatcher: Option<Value>,
    pub quota: Option<TimerQuotaFn>,
    /// Quota leases of live host timers, released by `clear` right away (the queue applies it later).
    pub leases: HashMap<TimerId, TimerLease>,
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
    /// Schedules without consulting the quota (host code and tests).
    pub fn set(&self, delay: Duration, repeat: bool) -> TimerId {
        self.push_set(delay, repeat, None)
    }

    /// Like [`set`](Self::set), but reserves the quota first; the lease lives as long as the timer.
    pub fn try_set(&self, delay: Duration, repeat: bool) -> Result<TimerId, VelyrError> {
        self.try_set_nested(delay, repeat, None)
    }

    /// [`try_set`](Self::try_set) for a caller running inside a batched dispatch: `level` is
    /// the timer's own nesting level, which the loop cannot know from a single running timer.
    pub fn try_set_nested(
        &self,
        delay: Duration,
        repeat: bool,
        level: Option<u32>,
    ) -> Result<TimerId, VelyrError> {
        let lease = self
            .0
            .borrow()
            .quota
            .as_ref()
            .map(|reserve| reserve())
            .transpose()?;
        let id = self.push_set(delay, repeat, level);
        if let Some(lease) = lease {
            self.0.borrow_mut().leases.insert(id, lease);
        }
        Ok(id)
    }

    /// Installs the quota consulted by [`try_set`](Self::try_set).
    pub fn set_quota(&self, quota: TimerQuotaFn) {
        self.0.borrow_mut().quota = Some(quota);
    }

    fn push_set(&self, delay: Duration, repeat: bool, level: Option<u32>) -> TimerId {
        let mut shared = self.0.borrow_mut();
        let id = shared.alloc_id();
        shared.commands.push(TimerCommand::Set {
            id,
            at: Instant::now(),
            delay,
            repeat,
            level,
        });
        id
    }

    pub fn clear(&self, id: TimerId) {
        let mut shared = self.0.borrow_mut();
        shared.leases.remove(&id);
        shared.commands.push(TimerCommand::Clear(id));
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

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn add(timers: &mut Timers, id: u64, repeat: bool) {
        timers.add(
            TimerId(id),
            Instant::now(),
            MS,
            repeat,
            Value::Undefined,
            None,
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn has_ref_tracks_set_ref_without_scanning() {
        let mut timers = Timers::default();
        for id in 1..=10_001 {
            add(&mut timers, id, true);
        }
        for id in 1..=10_000 {
            timers.set_ref(TimerId(id), false);
        }
        assert!(timers.has_ref());
        timers.set_ref(TimerId(10_001), false);
        assert!(!timers.has_ref());
        timers.set_ref(TimerId(7), true);
        assert!(timers.has_ref());
        timers.clear(TimerId(7));
        assert!(!timers.has_ref());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn batch_clear_of_running_interval_does_not_reschedule() {
        let mut timers = Timers::default();
        add(&mut timers, 1, true);
        add(&mut timers, 2, true);
        let entries = timers.begin_batch(&[TimerId(1), TimerId(2)]);
        assert_eq!(entries.len(), 2);
        assert!(!timers.has_ref());
        timers.clear(TimerId(1));
        timers.end_batch(Instant::now(), entries);
        assert!(timers.next_deadline().is_some());
        assert_eq!(timers.due(Instant::now() + MS * 10), vec![TimerId(2)]);
        let rescheduled = timers.begin_batch(&[TimerId(2)]);
        assert_eq!(rescheduled[0].level, 2);
    }
}
