//! Placeholder slot: filled by a later Temporal task.

use temporal_rs::TemporalError;

use super::{Args, Res};

pub(super) fn run(_method: &str, _a: Args) -> Res {
    Err(TemporalError::general("not implemented"))
}
