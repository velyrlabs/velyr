//! Embed the Velyr JavaScript runtime in a Rust host.
//!
//! ```no_run
//! use velyr::{Capability, PathPattern, Permissions, Runtime};
//!
//! let permissions = Permissions::new().allow(Capability::ImportLocal(PathPattern::new("/srv/app")));
//! let mut runtime = Runtime::builder().permissions(permissions).build()?;
//! runtime.run_module("/srv/app/main.mjs")?;
//! let total = runtime.call("sum", &[1.into(), 2.into()])?;
//! # Ok::<(), velyr::Error>(())
//! ```
//!
//! # Model
//!
//! * Default deny (ADR-003): a runtime built without permissions cannot even import its entry
//!   module. Grant `ImportLocal` for the module tree.
//! * Host ops follow ADR-010: a [`HostFunctionDef`] named `op_<area>_<verb>`, installed as a
//!   global. The callback starts with [`authorize`] when the op needs a capability, exactly like
//!   the built-in ops, so the runtime's [`Permissions`] apply to it.
//! * Every [`Runtime::run_module`] and [`Runtime::call`] runs in a fresh V8 isolate on its own
//!   thread. State does not survive between calls and the module's top level runs again for each
//!   call. A persistent isolate needs a session API in `velyr-runtime-core`.
//!
//! # Stability
//!
//! This crate follows semver. Everything exported here is the stable surface; enums and option
//! structs are `#[non_exhaustive]` so variants and fields can be added in a minor release. The
//! re-exported engine and permission types come from `0.x` workspace crates and track their
//! changes until those reach 1.0.

mod call;
mod error;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value as Json;
use velyr_engine_api::Isolate;
use velyr_engine_v8::V8Engine;
use velyr_runtime_core::{LoopIsolate, RuntimeConfig, run_module_file};
use velyr_runtime_event_loop::{CompletionSender, TimerHandle};
use velyr_runtime_modules::ModuleId;
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context, take_leak_report};
use velyr_runtime_permissions::CapabilitySet;
use velyr_web_core as web;

use call::{RETURN_DEF, ReturnSlot, wrapper_source};

pub use error::Error;
pub use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
pub use velyr_runtime_ops::{CapabilityKind, OpContext, authorize};
pub use velyr_runtime_permissions::{
    AddrPattern, Capability, HostPattern, NamePattern, PathPattern, PortRange, ProgramPattern,
    Request,
};
pub use velyr_runtime_resources::QuotaConfig;

/// Isolate id of an embedded run (one isolate per run).
const ISOLATE_ID: u64 = 0;

static SCRATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Authority granted to the JS code: allow and deny rules, deny winning. Empty denies everything.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Permissions {
    allow: Vec<Capability>,
    deny: Vec<Capability>,
}

impl Permissions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn allow(mut self, capability: Capability) -> Self {
        self.allow.push(capability);
        self
    }

    pub fn deny(mut self, capability: Capability) -> Self {
        self.deny.push(capability);
        self
    }

    fn into_set(self, scratch: &Path) -> Arc<CapabilitySet> {
        let scratch = PathPattern::new(scratch.to_string_lossy());
        let mut builder = CapabilitySet::builder().allow(Capability::ImportLocal(scratch));
        for capability in self.allow {
            builder = builder.allow(capability);
        }
        for capability in self.deny {
            builder = builder.deny(capability);
        }
        builder.build()
    }
}

/// Which globals a fresh isolate gets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Profile {
    /// `console`, `Velyr.env` and `Velyr.permissions` only.
    Core,
    /// Core plus timers, `URL`, events, text encoding and `performance`.
    #[default]
    Minimal,
}

impl Profile {
    fn host_functions(self) -> Vec<HostFunctionDef> {
        match self {
            Self::Core => Vec::new(),
            Self::Minimal => [
                web::timer_host_functions(),
                web::url_host_functions(),
                web::performance_host_functions(),
                web::encoding_host_functions(),
            ]
            .concat(),
        }
    }

    fn install<I: Isolate>(self, isolate: &mut LoopIsolate<I>, timers: TimerHandle) {
        if self == Self::Core {
            return;
        }
        web::install_timers(isolate, timers).expect("timers bootstrap is valid JS");
        web::install_url(isolate).expect("url bootstrap is valid JS");
        web::install_performance(isolate).expect("performance bootstrap is valid JS");
        web::install_events(isolate).expect("events bootstrap is valid JS");
        web::install_encoding(isolate).expect("encoding bootstrap is valid JS");
    }
}

/// Configures a [`Runtime`].
#[derive(Default)]
pub struct RuntimeBuilder {
    permissions: Permissions,
    quotas: QuotaConfig,
    profile: Profile,
    heap_limit_bytes: Option<usize>,
    cpu_budget: Option<Duration>,
    ops: Vec<(HostFunctionDef, CapabilityKind)>,
}

impl RuntimeBuilder {
    pub fn permissions(mut self, permissions: Permissions) -> Self {
        self.permissions = permissions;
        self
    }

    /// Per-class resource limits (files, sockets, timers, ...).
    pub fn quotas(mut self, quotas: QuotaConfig) -> Self {
        self.quotas = quotas;
        self
    }

