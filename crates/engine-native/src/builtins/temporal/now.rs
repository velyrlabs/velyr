//! `Temporal.Now` ops: the system clock and the host zone `Date` already uses.

use std::time::{SystemTime, UNIX_EPOCH};

use temporal_rs::TemporalError;

use super::{Res, range};
use crate::date::host_zone;

fn epoch_nanoseconds() -> i128 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_nanos() as i128,
        Err(before) => -(before.duration().as_nanos() as i128),
    }
}

pub(super) fn run(method: &str) -> Res {
    match method {
        "epochNanoseconds" => Ok(epoch_nanoseconds().to_string()),
        "timeZoneId" => host_zone()
            .identifier()
            .map_err(|_| range("invalid host time zone")),
        _ => Err(TemporalError::general("unknown Now op")),
    }
}
