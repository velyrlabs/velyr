//! CPU watchdog: terminates the isolate when one synchronous execution outlives its budget.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use velyr_engine_api::TerminationHandle;

#[derive(Default)]
struct Slot {
    /// Start of the execution being timed; `None` between executions.
    started: Option<Instant>,
    shutdown: bool,
}

#[derive(Default)]
struct State {
    slot: Mutex<Slot>,
    wake: Condvar,
    fired: AtomicBool,
}

impl State {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        self.slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) struct Watchdog {
    state: Arc<State>,
    monitor: Option<JoinHandle<()>>,
}

/// Times one execution; ends it on drop.
pub(crate) struct TickGuard<'a>(&'a State);

impl Watchdog {
    pub(crate) fn spawn(budget: Duration, isolate: TerminationHandle) -> std::io::Result<Self> {
        let state = Arc::new(State::default());
        let monitor = thread::Builder::new()
            .name("velyr-watchdog".into())
            .spawn({
                let state = Arc::clone(&state);
                move || monitor(&state, budget, &isolate)
            })?;
        Ok(Self {
            state,
            monitor: Some(monitor),
        })
    }

    /// Starts the clock for one execution.
    pub(crate) fn begin_tick(&self) -> TickGuard<'_> {
        self.state.fired.store(false, Ordering::Release);
        self.state.lock().started = Some(Instant::now());
        self.state.wake.notify_one();
        TickGuard(&self.state)
    }

    /// Whether the budget terminated the latest execution.
    pub(crate) fn fired(&self) -> bool {
        self.state.fired.load(Ordering::Acquire)
    }
}

impl Drop for TickGuard<'_> {
    fn drop(&mut self) {
        self.0.lock().started = None;
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.state.lock().shutdown = true;
        self.state.wake.notify_one();
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }
}

fn monitor(state: &State, budget: Duration, isolate: &TerminationHandle) {
    let mut slot = state.lock();
    while !slot.shutdown {
        slot = match slot.started {
            None => state.wake.wait(slot).unwrap_or_else(|p| p.into_inner()),
            Some(started) => match budget.checked_sub(started.elapsed()) {
                Some(left) => {
                    state
                        .wake
                        .wait_timeout(slot, left)
                        .unwrap_or_else(|p| p.into_inner())
                        .0
                }
                None => {
                    // Under the lock: the tick cannot end between the check and the terminate.
                    state.fired.store(true, Ordering::Release);
                    isolate.terminate();
                    slot.started = None;
                    slot
                }
            },
        };
    }
}
