//! WHATWG Streams (`ReadableStream`, `WritableStream`, `TransformStream`, queuing strategies, text and
//! compression streams). Pure JS written from the Streams Standard; only gzip/deflate go through Rust ops.

mod flate;
mod install;

pub use flate::flate_host_functions;
pub use install::{STREAMS_JS, install_web_streams};
