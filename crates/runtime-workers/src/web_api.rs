//! Web-facing messaging (ADR-014): `Worker`, `MessageChannel`/`MessagePort`, `BroadcastChannel`
//! and the worker global scope, as JS on top of the `op_port_*`, `op_bc_*` and `op_worker_*`
//! ops below. Endpoints live in a per-isolate table and move between isolates inside packets.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, Isolate, JsException, JsString, Source, Value,
};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::{Completion, CompletionValue};
use velyr_runtime_modules::{FsResolver, ModuleId, ModuleResolver, ResolveContext};
use velyr_runtime_ops::{OpContext, authorize, op_async};
use velyr_runtime_permissions::{CapabilitySet, Request};

use crate::channel::{Endpoint, Fill, Inbox, Message, Packet, new_group};
use crate::port::{port, request_exit, transfer_list};
use crate::report::ErrorReport;
use crate::{ParentContext, WorkerHandle};

/// Bootstrap script; its completion value is the toolkit [`start_web_api`] takes.
pub const MESSAGING_JS: &str = include_str!("../js/messaging.js");
/// Bootstrap script; its completion value is the `install(toolkit)` [`start_web_api`] calls.
pub const WORKER_JS: &str = include_str!("../js/worker.js");
const SCOPE_JS: &str = include_str!("../js/worker_scope.js");

/// Upper bound for waiting on a terminated worker thread.
const TERMINATE_WAIT: Duration = Duration::from_secs(5);

/// What `new Worker(url, { velyr: { permissions } })` asks the host to start.
pub struct SpawnRequest {
    /// Canonical path of the entry module, already authorized for the parent.
    pub entry: PathBuf,
    pub name: String,
    /// `JSON.stringify` of `velyr.permissions`; `None` means "inherit the parent's authority".
    pub permissions_json: Option<String>,
    /// Isolate group of the spawning isolate; pass it to [`install_web_api`] so the worker's
    /// `BroadcastChannel`s stay in the family.
    pub group: u64,
    /// `velyr.heapLimitMb` in bytes; `None` keeps the host's default. Pass it as
    /// `WorkerOptions::heap_limit_bytes`, which caps it at the parent's limit.
    pub heap_limit_bytes: Option<usize>,
}

/// Worker identity for [`install_web_api`].
pub struct WorkerScope<'a> {
    pub name: &'a str,
    pub group: u64,
}

/// The isolate group of this isolate (see [`SpawnRequest::group`]).
struct Group(u64);

/// Builds the worker's engine, host functions and globals (the host knows them, not this crate).
pub type SpawnFn =
    dyn Fn(&ParentContext, SpawnRequest) -> Result<WorkerHandle, VelyrError> + Send + Sync;

/// What an isolate needs to spawn workers; share one value with every worker for nesting.
#[derive(Clone)]
pub struct WorkerHost {
    pub deployment_policy: Arc<CapabilitySet>,
    pub heap_limit_bytes: Option<usize>,
    pub spawn: Arc<SpawnFn>,
}

#[derive(Default)]
struct Ports {
    next: u32,
    ends: HashMap<u32, Endpoint>,
}

impl Ports {
    fn insert(&mut self, endpoint: Endpoint) -> u32 {
        self.next += 1;
        self.ends.insert(self.next, endpoint);
        self.next
    }

    fn get(&self, id: u32) -> Result<&Endpoint, HostError> {
        self.ends
            .get(&id)
            .ok_or_else(|| HostError::TypeError("the port is closed or was transferred".into()))
    }

    /// Removes `ids` after checking every one can move; nothing changes on failure.
    fn take_transferable(&mut self, ids: &[u32]) -> Result<Vec<Endpoint>, HostError> {
        for (index, id) in ids.iter().enumerate() {
            if !self.get(*id)?.is_transferable() || ids[..index].contains(id) {
                return Err(HostError::TypeError("port cannot be transferred".into()));
            }
        }
        Ok(ids.iter().filter_map(|id| self.ends.remove(id)).collect())
    }
}