    pub fn profile(mut self, profile: Profile) -> Self {
        self.profile = profile;
        self
    }

    /// Hard JS heap limit; hitting it fails the run.
    pub fn heap_limit_bytes(mut self, bytes: usize) -> Self {
        self.heap_limit_bytes = Some(bytes);
        self
    }

    /// Wall-clock budget of one synchronous execution.
    pub fn cpu_budget(mut self, budget: Duration) -> Self {
        self.cpu_budget = Some(budget);
        self
    }

    /// Registers a host op; `required` is the capability it declares (the callback must still
    /// call [`authorize`] itself). Names are validated by [`RuntimeBuilder::build`].
    pub fn op(mut self, def: HostFunctionDef, required: CapabilityKind) -> Self {
        self.ops.push((def, required));
        self
    }

    pub fn build(self) -> Result<Runtime, Error> {
        let mut registry = OpRegistry::builtin();
        registry.register(RETURN_DEF).map_err(Error::Op)?;
        for (def, required) in self.ops {
            registry.register_with(def, required).map_err(Error::Op)?;
        }
        let mut host_functions = registry.host_functions().to_vec();
        host_functions.extend(self.profile.host_functions());

        let id = SCRATCH_COUNTER.fetch_add(1, Ordering::Relaxed);
        let scratch = std::env::temp_dir().join(format!("velyr-embed-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&scratch)?;
        let scratch = scratch.canonicalize()?;
        Ok(Runtime {
            capabilities: self.permissions.into_set(&scratch),
            scratch,
            quotas: self.quotas,
            profile: self.profile,
            heap_limit_bytes: self.heap_limit_bytes,
            cpu_budget: self.cpu_budget,
            host_functions,
            module: None,
            calls: AtomicU64::new(0),
        })
    }
}

/// An embedded Velyr runtime. See the crate docs for the execution model.
pub struct Runtime {
    capabilities: Arc<CapabilitySet>,
    quotas: QuotaConfig,
    profile: Profile,
    heap_limit_bytes: Option<usize>,
    cpu_budget: Option<Duration>,
    host_functions: Vec<HostFunctionDef>,
    scratch: PathBuf,
    module: Option<ModuleId>,
    calls: AtomicU64,
}

impl Runtime {
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::default()
    }

    /// Runs the ES module at `module` (a path or a `file://` URL) to completion, event loop
    /// included. On success it becomes the module [`Runtime::call`] uses.
    pub fn run_module(&mut self, module: &str) -> Result<(), Error> {
        let id = module_id(module)?;
        let path = id
            .to_path()
            .ok_or_else(|| Error::InvalidModule(module.into()))?;
        self.execute(path, None)?;
        self.module = Some(id);
        Ok(())
    }

    /// Calls the exported function `export` of the module loaded by [`Runtime::run_module`] with
    /// JSON `args`, awaits its result and returns it as JSON (`undefined` becomes `null`).
    pub fn call(&self, export: &str, args: &[Json]) -> Result<Json, Error> {
        let module = self.module.as_ref().ok_or(Error::NoModule)?;
        let n = self.calls.fetch_add(1, Ordering::Relaxed);
        let entry = self.scratch.join(format!("call-{n}.mjs"));
        std::fs::write(&entry, wrapper_source(module.as_str(), export, args))?;
        let slot = ReturnSlot::default();
        let outcome = self.execute(entry.clone(), Some(slot.clone()));
        std::fs::remove_file(&entry).ok();
        outcome?;
        let text = slot.0.lock().expect("return slot is never poisoned").take();
        let text = text.ok_or(Error::NoResult)?;
        serde_json::from_str(&text).map_err(|_| Error::NoResult)
    }

    fn execute(&self, entry: PathBuf, slot: Option<ReturnSlot>) -> Result<(), Error> {
        let config = RuntimeConfig {
            platform_init: Some(V8Engine::init_platform as fn()),
            capabilities: Some(self.capabilities.clone()),
            quotas: Some(self.quotas.clone()),
            heap_limit_bytes: self.heap_limit_bytes,
            cpu_budget_ms: self
                .cpu_budget
                .map(|d| d.as_millis().try_into().unwrap_or(u64::MAX)),
            leak_collector: Some(take_leak_report),
            ..RuntimeConfig::default()
        };
        let (capabilities, quotas, profile) =
            (self.capabilities.clone(), self.quotas.clone(), self.profile);
        let install = move |isolate: &mut LoopIsolate<_>, sender: CompletionSender, timers| {
            install_op_context(isolate, ISOLATE_ID, sender, capabilities, quotas);
            profile.install(isolate, timers);
            if let Some(slot) = slot {
                isolate.set_slot(slot);
            }
        };
        run_module_file(
            V8Engine::new(),
            config,
            self.host_functions.clone(),
            BOOTSTRAP_JS,
            install,
            entry,
        )
        .map_err(Error::Run)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.scratch).ok();
    }
}

fn module_id(module: &str) -> Result<ModuleId, Error> {
    let invalid = || Error::InvalidModule(module.into());
    if module.starts_with("file:") {
        return ModuleId::parse(module).ok_or_else(invalid);
    }
    let absolute = std::path::absolute(module).map_err(|_| invalid())?;
    ModuleId::from_path(&absolute).ok_or_else(invalid)
}
