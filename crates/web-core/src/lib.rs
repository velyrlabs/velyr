//! Web-platform globals that do not need the network (timers, `URL`, text encoding, `structuredClone`). They live here so every
//! profile, `minimal` included, can use them without pulling in other web crates (ADR-011).

mod blob;
mod encoding;
mod events;
mod microtask;
mod performance;
mod structured_clone;
mod timers;
mod url;

pub use blob::{BLOB_JS, blob_host_functions, install_blob, install_blob_slots};
pub use encoding::{
    ENCODING_JS, encoding_host_functions, install_encoding, install_encoding_slots,
};
pub use events::{ABORT_JS, DOM_EXCEPTION_JS, EVENT_JS, install_events};
pub use performance::{
    PERFORMANCE_JS, install_performance, install_performance_slots, performance_host_functions,
};
pub use structured_clone::{
    STRUCTURED_CLONE_JS, install_structured_clone, structured_clone_host_functions,
};
pub use timers::{TIMERS_JS, install_timer_slots, install_timers, timer_host_functions};
pub use url::{URL_JS, install_url, url_host_functions};
