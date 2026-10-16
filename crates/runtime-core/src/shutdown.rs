//! Cross-thread termination of an isolate and mapping of its outcome.

use velyr_engine_api::JsException;

/// Message adapters give the exception produced by a terminated execution; the engine
/// contract exposes no dedicated flag.
const TERMINATED_MESSAGE: &str = "execution terminated";

pub(crate) fn is_termination(exception: &JsException) -> bool {
    exception.message == TERMINATED_MESSAGE
}
