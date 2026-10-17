use velyr_engine_api::{Isolate, JsException, Source};

use crate::flate::FlateTable;

/// Bootstrap script. The fragments share one private scope: `00_head.js` opens it and
/// `70_exports.js` closes it, so they are only valid concatenated in this order.
pub const STREAMS_JS: &str = concat!(
    include_str!("js/00_head.js"),
    include_str!("js/10_readable.js"),
    include_str!("js/15_readable_classes.js"),
    include_str!("js/20_byte.js"),
    include_str!("js/30_tee_pipe.js"),
    include_str!("js/40_writable.js"),
    include_str!("js/45_writable_classes.js"),
    include_str!("js/50_transform.js"),
    include_str!("js/60_text_compression.js"),
    include_str!("js/70_exports.js"),
);

/// Registers the codec table and runs [`STREAMS_JS`]. Call from the `install` hook of `run_file`,
/// after `flate_host_functions` are registered and after timers, events and encoding are installed
/// (it uses `AbortController`, `TextEncoder` and `TextDecoder`).
pub fn install_web_streams(isolate: &mut impl Isolate) -> Result<(), JsException> {
    install_web_streams_slots(isolate);
    isolate
        .eval_script(&Source {
            name: "velyr:streams".into(),
            text: STREAMS_JS.into(),
        })
        .map(drop)
}

/// The Rust half of [`install_web_streams`], for an isolate whose [`STREAMS_JS`] already ran.
pub fn install_web_streams_slots(isolate: &mut impl Isolate) {
    isolate.set_slot(FlateTable::default());
}