/// Child workers of this isolate. Dropping the isolate stops them and waits for their threads.
#[derive(Default)]
struct Workers {
    next: u32,
    map: HashMap<u32, WorkerHandle>,
}

impl Drop for Workers {
    fn drop(&mut self) {
        for (_, worker) in self.map.drain() {
            let _ = worker.terminate(TERMINATE_WAIT);
        }
    }
}

struct Host(Option<WorkerHost>);

macro_rules! def {
    ($name:ident, $arity:literal) => {
        HostFunctionDef {
            name: stringify!($name),
            arity: $arity,
            callback: $name,
        }
    };
}

/// Every op the web API needs. The worker-only ones (`op_worker_scope_port`, ...) are part of
/// the same list: they fail with a `TypeError` outside a worker and the scope bootstrap
/// deletes them. The three `op_worker_*` messaging ops of [`crate::worker_host_functions`] are
/// added by `WorkerHandle::spawn` itself and must not be listed here.
pub fn web_api_host_functions() -> Vec<HostFunctionDef> {
    vec![
        def!(op_port_pair, 0),
        def!(op_port_post, 4),
        def!(op_port_recv, 2),
        def!(op_port_take, 1),
        def!(op_port_interrupt, 1),
        def!(op_port_close, 1),
        def!(op_bc_open, 1),
        def!(op_worker_spawn, 5),
        def!(op_worker_terminate, 1),
        def!(op_worker_scope_port, 0),
        def!(op_worker_close, 0),
        def!(op_worker_report_error, 2),
    ]
}

/// Installs `Worker`, `MessageChannel`, `MessagePort`, `BroadcastChannel`, `MessageEvent` and
/// `ErrorEvent`, plus `Velyr.unrefWorker`. With `scope` the isolate is a worker (of the isolate
/// group that spawned it; a main isolate starts a new group) and also
/// gets `self.postMessage`, `onmessage`, `close()`, `reportError` and `name`. Call after
/// `install_op_context` and `install_events`; `host` is `None` where spawning is unavailable.
pub fn install_web_api(
    isolate: &mut impl Isolate,
    host: Option<WorkerHost>,
    scope: Option<WorkerScope<'_>>,
) -> Result<(), JsException> {
    install_web_api_slots(isolate, host, scope.as_ref());
    let toolkit = eval(isolate, "velyr:messaging", MESSAGING_JS)?;
    let install = eval(isolate, "velyr:worker", WORKER_JS)?;
    start_web_api(isolate, &toolkit, &install, scope)
}

/// The Rust half of [`install_web_api`], for an isolate whose [`MESSAGING_JS`] and [`WORKER_JS`]
/// already ran (startup snapshot); follow it with [`start_web_api`].
pub fn install_web_api_slots(
    isolate: &mut impl Isolate,
    host: Option<WorkerHost>,
    scope: Option<&WorkerScope<'_>>,
) {
    isolate.set_slot(Group(scope.map_or_else(new_group, |scope| scope.group)));
    isolate.set_slot(Ports::default());
    isolate.set_slot(Workers::default());
    isolate.set_slot(Host(host));
}

/// Defines the globals from the completion values of [`MESSAGING_JS`] (`toolkit`) and
/// [`WORKER_JS`] (`install`), plus the worker global scope when `scope` is set.
pub fn start_web_api(
    isolate: &mut impl Isolate,
    toolkit: &Value,
    install: &Value,
    scope: Option<WorkerScope<'_>>,
) -> Result<(), JsException> {
    isolate.call(install, &Value::Undefined, std::slice::from_ref(toolkit))?;
    if let Some(scope) = scope {
        let install = eval(isolate, "velyr:worker-scope", SCOPE_JS)?;
        let name = Value::String(JsString::from(scope.name));
        isolate.call(&install, &Value::Undefined, &[toolkit.clone(), name])?;
    }
    Ok(())
}

