//! `op_telemetry_*`: the ops behind `Velyr.telemetry.span`. Without the `telemetry` feature, or
//! before the host started the exporter, `start` answers `null` and the script just runs `fn`.

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_ops::CapabilityKind;

use crate::ops::{string_arg, type_error};

pub(crate) const OPS: [(HostFunctionDef, CapabilityKind); 2] = [
    op!(op_telemetry_start, 2, None),
    op!(op_telemetry_end, 2, None),
];

/// Most spans a script can hold open at once; past it `start` answers `null`.
#[cfg(feature = "telemetry")]
const MAX_OPEN_SPANS: usize = 10_000;

/// Spans scripts can name by id: the ones they opened, and the server span of each request being
/// handled. Process-wide because a request is dropped on a network thread, not the isolate's.
#[cfg(feature = "telemetry")]
static OPEN: std::sync::LazyLock<std::sync::Mutex<(u64, std::collections::HashMap<u64, velyr_runtime_telemetry::ActiveSpan>)>> =
    std::sync::LazyLock::new(Default::default);

/// Makes `span` addressable; `None` when too many are open.
#[cfg(feature = "telemetry")]
pub(crate) fn register(span: velyr_runtime_telemetry::ActiveSpan) -> Option<u64> {
    let mut open = OPEN.lock().expect("open spans");
    if open.1.len() >= MAX_OPEN_SPANS {
        return None;
    }
    open.0 += 1;
    let id = open.0;
    open.1.insert(id, span);
    Some(id)
}

#[cfg(feature = "telemetry")]
pub(crate) fn lookup(id: u64) -> Option<velyr_runtime_telemetry::ActiveSpan> {
    OPEN.lock().expect("open spans").1.get(&id).cloned()
}

#[cfg(feature = "telemetry")]
pub(crate) fn release(id: u64) -> Option<velyr_runtime_telemetry::ActiveSpan> {
    OPEN.lock().expect("open spans").1.remove(&id)
}

/// `op_telemetry_start(name, parentId | null) -> id | null`.
fn op_telemetry_start(_ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let name = string_arg(args, 0, "op_telemetry_start")?;
    #[cfg(feature = "telemetry")]
    {
        let parent = match args.get(1) {
            Some(Value::Number(id)) => Some(*id as u64),
            _ => None,
        };
        let id = velyr_runtime_telemetry::user_span(&name, parent.and_then(lookup).as_ref())
            .and_then(register);
        return Ok(id.map_or(Value::Null, |id| Value::Number(id as f64)));
    }
    #[cfg(not(feature = "telemetry"))]
    {
        let _ = name;
        Ok(Value::Null)
    }
}

/// `op_telemetry_end(id, errorMessage | null)`: ends the span, as failed when a message is given.
fn op_telemetry_end(_ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::Number(id)) = args.first() else {
        return Err(type_error("op_telemetry_end", 0, "a span id"));
    };
    let failure = match args.get(1) {
        Some(Value::String(message)) => Some(message.to_string_lossy()),
        _ => None,
    };
    #[cfg(feature = "telemetry")]
    if let Some(span) = release(*id as u64) {
        match failure {
            Some(message) => span.fail(&message),
            None => span.end(),
        }
    }
    #[cfg(not(feature = "telemetry"))]
    let _ = (id, failure);
    Ok(Value::Undefined)
}
