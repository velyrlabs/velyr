//! Foreground tasks of the V8 platform, queued per isolate and run by `run_platform_tasks`. A
//! post wakes the isolate's event loop through its [`PlatformWaker`] (RT-15), so the loop
//! sleeps instead of polling while background work (async wasm compiles) is outstanding.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use velyr_engine_api::PlatformWaker;

struct Queued {
    due: Instant,
    task: v8::Task,
}

#[derive(Default)]
struct IsolateQueue {
    tasks: Vec<Queued>,
    waker: Option<PlatformWaker>,
}

/// Keyed by the address of the C++ isolate the task was posted for.
static QUEUES: LazyLock<Mutex<HashMap<usize, IsolateQueue>>> = LazyLock::new(Mutex::default);

fn queues() -> MutexGuard<'static, HashMap<usize, IsolateQueue>> {
    QUEUES.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct WakePlatform;

impl v8::PlatformImpl for WakePlatform {
    fn post_task(&self, isolate: *mut c_void, task: v8::Task) {
        enqueue(isolate, task, 0.0);
    }

    fn post_non_nestable_task(&self, isolate: *mut c_void, task: v8::Task) {
        enqueue(isolate, task, 0.0);
    }

    // shortcut: a delayed task runs on the first loop tick after it is due, it does not wake the
    // loop itself (the default platform never ran them unpumped either); add a deadline if JS
    // ever depends on one.
    fn post_delayed_task(&self, isolate: *mut c_void, task: v8::Task, delay: f64) {
        enqueue(isolate, task, delay);
    }

    fn post_non_nestable_delayed_task(&self, isolate: *mut c_void, task: v8::Task, delay: f64) {
        enqueue(isolate, task, delay);
    }

    // Idle tasks are disabled at `new_custom_platform`, so V8 posts none; dropping cancels one.
    fn post_idle_task(&self, _isolate: *mut c_void, _task: v8::IdleTask) {}
}

fn enqueue(isolate: *mut c_void, task: v8::Task, delay_seconds: f64) {
    let due = Instant::now() + Duration::try_from_secs_f64(delay_seconds).unwrap_or_default();
    let waker = {
        let mut queues = queues();
        let queue = queues.entry(isolate as usize).or_default();
        queue.tasks.push(Queued { due, task });
        queue.waker.clone()
    };
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// The key V8 posts `isolate`'s tasks under.
#[allow(unsafe_code)]
pub(crate) fn key(isolate: &v8::Isolate) -> usize {
    // SAFETY: only the address is read, never dereferenced; `UnsafeRawIsolatePtr` is a
    // `repr(transparent)` wrapper of the C++ `v8::Isolate*` that the task runner passes on posts.
    let raw: *mut c_void = unsafe { std::mem::transmute(isolate.as_raw_isolate_ptr()) };
    raw as usize
}

/// Wakes `waker` on every task posted for the isolate at `key`.
pub(crate) fn register(key: usize, waker: PlatformWaker) {
    queues().entry(key).or_default().waker = Some(waker);
}

/// Runs the isolate's due tasks on its thread; `true` if any ran. Tasks they post wait for the
/// next call, which their wake-up schedules. Reporting `true` keeps the loop parked until a
/// wake-up, so one is sent: without it a last batch that posts nothing parks the loop for good.
pub(crate) fn run_due(key: usize) -> bool {
    let now = Instant::now();
    let due: Vec<Queued> = match queues().get_mut(&key) {
        Some(queue) => queue
            .tasks
            .extract_if(.., |queued| queued.due <= now)
            .collect(),
        None => return false,
    };
    let ran = !due.is_empty();
    for queued in due {
        queued.task.run();
    }
    if ran {
        wake(key);
    }
    ran
}

/// A task for the isolate at `key` is due: the loop must run it before it may exit. Wakes the
/// loop, as a delayed task that just became due sent no wake-up of its own.
pub(crate) fn has_due(key: usize) -> bool {
    let now = Instant::now();
    let due = queues()
        .get(&key)
        .is_some_and(|queue| queue.tasks.iter().any(|queued| queued.due <= now));
    if due {
        wake(key);
    }
    due
}

fn wake(key: usize) {
    let waker = queues().get(&key).and_then(|queue| queue.waker.clone());
    if let Some(waker) = waker {
        waker.wake();
    }
}

/// Forgets the isolate at `key` once it is disposed, dropping (cancelling) its leftover tasks,
/// so a later isolate at the same address never runs them.
pub(crate) struct QueueGuard(pub(crate) usize);

impl Drop for QueueGuard {
    fn drop(&mut self) {
        let leftover = queues().remove(&self.0);
        drop(leftover);
    }
}