fn eval(isolate: &mut impl Isolate, name: &str, text: &str) -> Result<Value, JsException> {
    isolate.eval_script(&Source {
        name: name.into(),
        text: text.into(),
    })
}

fn ports(ctx: &mut dyn CallContext) -> Result<&mut Ports, HostError> {
    ctx.isolate_slot::<Ports>()
        .ok_or_else(|| HostError::TypeError("the web messaging API is not installed".into()))
}

fn id_of(value: Option<&Value>) -> Result<u32, HostError> {
    match value {
        Some(Value::Number(n)) if n.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(n) => {
            Ok(*n as u32)
        }
        _ => Err(HostError::TypeError("expected a port id".into())),
    }
}

fn number(n: u32) -> Value {
    Value::Number(f64::from(n))
}

/// `op_port_pair() -> [idA, idB]`: two entangled ports.
fn op_port_pair(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let (first, second) = Endpoint::pair();
    let table = ports(ctx)?;
    let ids = [table.insert(first), table.insert(second)];
    Ok(ctx.create_array(&ids.map(number)))
}

/// `op_bc_open(name) -> id`: joins the broadcast channel `name` of this isolate's group.
fn op_bc_open(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(name)) = args.first() else {
        return Err(HostError::TypeError("op_bc_open expects a name".into()));
    };
    let group = ctx
        .isolate_slot::<Group>()
        .map(|group| group.0)
        .ok_or_else(|| HostError::TypeError("the web messaging API is not installed".into()))?;
    let endpoint = Endpoint::broadcast(group, &name.to_string_lossy());
    Ok(number(ports(ctx)?.insert(endpoint)))
}

/// `op_port_post(id, value, buffers, portIds)`: structured-clones `value` to the peer of `id`,
/// moving the `ArrayBuffer`s in `buffers` and the ports in `portIds` with it.
fn op_port_post(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = id_of(args.first())?;
    let value = args.get(1).cloned().unwrap_or(Value::Undefined);
    let buffers = transfer_list(ctx, args.get(2))?;
    let moved: Vec<u32> = transfer_list(ctx, args.get(3))?
        .iter()
        .map(|port| id_of(Some(port)))
        .collect::<Result<_, _>>()?;
    ports(ctx)?.get(id)?;
    let value = ctx.serialize(&value, &buffers)?;
    let table = ports(ctx)?;
    if moved.contains(&id) {
        return Err(HostError::TypeError(
            "a port cannot be sent over itself".into(),
        ));
    }
    let moved = table.take_transferable(&moved)?;
    for endpoint in &moved {
        endpoint.inbox.interrupt();
    }
    table.get(id)?.send(Packet::Message(Message {
        value,
        ports: moved,
    }))?;
    Ok(Value::Undefined)
}

/// `op_port_recv(id, refd) -> Promise<0|1|2>`: 1 when a packet is ready for `op_port_take`,
/// 0 when the channel closed, 2 when interrupted. A ref'd wait keeps the event loop alive;
/// an unref'd one only delivers while something else does.
fn op_port_recv(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let inbox = ports(ctx)?.get(id_of(args.first())?)?.inbox.clone();
    if matches!(args.get(1), Some(Value::Bool(false))) {
        return recv_unref(ctx, inbox);
    }
    op_async(ctx, |_token| async move { Ok(code(inbox.fill().await)) })
}

fn code(fill: Fill) -> CompletionValue {
    CompletionValue::Number(f64::from(fill as u8))
}

/// Like `op_async`, but registers with the loop only once there is something to deliver, so a
/// pending wait does not keep it alive. The loop and this task share one thread, so the loop
/// cannot decide to exit between `begin_op` and `send`.
fn recv_unref(ctx: &mut dyn CallContext, inbox: Inbox) -> Result<Value, HostError> {
    let sender = ctx
        .isolate_slot::<OpContext>()
        .map(|op_ctx| op_ctx.sender.clone())
        .ok_or_else(|| HostError::TypeError("async ops need an installed OpContext".into()))?;
    let (promise, resolver) = ctx.create_promise();
    tokio::spawn(async move {
        let result = Ok(code(inbox.fill().await));
        sender.begin_op().send(Completion { resolver, result });
    });
    Ok(promise)
}

