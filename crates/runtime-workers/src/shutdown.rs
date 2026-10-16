//! Graceful shutdown of a supervised server (`velyr serve --workers=N`): stop accepting, let the
//! workers drain, and force them once the deadline passes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::supervisor::SupervisorHandle;

/// How long workers get to drain when the caller sets no deadline.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);

/// Shared by the thread that reacts to the signal ([`Drain::run`]) and the one that waits for
/// the supervisor ([`Drain::finished`]).
#[derive(Clone, Default)]
pub struct Drain {
    done: CancellationToken,
    forced: Arc<AtomicBool>,
}

impl Drain {
    /// The supervisor ended: every worker is done.
    pub fn finished(&self) {
        self.done.cancel();
    }

    /// Whether the deadline passed before the workers finished.
    pub fn was_forced(&self) -> bool {
        self.forced.load(Ordering::Acquire)
    }

    /// Stops accepting, then waits up to `deadline` for [`Drain::finished`]; past it, the workers
    /// are told to terminate and the shutdown counts as forced.
    pub async fn run(&self, handle: &SupervisorHandle, deadline: Duration) {
        handle.stop_accepting();
        tokio::select! {
            () = self.done.cancelled() => {}
            () = tokio::time::sleep(deadline) => {
                tracing::warn!(?deadline, "shutdown deadline passed, terminating the workers");
                self.forced.store(true, Ordering::Release);
                handle.shutdown();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::time::Instant;

    use super::*;
    use crate::supervisor::{Supervisor, SupervisorOptions, WorkerContext};

    fn start(
        run: impl Fn(WorkerContext) -> Result<(), String> + Send + Sync + 'static,
    ) -> Supervisor {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        Supervisor::start(SupervisorOptions::new(2, addr), run).unwrap()
    }

    #[tokio::test]
    async fn workers_that_drain_in_time_are_not_forced() {
        let supervisor = start(|_| {
            std::thread::sleep(Duration::from_millis(50));
            Ok(())
        });
        let (drain, handle) = (Drain::default(), supervisor.handle());
        let waiter = {
            let drain = drain.clone();
            std::thread::spawn(move || {
                supervisor.join().unwrap();
                drain.finished();
            })
        };
        drain.run(&handle, Duration::from_secs(10)).await;
        waiter.join().unwrap();
        assert!(!drain.was_forced());
    }

    #[tokio::test]
    async fn workers_still_running_at_the_deadline_are_terminated() {
        let supervisor = start(|ctx| {
            while !ctx.shutdown.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        });
        let (drain, handle) = (Drain::default(), supervisor.handle());
        let started = Instant::now();
        drain.run(&handle, Duration::from_millis(100)).await;
        supervisor.join().unwrap();
        assert!(drain.was_forced());
        assert!(started.elapsed() >= Duration::from_millis(100));
    }
}
