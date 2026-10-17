//! `$262.evalScript`, `$262.gc` and the parse-only phase check for `negative.phase: parse`.

use velyr_engine_api::{CallContext, HostError, Isolate, Source, Value};

use crate::frontmatter::Meta;

/// Engine gap marker: native reports constructs it cannot compile as `SyntaxError: unsupported: ...`.
const UNSUPPORTED_PREFIX: &str = "SyntaxError: unsupported: ";

pub fn eval_script_op(cx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let text = match args.first() {
        Some(Value::String(text)) => text.to_string_lossy(),
        _ => String::new(),
    };
    cx.eval_script(&Source {
        name: "evalScript".to_owned(),
        text,
    })
}

pub fn gc_op(cx: &mut dyn CallContext, _: &[Value]) -> Result<Value, HostError> {
    cx.request_gc().map(|()| Value::Undefined)
}

/// What the parse-only check decided for a `negative.phase: parse` script.
pub enum ParsePhase {
    /// Not a parse-phase negative test: run it normally.
    NotApplicable,
    /// The engine cannot parse without running.
    Unavailable,
    /// Final verdict reached without running any code.
    Done(Result<(), String>),
}

pub fn check_parse_phase<I: Isolate>(isolate: &mut I, meta: &Meta, source: &Source) -> ParsePhase {
    let Some(negative) = meta.negative.as_ref().filter(|n| n.phase == "parse") else {
        return ParsePhase::NotApplicable;
    };
    match isolate.check_script(source) {
        Err(e) if e.is_unsupported() => ParsePhase::Unavailable,
        Err(e) => ParsePhase::Done(judge_negative(&negative.error_type, &e.message)),
        Ok(()) => ParsePhase::Done(Err("expected parse error, got successful parse".to_owned())),
    }
}

/// An error matches when it starts with the expected type; engine gaps never match.
pub fn judge_negative(error_type: &str, message: &str) -> Result<(), String> {
    if message.starts_with(error_type) && !message.starts_with(UNSUPPORTED_PREFIX) {
        Ok(())
    } else {
        Err(format!("expected {error_type}, got {message}"))
    }
}
