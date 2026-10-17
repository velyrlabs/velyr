use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use tracing::Span;
use velyr_engine_api::{CallContext, HostError, Isolate, Value};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionSender;
use velyr_runtime_permissions::{CapabilitySet, Request};
use velyr_runtime_resources::{IsolateId, LeakEntry, QuotaConfig, QuotaContext, ResourceTable};

/// Per-isolate context ops reach through the isolate slot. The slot hands out `&mut OpContext`,
/// so `resources` is owned here and needs no `Rc<RefCell<_>>`.
pub struct OpContext {
    pub isolate_id: u64,
    pub span: Span,
    pub sender: CompletionSender,
    pub permissions: Arc<CapabilitySet>,
    pub quotas: Arc<QuotaContext>,
    pub resources: ResourceTable,
    /// Values written by `Velyr.env.set`; the process environment is never mutated.
    pub env_overlay: HashMap<String, String>,
}

thread_local! {
    static LEAK_REPORT: RefCell<Vec<LeakEntry>> = const { RefCell::new(Vec::new()) };
}

/// Resources the last [`OpContext`] dropped on this thread still held open; clears the report.
/// Meant for the host that ran the isolate on this thread (see `RuntimeConfig::leak_collector`).
pub fn take_leak_report() -> Vec<LeakEntry> {
    LEAK_REPORT.with(|report| std::mem::take(&mut *report.borrow_mut()))
}

impl Drop for OpContext {
    fn drop(&mut self) {
        // Termination closes resources (specs/worker-api.md step 4); a normal exit still reports leaks.
        if self.sender.is_shutdown() {
            self.resources.close_all();
        }
        let leaks = self.resources.leak_report();
        LEAK_REPORT.with(|report| *report.borrow_mut() = leaks);
    }
}

impl OpContext {
    pub fn new(
        isolate_id: u64,
        sender: CompletionSender,
        permissions: Arc<CapabilitySet>,
        quotas: Arc<QuotaContext>,
    ) -> Self {
        let span = tracing::debug_span!("velyr.ops.call", isolate_id);
        Self {
            isolate_id,
            span,
            sender,
            permissions,
            resources: ResourceTable::new(IsolateId(isolate_id), quotas.clone()),
            quotas,
            env_overlay: HashMap::new(),
        }
    }

    /// Checks `request` for `op`. A denial emits `velyr.permission.denied` carrying the kind and
    /// the target length only, never the requested value.
    pub fn authorize(&self, op: &'static str, request: Request<'_>) -> Result<(), VelyrError> {
        let decision = self.permissions.check(&request);
        if !decision.is_allow() {
            tracing::warn!(
                target: "velyr.permission.denied",
                op,
                kind = request.kind(),
                target_redacted = %redacted(&request),
                isolate_id = self.isolate_id,
                "velyr.permission.denied"
            );
        }
        decision.into_result()
    }
}

fn redacted(request: &Request<'_>) -> String {
    let len = match request {
        Request::FsRead(t)
        | Request::FsWrite(t)
        | Request::NetConnect(t, _)
        | Request::NetListen(t, _)
        | Request::EnvRead(t)
        | Request::EnvWrite(t)
        | Request::ProcessSpawn(t)
        | Request::ImportLocal(t)
        | Request::ImportRemote(t)
        | Request::NativeLoad(t)
        | Request::InspectorBind(t)
        | Request::SysInfo(t) => t.len(),
    };
    format!("<redacted len={len}>")
}

/// Mandatory first step of every op whose `CapabilityKind` is not `None`. Fails closed when no
/// [`OpContext`] is installed.
pub fn authorize(
    ctx: &mut dyn CallContext,
    op: &'static str,
    request: Request<'_>,
) -> Result<(), HostError> {
    let op_ctx = ctx
        .isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))?;
    Ok(op_ctx.authorize(op, request)?)
}

/// The `index`-th argument as a string, or a `TypeError` naming `op`.
pub(crate) fn string_arg(args: &[Value], index: usize, op: &str) -> Result<String, HostError> {
    match args.get(index) {
        Some(Value::String(text)) => Ok(text.to_string_lossy()),
        _ => Err(HostError::TypeError(format!(
            "{op} expects argument {index} to be a string"
        ))),
    }
}

/// Stores the [`OpContext`] on `isolate` so ops can find the completion sender, the permissions
/// and the quotas. Pass `Arc::default()` for a deny-all set.
pub fn install_op_context(
    isolate: &mut impl Isolate,
    isolate_id: u64,
    sender: CompletionSender,
    permissions: Arc<CapabilitySet>,
    quotas: QuotaConfig,
) {
    let quotas = Arc::new(QuotaContext::new(quotas));
    isolate.set_slot(OpContext::new(isolate_id, sender, permissions, quotas));
}
