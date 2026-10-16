//! Pluggable resolution strategies consulted for specifiers the core resolver cannot handle.

mod npm;

pub use npm::{BareSpecifierResolver, StrategyResolver};
