//! `--workers=auto` (ADR-014): `min(cpus, floor(memory_budget / per_worker_estimate))`, at
//! least 1. CPUs and memory honor cgroup v1/v2 limits; the cgroup root and `/proc/meminfo` are
//! injectable so the arithmetic is tested with fake files.

use std::path::PathBuf;

/// Memory one worker is assumed to need. ADR-016 (VELYR-61, macOS M4 Pro, `velyr serve` under load):
/// about 5 MiB idle per extra worker, 26 to 70 MiB under plaintext load and 85 to 130 MiB under a
/// streamed-body load (garbage that V8 has not collected yet). 128 MiB covers the streaming case;
/// a heap limit per isolate (`heap_limit_bytes`) is what bounds it for real.
pub const PER_WORKER_ESTIMATE_BYTES: u64 = 128 << 20;

/// Share of the total RAM used as the budget when neither `quotas.memoryMb` nor a cgroup says more.
const RAM_SHARE_PERCENT: u64 = 75;

/// Where the host's limits live.
#[derive(Debug, Clone)]
pub struct Environment {
    /// Directory of the cgroup the process runs in (`/sys/fs/cgroup`).
    pub cgroup_root: PathBuf,
    pub meminfo: PathBuf,
    /// `std::thread::available_parallelism`, before the cgroup limits are applied by hand.
    pub host_cpus: usize,
}

impl Environment {
    pub fn system() -> Self {
        Self {
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            meminfo: PathBuf::from("/proc/meminfo"),
            host_cpus: std::thread::available_parallelism().map_or(1, usize::from),
        }
    }
}

/// Where the memory budget came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetSource {
    Config,
    Cgroup,
    Ram,
    Unlimited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub workers: usize,
    pub cpus: usize,
    pub memory_budget: Option<u64>,
    pub source: BudgetSource,
    /// Old-generation limit for each worker isolate; `None` without a budget.
    pub heap_limit_bytes: Option<usize>,
}

/// Resolves `auto` and logs the arithmetic under `velyr.workers.auto`.
pub fn workers(env: &Environment, memory_mb: Option<u64>) -> usize {
    let decision = decide(env, memory_mb);
    tracing::info!(
        target: "velyr.workers.auto",
        workers = decision.workers,
        cpus = decision.cpus,
        memory_budget = decision.memory_budget,
        source = ?decision.source,
        per_worker = PER_WORKER_ESTIMATE_BYTES,
        "velyr.workers.auto"
    );
    decision.workers
}

pub fn decide(env: &Environment, memory_mb: Option<u64>) -> Decision {
    let cpus = cgroup_cpus(env).map_or(env.host_cpus, |limit| limit.min(env.host_cpus));
    let cpus = cpus.max(1);
    let (memory_budget, source) = memory_budget(env, memory_mb);
    let by_memory = memory_budget.map(|bytes| bytes / PER_WORKER_ESTIMATE_BYTES);
    let workers = by_memory.map_or(cpus, |fit| {
        cpus.min(usize::try_from(fit).unwrap_or(usize::MAX))
    });
    let workers = workers.max(1);
    Decision {
        workers,
        cpus,
        memory_budget,
        source,
        heap_limit_bytes: memory_budget.map(|budget| heap_limit_per_worker(budget, workers)),
    }
}

const HEAP_LIMIT_FLOOR_BYTES: usize = 64 << 20;
const HEAP_LIMIT_CEILING_BYTES: usize = 4 << 30;

/// `clamp(budget / workers, 64 MiB, 4 GiB)`: the heap limit of one worker isolate.
pub fn heap_limit_per_worker(budget: u64, workers: usize) -> usize {
    let share = budget / workers.max(1) as u64;
    usize::try_from(share)
        .unwrap_or(usize::MAX)
        .clamp(HEAP_LIMIT_FLOOR_BYTES, HEAP_LIMIT_CEILING_BYTES)
}

/// `quotas.memoryMb`, else the cgroup limit, else 75% of the RAM, else no budget.
fn memory_budget(env: &Environment, memory_mb: Option<u64>) -> (Option<u64>, BudgetSource) {
    if let Some(mb) = memory_mb {
        return (Some(mb.saturating_mul(1 << 20)), BudgetSource::Config);
    }
    if let Some(limit) = cgroup_memory(env) {
        return (Some(limit), BudgetSource::Cgroup);
    }
    match total_ram(env) {
        Some(total) => (
            Some(total.saturating_mul(RAM_SHARE_PERCENT) / 100),
            BudgetSource::Ram,
        ),
        None => (None, BudgetSource::Unlimited),
    }
}

fn read_number(path: PathBuf) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// v2 `cpu.max` (`<quota> <period>` or `max <period>`), else v1 `cpu.cfs_quota_us` (`-1` is none).
/// A fractional quota rounds up, like std does.
fn cgroup_cpus(env: &Environment) -> Option<usize> {
    let root = &env.cgroup_root;
    let (quota, period) = match std::fs::read_to_string(root.join("cpu.max")) {
        Ok(text) => {
            let mut parts = text.split_whitespace();
            (
                parts.next()?.parse::<u64>().ok()?,
                parts.next()?.parse::<u64>().ok()?,
            )
        }
        Err(_) => {
            let v1 = ["cpu", "cpu,cpuacct"]
                .iter()
                .map(|dir| root.join(dir))
                .find(|dir| dir.join("cpu.cfs_quota_us").exists())?;
            (
                read_number(v1.join("cpu.cfs_quota_us"))?,
                read_number(v1.join("cpu.cfs_period_us"))?,
            )
        }
    };
    (period > 0).then(|| usize::try_from(quota.div_ceil(period)).unwrap_or(usize::MAX))
}

