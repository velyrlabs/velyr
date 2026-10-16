use velyr_engine_api::HostFunctionDef;
use velyr_runtime_errors::VelyrError;

use crate::{console, env, permissions_query};

/// Numeric id of a registered op: its registration index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OpId(pub u32);

/// Capability an op needs before it may have any effect; mirrors the `Request` variants.
/// `None` ops (console, timers) never call `authorize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityKind {
    None,
    FsRead,
    FsWrite,
    NetConnect,
    NetListen,
    EnvRead,
    EnvWrite,
    ProcessSpawn,
    ImportLocal,
    ImportRemote,
    NativeLoad,
    InspectorBind,
}

/// Ops known to an isolate, in id order.
#[derive(Debug, Default)]
pub struct OpRegistry {
    defs: Vec<HostFunctionDef>,
    required: Vec<CapabilityKind>,
}

impl OpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with every built-in op.
    pub fn builtin() -> Self {
        let mut registry = Self::new();
        registry
            .register(console::OP_CONSOLE_WRITE)
            .expect("built-in op names are valid and unique");
        for (def, kind) in [
            (env::OP_ENV_GET, CapabilityKind::EnvRead),
            (env::OP_ENV_HAS, CapabilityKind::EnvRead),
            (env::OP_ENV_SET, CapabilityKind::EnvWrite),
            (env::OP_ENV_ENTRIES, CapabilityKind::EnvRead),
            (
                permissions_query::OP_PERMISSIONS_QUERY,
                CapabilityKind::None,
            ),
        ] {
            registry
                .register_with(def, kind)
                .expect("built-in op names are valid and unique");
        }
        registry
    }

    /// Registers an op that needs no capability; see [`OpRegistry::register_with`].
    pub fn register(&mut self, def: HostFunctionDef) -> Result<OpId, VelyrError> {
        self.register_with(def, CapabilityKind::None)
    }

    /// Fails with `InvalidData` unless the name is `op_<area>_<verb>`, and with
    /// `AlreadyExists` on a duplicate name.
    pub fn register_with(
        &mut self,
        def: HostFunctionDef,
        required: CapabilityKind,
    ) -> Result<OpId, VelyrError> {
        if !is_op_name(def.name) {
            return Err(VelyrError::invalid_data(format!(
                "op name `{}` must match op_<area>_<verb>",
                def.name
            )));
        }
        if self.id_of(def.name).is_some() {
            return Err(VelyrError::already_exists(format!(
                "op `{}` is already registered",
                def.name
            )));
        }
        self.defs.push(def);
        self.required.push(required);
        Ok(OpId(
            u32::try_from(self.defs.len() - 1).expect("op count fits u32"),
        ))
    }

    pub fn id_of(&self, name: &str) -> Option<OpId> {
        let index = self.defs.iter().position(|def| def.name == name)?;
        Some(OpId(index as u32))
    }

    /// Every op with the capability it declares, in id order.
    pub fn ops(&self) -> Vec<(HostFunctionDef, CapabilityKind)> {
        self.defs
            .iter()
            .copied()
            .zip(self.required.iter().copied())
            .collect()
    }

    pub fn host_functions(&self) -> &[HostFunctionDef] {
        &self.defs
    }
}

fn is_op_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("op_") else {
        return false;
    };
    let mut parts = rest.split('_');
    matches!((parts.next(), parts.next()), (Some(area), Some(verb)) if !area.is_empty() && !verb.is_empty())
}
