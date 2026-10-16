//! Tracing subscriber setup. This is the only place a subscriber is installed.
//!
//! Filtering comes from the `VELYR_LOG` env var (`EnvFilter` syntax, default `warn`).
//! Output goes to stderr so stdout stays reserved for program output.
//!
//! Span naming convention: `velyr.<crate>.<op>`, for example `velyr.runtime.eval`
//! or `velyr.cli.run`. Env values and secrets must never be recorded as fields.

use clap::ValueEnum;
use tracing::Level;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};
use velyr_runtime_core::metrics::{self, MetricsLayer};

const LOG_ENV_VAR: &str = "VELYR_LOG";
const DEFAULT_FILTER: &str = "warn";

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
}

pub fn init(format: LogFormat) {
    let filter =
        EnvFilter::try_from_env(LOG_ENV_VAR).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let fmt = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let fmt = match format {
        LogFormat::Pretty => fmt.pretty().with_filter(filter).boxed(),
        LogFormat::Json => fmt.json().with_filter(filter).boxed(),
    };
    // Independent of VELYR_LOG: denial counters must not depend on what gets printed.
    let denials = Targets::new()
        .with_target("velyr.permission.denied", Level::WARN)
        .with_target("velyr.quota.denied", Level::WARN);
    tracing_subscriber::registry()
        .with(fmt)
        .with(MetricsLayer::new(metrics::global().clone()).with_filter(denials))
        .init();
}
