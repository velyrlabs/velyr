//! Per-class resource quotas with atomic counters. A `QuotaGuard` releases its reservation on drop.

use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use velyr_runtime_errors::VelyrError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaClass {
    Files,
    Sockets,
    Listeners,
    Workers,
    Timers,
    BodyBytes,
    /// Requests being handled by `Velyr.serve`. The connection itself is a `Sockets` unit.
    Requests,
    /// Live zlib/brotli contexts (each holds native compressor state).
    Compression,
    /// Live child processes spawned by `Velyr.Command` / `node:child_process`.
    Processes,
}

const CLASS_COUNT: usize = 9;

impl QuotaClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Sockets => "sockets",
            Self::Listeners => "listeners",
            Self::Workers => "workers",
            Self::Timers => "timers",
            Self::BodyBytes => "body_bytes",
            Self::Requests => "requests",
            Self::Compression => "compression",
            Self::Processes => "processes",
        }
    }
}

/// Optional per-class limits (the `quotas` key of `velyr.json`); unset fields use the defaults.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaConfig {
    pub files: Option<u64>,
    pub sockets: Option<u64>,
    pub listeners: Option<u64>,
    pub workers: Option<u64>,
    pub timers: Option<u64>,
    pub body_bytes: Option<u64>,
    /// Defaults to the `sockets` limit: one request per connection at a time.
    pub requests: Option<u64>,
    pub compression: Option<u64>,
    pub processes: Option<u64>,
}

pub struct QuotaContext {
    limits: [u64; CLASS_COUNT],
    used: [AtomicU64; CLASS_COUNT],
    /// A worker's context: `Processes` units are also reserved here, so the limit covers the whole
    /// process instead of multiplying by the number of isolates.
    process_pool: Option<Arc<QuotaContext>>,
}

impl Default for QuotaContext {
    fn default() -> Self {
        Self::new(QuotaConfig::default())
    }
}

impl QuotaContext {
    pub fn new(config: QuotaConfig) -> Self {
        let sockets = config.sockets.unwrap_or(16384);
        Self {
            limits: [
                config.files.unwrap_or(4096),
                sockets,
                config.listeners.unwrap_or(1024),
                config.workers.unwrap_or(64),
                config.timers.unwrap_or(1_000_000),
                config.body_bytes.unwrap_or(1 << 30),
                config.requests.unwrap_or(sockets),
                config.compression.unwrap_or(1024),
                config.processes.unwrap_or(64),
            ],
            used: Default::default(),
            process_pool: None,
        }
    }

    /// A context with its own limits whose `Processes` reservations also count against `parent`
    /// (and its limit). `used` stays per context, so summing isolates does not double count.
    pub fn child_of(config: QuotaConfig, parent: &Arc<Self>) -> Self {
        Self {
            process_pool: Some(parent.clone()),
            ..Self::new(config)
        }
    }

    pub fn limit(&self, class: QuotaClass) -> u64 {
        self.limits[class as usize]
    }

    pub fn used(&self, class: QuotaClass) -> u64 {
        self.used[class as usize].load(Ordering::Acquire)
    }

    /// Atomically reserves `amount` units; never exceeds the limit, even under contention.
    pub fn reserve(
        self: &Arc<Self>,
        class: QuotaClass,
        amount: u64,
    ) -> Result<QuotaGuard, VelyrError> {
        let pooled = match (&self.process_pool, class) {
            (Some(parent), QuotaClass::Processes) => Some(Box::new(parent.reserve(class, amount)?)),
            _ => None,
        };
        let limit = self.limit(class);
        let reserved =
            self.used[class as usize].try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(amount).filter(|total| *total <= limit)
            });
        if reserved.is_err() {
            tracing::warn!(
                target: "velyr.quota.denied",
                class = class.name(),
                limit,
                "velyr.quota.denied"
            );
            return Err(VelyrError::quota_exceeded(class.name()));
        }
        Ok(QuotaGuard {
            context: self.clone(),
            class,
            amount,
            _pooled: pooled,
        })
    }
}

#[must_use = "dropping the guard releases the reservation"]
pub struct QuotaGuard {
    context: Arc<QuotaContext>,
    class: QuotaClass,
    amount: u64,
    /// The reservation in the parent's pool, released together with this one.
    _pooled: Option<Box<QuotaGuard>>,
}

