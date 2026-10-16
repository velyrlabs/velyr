//! `velyr bench <file>`: runs the file with `Velyr.bench(name, fn)` installed. Each bench is
//! warmed up, timed `--samples` times and reported as mean, p50, p99 and ops/s, as a table or
//! (`--json`) one JSON document on stdout.
//!
//! The runner is a JS prelude evaluated before the entry module. It starts on a timer, so every
//! top-level `Velyr.bench` call of the module graph has registered by then. `Velyr.bench` does
//! not exist outside this command.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::permissions::Permissions;
use crate::run::EngineKind;

#[derive(clap::Args)]
pub struct BenchArgs {
    /// Benchmark file; it calls `Velyr.bench(name, fn)`.
    file: PathBuf,

    /// Print one JSON document instead of the table.
    #[arg(long)]
    json: bool,

    /// Calls made and thrown away before sampling.
    #[arg(long, default_value_t = 10, value_name = "N")]
    warmup: u32,

    /// Timed calls per bench.
    #[arg(long, default_value_t = 100, value_name = "N")]
    samples: u32,

    /// Run only the benches whose name contains TEXT.
    #[arg(long, value_name = "TEXT")]
    filter: Option<String>,

    /// JS engine to run on.
    #[arg(long, value_enum, default_value_t)]
    engine: EngineKind,

    #[command(flatten)]
    permissions: Permissions,
}

const RUNNER_JS: &str = r#"
(() => {
  const config = __CONFIG__;
  const MIN_BATCH_NS = 1e6;
  const MAX_BATCH = 1 << 20;
  const benches = [];
  let scheduled = false;
  const pick = (sorted, q) => sorted[Math.min(sorted.length - 1, Math.ceil(q * sorted.length) - 1)];
  const batch = async (fn, calls) => {
    const start = performance.now();
    for (let i = 0; i < calls; i++) await fn();
    return (performance.now() - start) * 1e6;
  };
  const measure = async (fn) => {
    await batch(fn, config.warmup);
    // The clock is coarse: time batches long enough to read (>= 1 ms) and divide.
    let calls = 1;
    while (calls < MAX_BATCH && (await batch(fn, calls)) < MIN_BATCH_NS) calls *= 2;
    const times = [];
    for (let i = 0; i < config.samples; i++) times.push((await batch(fn, calls)) / calls);
    const sorted = times.slice().sort((a, b) => a - b);
    const meanNs = times.reduce((sum, t) => sum + t, 0) / times.length;
    return {
      samples: times.length,
      meanNs,
      p50Ns: pick(sorted, 0.5),
      p99Ns: pick(sorted, 0.99),
      opsPerSec: 1e9 / meanNs,
    };
  };
  const row = (r) =>
    `${r.name.padEnd(32)} ${r.meanNs.toFixed(0).padStart(12)} ns ${r.p50Ns.toFixed(0).padStart(12)} ns ${r.p99Ns.toFixed(0).padStart(12)} ns ${r.opsPerSec.toFixed(0).padStart(12)} ops/s`;
  const run = async () => {
    const results = [];
    let failure;
    for (const { name, fn } of benches) {
      if (!name.includes(config.filter)) continue;
      try {
        results.push({ name, ...(await measure(fn)) });
      } catch (error) {
        failure = error;
        console.error(`bench "${name}" failed: ${error && error.message ? error.message : error}`);
      }
    }
    if (config.json) console.log(JSON.stringify({ benches: results }));
    else {
      console.log(`${"name".padEnd(32)} ${"mean".padStart(15)} ${"p50".padStart(15)} ${"p99".padStart(15)} ${"ops/s".padStart(19)}`);
      for (const r of results) console.log(row(r));
    }
    if (failure !== undefined) throw new Error("one or more benches failed");
  };
  Object.defineProperty(globalThis.Velyr, "bench", {
    value: (name, fn) => {
      if (typeof name !== "string" || typeof fn !== "function") {
        throw new TypeError("Velyr.bench(name, fn) expects a string and a function");
      }
      benches.push({ name, fn });
      if (!scheduled) {
        scheduled = true;
        setTimeout(run, 0);
      }
    },
    configurable: true,
    enumerable: true,
  });
})();
"#;

fn runner_script(args: &BenchArgs) -> String {
    let config = serde_json::json!({
        "json": args.json,
        "warmup": args.warmup,
        "samples": args.samples.max(1),
        "filter": args.filter.as_deref().unwrap_or(""),
    });
    RUNNER_JS.replace("__CONFIG__", &config.to_string())
}

pub fn bench(args: BenchArgs) -> ExitCode {
    #[cfg(not(feature = "web"))]
    {
        let _ = args;
        eprintln!("error: `velyr bench` is not available in this profile");
        ExitCode::FAILURE
    }
    #[cfg(feature = "web")]
    {
        let script = runner_script(&args);
        crate::run::run_with(&args.file, args.permissions, Some(script), args.engine)
    }
}