/// `op_port_take(id) -> [kind, value, portIds] | undefined`. `kind` 0 is a message (`value`
/// is its clone, `portIds` the ports that arrived with it); 1 is a worker error (`value` is
/// `{ message, filename, lineno, colno }`).
fn op_port_take(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let packet = ports(ctx)?.get(id_of(args.first())?)?.inbox.pop();
    let (kind, value, arrived) = match packet {
        None => return Ok(Value::Undefined),
        Some(Packet::Error(report)) => (1, error_object(ctx, &report)?, Vec::new()),
        Some(Packet::Ended(code)) => (2, Value::Number(f64::from(code)), Vec::new()),
        Some(Packet::Message(message)) => (0, ctx.deserialize(&message.value)?, message.ports),
    };
    let table = ports(ctx)?;
    let ids: Vec<Value> = arrived
        .into_iter()
        .map(|e| number(table.insert(e)))
        .collect();
    let ids = ctx.create_array(&ids);
    Ok(ctx.create_array(&[number(kind), value, ids]))
}

fn error_object(ctx: &mut dyn CallContext, report: &ErrorReport) -> Result<Value, HostError> {
    let object = ctx.create_object();
    let message = ctx.create_string(JsString::from(report.message.as_str()));
    let filename = ctx.create_string(JsString::from(report.filename.as_str()));
    ctx.set_property(&object, "message", message)?;
    ctx.set_property(&object, "filename", filename)?;
    ctx.set_property(&object, "lineno", Value::Number(f64::from(report.lineno)))?;
    ctx.set_property(&object, "colno", Value::Number(f64::from(report.colno)))?;
    if let Some(class) = report.quota {
        let class = ctx.create_string(JsString::from(class));
        ctx.set_property(&object, "quota", class)?;
    }
    Ok(object)
}

fn op_port_interrupt(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    ports(ctx)?.get(id_of(args.first())?)?.inbox.interrupt();
    Ok(Value::Undefined)
}

/// Closing an unknown id is a no-op: both a local `close()` and a peer's can race.
fn op_port_close(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    if let Some(endpoint) = ports(ctx)?.ends.remove(&id_of(args.first())?) {
        endpoint.inbox.interrupt();
    }
    Ok(Value::Undefined)
}

/// Same rule as the module loader: `import-local` or `fs-read` on the entry.
fn authorize_entry(ctx: &mut dyn CallContext, entry: &str) -> Result<(), HostError> {
    let op_ctx = ctx
        .isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))?;
    let permissions = &op_ctx.permissions;
    let allowed = permissions.check(&Request::ImportLocal(entry)).is_allow()
        || permissions.check(&Request::FsRead(entry)).is_allow();
    if allowed {
        return Ok(());
    }
    authorize(ctx, "op_worker_spawn", Request::ImportLocal(entry))
}

/// Relative specifiers resolve against `caller` (the module that ran `new Worker`) when it is
/// a valid module id, else against the working directory.
fn resolve_entry(specifier: &str, caller: Option<&str>) -> Result<PathBuf, VelyrError> {
    let referrer = match caller.and_then(ModuleId::parse) {
        Some(referrer) => referrer,
        None => {
            let cwd =
                std::env::current_dir().map_err(|err| VelyrError::internal(err.to_string()))?;
            ModuleId::from_path(&cwd.join("worker"))
                .ok_or_else(|| VelyrError::invalid_data("working directory is not absolute"))?
        }
    };
    let resolved = FsResolver.resolve(specifier, &referrer, &ResolveContext::default())?;
    Ok(resolved.path)
}

