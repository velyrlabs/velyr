//! How often a failing server worker may be restarted.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Longest wait before a restart, however many failures piled up.
const MAX_BACKOFF: Duration = Duration::from_secs(2);

/// Allows `limit` restarts per `window`; each one waits twice as long as the previous failure
/// in the window (starting at `base`, capped at [`MAX_BACKOFF`]).
#[derive(Debug)]
pub struct RestartPolicy {
    limit: u32,
    window: Duration,
    base: Duration,
    failures: VecDeque<Instant>,
}

impl RestartPolicy {
    pub fn new(limit: u32, window: Duration, base: Duration) -> Self {
        Self {
            limit,
            window,
            base,
            failures: VecDeque::new(),
        }
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// Records a failure at `now`. `Some(wait)` is how long to wait before restarting; `None`
    /// means the budget is spent and the supervisor should give up.
    pub fn record(&mut self, now: Instant) -> Option<Duration> {
        while self
            .failures
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= self.window)
        {
            self.failures.pop_front();
        }
        if self.failures.len() >= self.limit as usize {
            return None;
        }
        let doublings = u32::try_from(self.failures.len())
            .unwrap_or(u32::MAX)
            .min(16);
        self.failures.push_back(now);
        Some(self.base.saturating_mul(1 << doublings).min(MAX_BACKOFF))
    }
}
