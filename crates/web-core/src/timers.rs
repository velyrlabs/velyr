use std::time::Duration;

use velyr_engine_api::{
    CallContext, HostError, HostFunctionDef, Isolate, JsException, Source, Value,
};
use velyr_runtime_event_loop::{TimerHandle, TimerId};

/// Bootstrap script: defines `setTimeout`, `setInterval`, `clear*` and `queueMicrotask`.
pub const TIMERS_JS: &str = include_str!("timers.js");

/// Isolate slot the timer ops find the loop's [`TimerHandle`] through.
struct TimerSlot(TimerHandle);

pub fn timer_host_functions() -> Vec<HostFunctionDef> {
    let mut defs = vec![
        HostFunctionDef {
            name: "op_timer_set",
            arity: 3,
            callback: op_timer_set,
        },
        HostFunctionDef {
            name: "op_timer_clear",
            arity: 1,
            callback: op_timer_clear,
        },
        HostFunctionDef {
            name: "op_timer_ref",
            arity: 2,
            callback: op_timer_ref,
        },
    ];
    defs.extend(crate::microtask::microtask_host_functions());
    defs
}

/// Wires `handle` into `isolate`, runs [`TIMERS_JS`] and hands its dispatcher to the loop.
/// Call from the `install` hook of `run_file`, after the `timer_host_functions` are registered.
pub fn install_timers(isolate: &mut impl Isolate, handle: TimerHandle) -> Result<(), JsException> {
    install_timer_slots(isolate, handle.clone());
    let dispatcher = isolate.eval_script(&Source {
        name: "velyr:timers".into(),
        text: TIMERS_JS.into(),
    })?;
    handle.set_dispatcher(dispatcher);
    Ok(())
}

/// The Rust half of [`install_timers`], for an isolate whose [`TIMERS_JS`] already ran (startup
/// snapshot): the caller hands that script's completion value to `handle.set_dispatcher`.
pub fn install_timer_slots(isolate: &mut impl Isolate, handle: TimerHandle) {
    isolate.set_slot(TimerSlot(handle));
}

fn handle(ctx: &mut dyn CallContext) -> Result<TimerHandle, HostError> {
    ctx.isolate_slot::<TimerSlot>()
        .map(|slot| slot.0.clone())
        .ok_or_else(|| HostError::TypeError("timers are not installed on this isolate".into()))
}

fn timer_id(value: &Value) -> Result<TimerId, HostError> {
    match value {
        Value::Number(n) if n.is_finite() && *n >= 0.0 && n.fract() == 0.0 => {
            Ok(TimerId(*n as u64))
        }
        _ => Err(HostError::TypeError(
            "timer id must be a non-negative integer".into(),
        )),
    }
}

/// `op_timer_set(delayMs, repeat, level) -> id`; `level` is 0 outside a batched dispatch.
fn op_timer_set(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [
        Value::Number(delay),
        Value::Bool(repeat),
        Value::Number(level),
    ] = args
    else {
        return Err(HostError::TypeError(
            "op_timer_set expects (delay: number, repeat: boolean, level: number)".into(),
        ));
    };
    if !delay.is_finite() || *delay < 0.0 {
        return Err(HostError::TypeError(
            "timer delay must be a non-negative number".into(),
        ));
    }
    let delay = Duration::from_millis(*delay as u64);
    let level = (*level > 0.0).then_some(*level as u32);
    let id = handle(ctx)?.try_set_nested(delay, *repeat, level)?;
    Ok(Value::Number(id.0 as f64))
}

/// `op_timer_clear(id)`
fn op_timer_clear(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [id] = args else {
        return Err(HostError::TypeError(
            "op_timer_clear expects (id: number)".into(),
        ));
    };
    handle(ctx)?.clear(timer_id(id)?);
    Ok(Value::Undefined)
}

/// `op_timer_ref(id, refd)`
fn op_timer_ref(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [id, Value::Bool(refd)] = args else {
        return Err(HostError::TypeError(
            "op_timer_ref expects (id: number, refd: boolean)".into(),
        ));
    };
    handle(ctx)?.set_ref(timer_id(id)?, *refd);
    Ok(Value::Undefined)
}
