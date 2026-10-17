//! Lets engine threads wake the event loop of an isolate's thread (RT-15).
//!
//! The engine registers one waker per isolate thread when it creates the isolate; the event loop
//! built on that thread picks it up and installs the function that wakes it. Engines that never
//! register one leave the loop polling.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, PoisonError};

type WakeFn = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Default)]
pub struct PlatformWaker(Arc<Mutex<Option<WakeFn>>>);

thread_local! {
    static CURRENT: RefCell<Option<PlatformWaker>> = const { RefCell::new(None) };
}

impl PlatformWaker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes `self` the waker of the calling thread, replacing an earlier one.
    pub fn register_current(&self) {
        CURRENT.with_borrow_mut(|current| *current = Some(self.clone()));
    }

    /// The waker registered on the calling thread, if any.
    pub fn current() -> Option<Self> {
        CURRENT.with_borrow(Clone::clone)
    }

    /// Installs what [`wake`](Self::wake) calls (the event loop's side).
    pub fn set_wake_fn(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(wake));
    }

    /// Safe from any thread. A no-op until a loop installed its wake function.
    pub fn wake(&self) {
        let wake = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(wake) = wake {
            wake();
        }
    }
}

impl std::fmt::Debug for PlatformWaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlatformWaker")
    }
}