impl Drop for QuotaGuard {
    fn drop(&mut self) {
        self.context.used[self.class as usize].fetch_sub(self.amount, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use velyr_runtime_errors::ErrorCode;

    fn ctx(files: u64) -> Arc<QuotaContext> {
        Arc::new(QuotaContext::new(QuotaConfig {
            files: Some(files),
            ..Default::default()
        }))
    }

    #[test]
    fn dropped_guard_releases_quota() {
        let quota = ctx(2);
        let a = quota.reserve(QuotaClass::Files, 2).unwrap();
        let err = quota.reserve(QuotaClass::Files, 1).err().unwrap();
        assert_eq!(err.code(), ErrorCode::QuotaExceeded);
        drop(a);
        assert_eq!(quota.used(QuotaClass::Files), 0);
        assert!(quota.reserve(QuotaClass::Files, 1).is_ok());
    }

    #[test]
    fn child_contexts_share_the_process_quota_but_nothing_else() {
        let parent = Arc::new(QuotaContext::new(QuotaConfig {
            processes: Some(3),
            files: Some(1),
            ..Default::default()
        }));
        let child = || Arc::new(QuotaContext::child_of(QuotaConfig::default(), &parent));
        let (a, b) = (child(), child());
        let held = a.reserve(QuotaClass::Processes, 2).unwrap();
        let err = b.reserve(QuotaClass::Processes, 2).err().unwrap();
        assert_eq!(err.code(), ErrorCode::QuotaExceeded);
        let _one = b.reserve(QuotaClass::Processes, 1).unwrap();
        assert_eq!(parent.used(QuotaClass::Processes), 3);
        // Each child still reports only its own units (metrics sum them per isolate).
        assert_eq!(a.used(QuotaClass::Processes), 2);
        assert_eq!(b.used(QuotaClass::Processes), 1);
        drop(held);
        assert_eq!(parent.used(QuotaClass::Processes), 1);
        assert_eq!(a.used(QuotaClass::Processes), 0);
        // Other classes keep their own limits.
        assert!(a.reserve(QuotaClass::Files, 2).is_ok());
    }

    #[test]
    fn classes_are_independent_and_overflow_is_denied() {
        let quota = ctx(1);
        let _f = quota.reserve(QuotaClass::Files, 1).unwrap();
        let _s = quota.reserve(QuotaClass::Sockets, 1).unwrap();
        assert!(quota.reserve(QuotaClass::BodyBytes, u64::MAX).is_err());
        let big = quota.reserve(QuotaClass::BodyBytes, 1 << 30).unwrap();
        assert!(quota.reserve(QuotaClass::BodyBytes, u64::MAX).is_err());
        drop(big);
    }

    #[test]
    fn config_defaults_and_deny_unknown_fields() {
        let quota = QuotaContext::default();
        assert_eq!(quota.limit(QuotaClass::Files), 4096);
        assert_eq!(quota.limit(QuotaClass::Sockets), 16384);
        assert_eq!(quota.limit(QuotaClass::Listeners), 1024);
        assert_eq!(quota.limit(QuotaClass::Workers), 64);
        assert_eq!(quota.limit(QuotaClass::Timers), 1_000_000);
        assert_eq!(quota.limit(QuotaClass::BodyBytes), 1 << 30);
        assert_eq!(quota.limit(QuotaClass::Requests), 16384);
        assert_eq!(quota.limit(QuotaClass::Compression), 1024);
        assert_eq!(QuotaClass::Compression.name(), "compression");
        let cfg: QuotaConfig = serde_json::from_str(r#"{"compression": 5}"#).unwrap();
        assert_eq!(QuotaContext::new(cfg).limit(QuotaClass::Compression), 5);
        assert_eq!(quota.limit(QuotaClass::Processes), 64);
        assert_eq!(QuotaClass::Processes.name(), "processes");
        let cfg: QuotaConfig = serde_json::from_str(r#"{"processes": 2}"#).unwrap();
        assert_eq!(QuotaContext::new(cfg).limit(QuotaClass::Processes), 2);
        let cfg: QuotaConfig = serde_json::from_str(r#"{"sockets": 3}"#).unwrap();
        assert_eq!(QuotaContext::new(cfg).limit(QuotaClass::Requests), 3);
        let cfg: QuotaConfig = serde_json::from_str(r#"{"sockets": 3, "requests": 9}"#).unwrap();
        assert_eq!(QuotaContext::new(cfg).limit(QuotaClass::Requests), 9);
        let cfg: QuotaConfig = serde_json::from_str(r#"{"files": 7}"#).unwrap();
        assert_eq!(QuotaContext::new(cfg).limit(QuotaClass::Files), 7);
        assert!(serde_json::from_str::<QuotaConfig>(r#"{"bogus": 1}"#).is_err());
    }

    #[test]
    fn concurrent_reservations_never_exceed_limit() {
        let quota = ctx(10);
        let granted: Vec<_> = (0..16)
            .map(|_| {
                let quota = quota.clone();
                std::thread::spawn(move || {
                    (0..100)
                        .filter_map(|_| quota.reserve(QuotaClass::Files, 1).ok())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let guards: Vec<_> = granted
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect();
        assert_eq!(guards.len(), 10);
        assert_eq!(quota.used(QuotaClass::Files), 10);
        drop(guards);
        assert_eq!(quota.used(QuotaClass::Files), 0);
    }
}
