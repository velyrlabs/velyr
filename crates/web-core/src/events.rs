use velyr_engine_api::{Isolate, JsException, Source};

/// Bootstrap script: defines `DOMException`.
pub const DOM_EXCEPTION_JS: &str = include_str!("dom_exception.js");
/// Bootstrap script: defines `Event`, `CustomEvent` and `EventTarget`.
pub const EVENT_JS: &str = include_str!("event.js");
/// Bootstrap script: defines `AbortController` and `AbortSignal`.
pub const ABORT_JS: &str = include_str!("abort.js");

/// Runs the three bootstraps in dependency order. These are pure JS (no host functions).
/// Call from the `install` hook of `run_file`, after `install_timers` so `AbortSignal.timeout`
/// finds `setTimeout`; install it before encoding and `structuredClone` so they throw `DOMException`.
pub fn install_events(isolate: &mut impl Isolate) -> Result<(), JsException> {
    for (name, text) in [
        ("velyr:dom-exception", DOM_EXCEPTION_JS),
        ("velyr:event", EVENT_JS),
        ("velyr:abort", ABORT_JS),
    ] {
        isolate.eval_script(&Source {
            name: name.into(),
            text: text.into(),
        })?;
    }
    Ok(())
}
