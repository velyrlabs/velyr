use std::future::Future;

use tokio_util::sync::CancellationToken;
use velyr_engine_api::{CallContext, HostError, Value};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::{Completion, CompletionValue};

use crate::OpContext;

/// Body of an async op: spawns `work` on the isolate thread's Tokio runtime and returns the
/// promise its result settles. `work` gets the op's cancellation token (cancelled on
/// terminate/shutdown); its output returns to the isolate thread as a completion.
pub fn op_async<F, Fut>(ctx: &mut dyn CallContext, work: F) -> Result<Value, HostError>
where
    F: FnOnce(CancellationToken) -> Fut,
    Fut: Future<Output = Result<CompletionValue, VelyrError>> + Send + 'static,
{
    let sender = ctx
        .isolate_slot::<OpContext>()
        .map(|op_ctx| op_ctx.sender.clone())
        .ok_or_else(|| HostError::TypeError("async ops need an installed OpContext".into()))?;
    let (promise, resolver) = ctx.create_promise();
    let guard = sender.begin_op();
    let future = work(guard.token());
    tokio::spawn(async move {
        let result = future.await;
        guard.send(Completion { resolver, result });
    });
    Ok(promise)
}
