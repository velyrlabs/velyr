use std::time::{Instant, SystemTime, UNIX_EPOCH};

use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, Isolate, JsException, JsString, Source, Value,
};

/// Bootstrap script: defines `performance`, the `Performance*` classes, `navigator` and `Navigator`
/// on top of the private `op_perf_*` and `op_navigator_*` bindings.
pub const PERFORMANCE_JS: &str = include_str!("performance.js");

/// Clock resolution exposed to scripts (100us), coarse enough to blunt timing attacks.
const RESOLUTION_MICROS: u128 = 100;

/// Monotonic origin and wall-clock time captured when the bootstrap is installed.
struct PerfClock {
    origin: Instant,
    time_origin_ms: f64,
}

pub fn performance_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_perf_now",
            arity: 0,
            callback: op_perf_now,
        },
        HostFunctionDef {
            name: "op_perf_time_origin",
            arity: 0,
            callback: op_perf_time_origin,
        },
        HostFunctionDef {
            name: "op_navigator_user_agent",
            arity: 0,
            callback: op_navigator_user_agent,
        },
        HostFunctionDef {
            name: "op_navigator_hardware_concurrency",
            arity: 0,
            callback: op_navigator_hardware_concurrency,
        },
    ]
}

/// Captures the clock origin and runs [`PERFORMANCE_JS`]. Call from the `install` hook of
/// `run_file`, after the `performance_host_functions` are registered.
pub fn install_performance(isolate: &mut impl Isolate) -> Result<(), JsException> {
    isolate.set_slot(PerfClock {
        origin: Instant::now(),
        time_origin_ms: round_to_resolution(wall_clock_micros()),
    });
    isolate
        .eval_script(&Source {
            name: "velyr:performance".into(),
            text: PERFORMANCE_JS.into(),
        })
        .map(drop)
}

fn wall_clock_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_micros())
}

/// Floors `micros` to the exposed resolution and converts to fractional milliseconds.
fn round_to_resolution(micros: u128) -> f64 {
    (micros / RESOLUTION_MICROS) as f64 * RESOLUTION_MICROS as f64 / 1000.0
}

fn clock(ctx: &mut dyn CallContext) -> Result<&mut PerfClock, HostError> {
    ctx.isolate_slot::<PerfClock>()
        .ok_or_else(|| HostError::TypeError("performance is not installed on this isolate".into()))
}

/// `op_perf_now() -> ms`: monotonic time since the origin, floored to 0.1 ms.
fn op_perf_now(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let elapsed = clock(ctx)?.origin.elapsed();
    Ok(Value::Number(round_to_resolution(elapsed.as_micros())))
}

/// `op_perf_time_origin() -> ms`: Unix epoch milliseconds of the origin.
fn op_perf_time_origin(ctx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    Ok(Value::Number(clock(ctx)?.time_origin_ms))
}

/// `op_navigator_user_agent() -> "Velyr/<version>"`
fn op_navigator_user_agent(_: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    let agent = concat!("Velyr/", env!("CARGO_PKG_VERSION"));
    Ok(Value::String(JsString::from_str(agent)))
}

/// `op_navigator_hardware_concurrency() -> n`: logical cores, 1 when unknown.
fn op_navigator_hardware_concurrency(
    _: &mut dyn CallContext,
    _: &[Value],
) -> Result<Value, HostError> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    Ok(Value::Number(cores as f64))
}