/// A v1 limit this large means "unlimited" (the kernel reports a page-rounded `i64::MAX`).
const UNLIMITED_V1: u64 = 1 << 60;

fn cgroup_memory(env: &Environment) -> Option<u64> {
    let root = &env.cgroup_root;
    read_number(root.join("memory.max")).or_else(|| {
        ["memory", "memory,cpuacct"]
            .iter()
            .find_map(|dir| read_number(root.join(dir).join("memory.limit_in_bytes")))
            .filter(|bytes| *bytes < UNLIMITED_V1)
    })
}

/// `MemTotal` of `/proc/meminfo`; macOS has no such file and asks `sysctl hw.memsize`.
// shortcut: the macOS path spawns `sysctl` (no libc without unsafe); upgrade with a safe wrapper.
fn total_ram(env: &Environment) -> Option<u64> {
    if let Ok(text) = std::fs::read_to_string(&env.meminfo) {
        let kb: u64 = text
            .lines()
            .find_map(|line| line.strip_prefix("MemTotal:"))?
            .split_whitespace()
            .next()?
            .parse()
            .ok()?;
        return Some(kb * 1024);
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        return String::from_utf8_lossy(&out.stdout).trim().parse().ok();
    }
    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const MIB: u64 = 1 << 20;
    /// MiB per worker, so the arithmetic below follows the constant.
    const PER: u64 = PER_WORKER_ESTIMATE_BYTES / MIB;

    fn env(dir: &tempfile::TempDir, host_cpus: usize) -> Environment {
        Environment {
            cgroup_root: dir.path().to_path_buf(),
            meminfo: dir.path().join("meminfo"),
            host_cpus,
        }
    }

    fn write(dir: &tempfile::TempDir, name: &str, text: &str) {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn cgroup_v2_cpu_max_caps_the_cpus() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir, "cpu.max", "200000 100000\n");
        assert_eq!(decide(&env(&dir, 8), None).workers, 2);
        write(&dir, "cpu.max", "150000 100000\n");
        assert_eq!(decide(&env(&dir, 8), None).cpus, 2, "fractions round up");
        write(&dir, "cpu.max", "max 100000\n");
        assert_eq!(decide(&env(&dir, 8), None).cpus, 8);
    }

    #[test]
    fn cgroup_v1_cfs_quota_caps_the_cpus() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir, "cpu/cpu.cfs_quota_us", "300000\n");
        write(&dir, "cpu/cpu.cfs_period_us", "100000\n");
        assert_eq!(decide(&env(&dir, 8), None).cpus, 3);
        write(&dir, "cpu/cpu.cfs_quota_us", "-1\n");
        assert_eq!(decide(&env(&dir, 8), None).cpus, 8);
    }

    #[test]
    fn memory_budget_divides_by_the_per_worker_estimate() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir, "memory.max", &format!("{}\n", 2 * PER * MIB));
        let decision = decide(&env(&dir, 8), None);
        assert_eq!(
            (decision.workers, decision.source),
            (2, BudgetSource::Cgroup)
        );
        write(
            &dir,
            "memory/memory.limit_in_bytes",
            &format!("{}\n", 3 * PER * MIB),
        );
        fs::remove_file(dir.path().join("memory.max")).unwrap();
        assert_eq!(decide(&env(&dir, 8), None).workers, 3);
    }

    #[test]
    fn config_memory_wins_over_the_cgroup_and_ram() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir, "memory.max", &format!("{}\n", 128 * MIB));
        write(&dir, "meminfo", "MemTotal:       65536 kB\n");
        let decision = decide(&env(&dir, 16), Some(8 * PER));
        assert_eq!(
            (decision.workers, decision.source),
            (8, BudgetSource::Config)
        );
    }

    #[test]
    fn without_cgroup_memory_it_uses_three_quarters_of_the_ram() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir, "memory.max", "max\n");
        write(
            &dir,
            "meminfo",
            &format!("MemTotal:      {} kB\nMemFree: 1 kB\n", 4 * PER * 1024),
        );
        let decision = decide(&env(&dir, 16), None);
        assert_eq!(decision.memory_budget, Some(3 * PER * MIB));
        assert_eq!((decision.workers, decision.source), (3, BudgetSource::Ram));
    }

    #[test]
    fn limit_splits_budget_across_workers() {
        assert_eq!(heap_limit_per_worker(4 << 30, 8), 512 << 20);
        let dir = tempfile::tempdir().unwrap();
        write(&dir, "memory.max", &format!("{}\n", 4u64 << 30));
        let decision = decide(&env(&dir, 8), None);
        assert_eq!(decision.heap_limit_bytes, Some(512 << 20));
    }

    #[test]
    fn limit_never_below_floor() {
        assert_eq!(heap_limit_per_worker(100 << 20, 8), HEAP_LIMIT_FLOOR_BYTES);
        assert_eq!(heap_limit_per_worker(1 << 40, 1), HEAP_LIMIT_CEILING_BYTES);
        assert_eq!(heap_limit_per_worker(1 << 30, 0), 1 << 30);
    }

    #[test]
    fn the_estimate_is_the_one_adr_016_measured() {
        assert_eq!(PER_WORKER_ESTIMATE_BYTES, 128 << 20);
    }

    #[test]
    fn there_is_always_at_least_one_worker() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir, "memory.max", &format!("{}\n", 10 * MIB));
        assert_eq!(decide(&env(&dir, 8), None).workers, 1);
    }

    #[test]
    fn no_limits_at_all_means_every_cpu() {
        let dir = tempfile::tempdir().unwrap();
        let decision = decide(&env(&dir, 6), None);
        assert_eq!(decision.workers, 6);
    }
}
