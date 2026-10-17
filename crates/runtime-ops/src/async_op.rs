use std::future::Future;

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, Value};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::{Completion, CompletionValue};

use crate::OpContext;

/// Blocking jobs running or queued to run at once, across every isolate of the process.
const BLOCKING_PERMITS: usize = 64;
static BLOCKING: Semaphore = Semaphore::const_new(BLOCKING_PERMITS);

/// Runs `work` on Tokio's blocking pool once one of the process-wide permits is free. The pool
/// itself is capped per isolate (`RuntimeConfig::max_blocking_threads`). Dropping the future
/// while it waits for a permit releases its place in the queue; once `work` started, the permit
/// is held until it returns.
pub async fn blocking<T, F>(work: F) -> Result<T, VelyrError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let permit = BLOCKING
        .acquire()
        .await
        .expect("the semaphore is never closed");
    tokio::task::spawn_blocking(move || {
        let output = work();
        drop(permit);
        output
    })
    .await
    .map_err(|err| VelyrError::internal(err.to_string()))
}

/// Body of an async op: spawns `work` on the isolate thread's Tokio runtime and returns the
/// promise its result settles. `work` gets the op's cancellation token (cancelled on
/// terminate/shutdown); its output returns to the isolate thread as a completion.
pub fn op_async<F, Fut>(ctx: &mut dyn CallContext, work: F) -> Result<Value, HostError>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = Result<CompletionValue, VelyrError>> + Send + 'static,
{
    spawn_op(ctx, None, work)
}

/// [`op_async`] for an op of the handle `key`: it stops keeping the loop alive while the handle
/// is unref'd (`CompletionSender::set_handle_ref`).
pub fn op_async_on<F, Fut>(ctx: &mut dyn CallContext, key: u64, work: F) -> Result<Value, HostError>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = Result<CompletionValue, VelyrError>> + Send + 'static,
{
    spawn_op(ctx, Some(key), work)
}

fn spawn_op<F, Fut>(
    ctx: &mut dyn CallContext,
    key: Option<u64>,
    work: F,
) -> Result<Value, HostError>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = Result<CompletionValue, VelyrError>> + Send + 'static,
{
    let sender = ctx
        .isolate_slot::<OpContext>()
        .map(|op_ctx| op_ctx.sender.clone())
        .ok_or_else(|| HostError::TypeError("async ops need an installed OpContext".into()))?;
    let (promise, resolver) = ctx.create_promise();
    let guard = key.map_or_else(|| sender.begin_op(), |key| sender.begin_op_on(key));
    let future = work(guard.token());
    tokio::spawn(async move {
        let result = future.await;
        guard.send(Completion { resolver, result });
    });
    Ok(promise)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dropping_a_queued_job_frees_its_permit() {
        let held = BLOCKING
            .acquire_many(BLOCKING_PERMITS as u32)
            .await
            .unwrap();
        let queued = tokio::spawn(blocking(|| ()));
        tokio::task::yield_now().await;
        queued.abort();
        let _ = queued.await;
        drop(held);
        assert_eq!(BLOCKING.available_permits(), BLOCKING_PERMITS);
        assert_eq!(blocking(|| 7).await.unwrap(), 7);
        assert_eq!(BLOCKING.available_permits(), BLOCKING_PERMITS);
    }
}