fn text_arg(args: &[Value], index: usize) -> Option<String> {
    match args.get(index) {
        Some(Value::String(text)) => Some(text.to_string_lossy()),
        _ => None,
    }
}

/// The optional heap limit argument, in bytes: a positive integer a `usize` can hold.
fn heap_arg(args: &[Value], index: usize) -> Result<Option<usize>, HostError> {
    match args.get(index) {
        None | Some(Value::Undefined) => Ok(None),
        Some(Value::Number(bytes)) if bytes.fract() == 0.0 && (1.0..=1e15).contains(bytes) => {
            Ok(Some(*bytes as usize))
        }
        _ => Err(HostError::TypeError(
            "the worker heap limit must be a positive integer of bytes".into(),
        )),
    }
}

/// `op_worker_spawn(url, name, permissionsJson?, callerUrl?, heapLimitBytes?) -> [workerId, portId]`
fn op_worker_spawn(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (Some(url), Some(name)) = (text_arg(args, 0), text_arg(args, 1)) else {
        return Err(HostError::TypeError(
            "op_worker_spawn expects (url, name, permissions?)".into(),
        ));
    };
    let host = ctx
        .isolate_slot::<Host>()
        .and_then(|host| host.0.clone())
        .ok_or_else(|| VelyrError::unsupported("workers are not available here"))?;
    let heap_limit_bytes = heap_arg(args, 4)?;
    let entry = resolve_entry(&url, text_arg(args, 3).as_deref())?;
    authorize_entry(ctx, &entry.to_string_lossy())?;
    let group = ctx.isolate_slot::<Group>().map_or(0, |group| group.0);
    let op_ctx = ctx
        .isolate_slot::<OpContext>()
        .ok_or_else(|| HostError::TypeError("ops need an installed OpContext".into()))?;
    let parent = ParentContext {
        capabilities: op_ctx.permissions.clone(),
        deployment_policy: host.deployment_policy.clone(),
        quotas: op_ctx.quotas.clone(),
        heap_limit_bytes: host.heap_limit_bytes,
    };
    let request = SpawnRequest {
        entry,
        name,
        permissions_json: text_arg(args, 2),
        group,
        heap_limit_bytes,
    };
    let handle = (host.spawn)(&parent, request)?;
    let port_id = ports(ctx)?.insert(handle.endpoint());
    let workers = ctx
        .isolate_slot::<Workers>()
        .ok_or_else(|| HostError::TypeError("the web messaging API is not installed".into()))?;
    workers.next += 1;
    workers.map.insert(workers.next, handle);
    let worker_id = workers.next;
    Ok(ctx.create_array(&[number(worker_id), number(port_id)]))
}

/// `op_worker_terminate(workerId)`: stops the worker and waits (bounded) for its thread.
fn op_worker_terminate(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let id = id_of(args.first())?;
    let worker = ctx
        .isolate_slot::<Workers>()
        .and_then(|workers| workers.map.remove(&id));
    if let Some(worker) = worker {
        let _ = worker.terminate(TERMINATE_WAIT);
    }
    Ok(Value::Undefined)
}

/// Worker side: `op_worker_scope_port() -> id` of the endpoint that talks to the parent.
fn op_worker_scope_port(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let endpoint = port(ctx)?.endpoint();
    Ok(number(ports(ctx)?.insert(endpoint)))
}

/// Worker side: `self.close()`. The current task finishes, then the event loop stops.
fn op_worker_close(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let port = port(ctx)?;
    request_exit(&port.exit, 0);
    port.shutdown.cancel();
    Ok(Value::Undefined)
}

/// Worker side: forwards `reportError` to the parent as an `error` event.
fn op_worker_report_error(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let message = text_arg(args, 0).unwrap_or_default();
    let stack = text_arg(args, 1);
    // The parent may be gone already; a full queue just drops the report.
    let _ = port(ctx)?
        .outbox
        .send(Packet::Error(ErrorReport::new(message, stack.as_deref())));
    Ok(Value::Undefined)
}
