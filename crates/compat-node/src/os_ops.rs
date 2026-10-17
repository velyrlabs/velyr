//! Host op behind `node:os`: `op_node_os(name)` answers from the OS (sysinfo and std), never from
//! `process.env`, so it needs no env permission. Strings come back as text, memory and uptime as
//! numbers; multi-field answers are tab/newline separated and split in `os.js`.

use std::io::IsTerminal;

use sysinfo::{
    CpuRefreshKind, MemoryRefreshKind, Networks, Pid, ProcessRefreshKind, ProcessesToUpdate,
    RefreshKind, System, Users,
};
use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_runtime_ops::authorize;
use velyr_runtime_permissions::Request;

pub(crate) fn os_host_functions() -> Vec<HostFunctionDef> {
    vec![HostFunctionDef {
        name: "op_node_os",
        arity: 1,
        callback: op_os,
    }]
}

fn text(value: impl Into<String>) -> Value {
    Value::String(value.into().as_str().into())
}

fn memory() -> System {
    System::new_with_specifics(
        RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
    )
}

/// Per-core `[user, nice, sys, idle, irq]` in milliseconds since boot, from `/proc/stat`
/// (assumes USER_HZ = 100, true on every mainstream Linux build).
#[cfg(target_os = "linux")]
fn cpu_times(count: usize) -> Vec<[u64; 5]> {
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let mut times: Vec<[u64; 5]> = stat
        .lines()
        .filter(|line| {
            line.starts_with("cpu") && line[3..].starts_with(|c: char| c.is_ascii_digit())
        })
        .map(|line| {
            let f: Vec<u64> = line
                .split_whitespace()
                .skip(1)
                .map(|v| v.parse().unwrap_or(0) * 10)
                .collect();
            let at = |i: usize| f.get(i).copied().unwrap_or(0);
            [at(0), at(1), at(2), at(3), at(5)]
        })
        .collect();
    times.resize(count, [0; 5]);
    times
}

/// Other platforms: zeros, without blocking.
// shortcut: sysinfo only reports a usage percentage that needs two samples ~200 ms apart, and the
// per-state tick counters sit behind an unsafe Mach call; lift this when an unsafe-free source of
// per-core ticks is available (or the unsafe allowlist admits host_processor_info).
#[cfg(not(target_os = "linux"))]
fn cpu_times(count: usize) -> Vec<[u64; 5]> {
    vec![[0; 5]; count]
}

fn cpus() -> String {
    let system = System::new_with_specifics(
        RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_frequency()),
    );
    let times = cpu_times(system.cpus().len());
    let lines: Vec<String> = system
        .cpus()
        .iter()
        .zip(times)
        .map(|(cpu, t)| {
            format!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                cpu.brand(),
                cpu.frequency(),
                t[0],
                t[1],
                t[2],
                t[3],
                t[4]
            )
        })
        .collect();
    lines.join("\n")
}

/// Accumulated CPU time of this process in milliseconds (all threads).
fn cpu_time() -> f64 {
    let Ok(pid) = sysinfo::get_current_pid() else {
        return 0.0;
    };
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cpu(),
    );
    system
        .process(pid)
        .map_or(0.0, |p| p.accumulated_cpu_time() as f64)
}

/// `username\tuid\tgid` of the process owner, empty when the OS does not say.
fn user() -> String {
    let Ok(pid) = sysinfo::get_current_pid() else {
        return String::new();
    };
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from(pid.as_u32() as usize)]),
        true,
        ProcessRefreshKind::nothing().with_user(sysinfo::UpdateKind::Always),
    );
    let Some(uid) = system.process(pid).and_then(|process| process.user_id()) else {
        return String::new();
    };
    let users = Users::new_with_refreshed_list();
    match users.get_user_by_id(uid) {
        Some(user) => format!("{}\t{}\t{}", user.name(), **uid, *user.group_id()),
        None => String::new(),
    }
}

/// `name\tmac\taddress\tprefix` per address; the netmask and the rest are derived in `os.js`.
fn network_interfaces() -> String {
    let networks = Networks::new_with_refreshed_list();
    let mut lines = Vec::new();
    for (name, data) in &networks {
        for ip in data.ip_networks() {
            lines.push(format!(
                "{name}\t{}\t{}\t{}",
                data.mac_address(),
                ip.addr,
                ip.prefix
            ));
        }
    }
    lines.join("\n")
}

/// The `SysInfo` name guarding an os query, `None` for the queries that stay free.
fn sys_info_name(query: &str) -> Option<&'static str> {
    match query {
        "netifs" => Some("networkInterfaces"),
        "user" => Some("userInfo"),
        "loadavg" => Some("loadavg"),
        _ => None,
    }
}

fn op_os(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let Some(Value::String(name)) = args.first() else {
        return Err(HostError::TypeError("op_node_os expects (name)".into()));
    };
    let name = name.to_string_lossy();
    if let Some(api) = sys_info_name(&name) {
        authorize(ctx, "op_node_os", Request::SysInfo(api))?;
    }
    Ok(match name.as_str() {
        "netifs" => text(network_interfaces()),
        "hostname" => text(System::host_name().unwrap_or_default()),
        "totalmem" => Value::Number(memory().total_memory() as f64),
        "freemem" => {
            // On macOS "available" subtracts compressor pages and saturates to 0 under heavy
            // compression, so never report less than the plain free pages.
            let system = memory();
            Value::Number(system.available_memory().max(system.free_memory()) as f64)
        }
        "cpus" => text(cpus()),
        "cpucount" => {
            Value::Number(std::thread::available_parallelism().map_or(1, |n| n.get()) as f64)
        }
        "cputime" => Value::Number(cpu_time()),
        "loadavg" => {
            let load = System::load_average();
            text(format!("{}\t{}\t{}", load.one, load.five, load.fifteen))
        }
        "uptime" => Value::Number(System::uptime() as f64),
        "release" => text(System::kernel_version().unwrap_or_default()),
        "version" => text(System::long_os_version().unwrap_or_default()),
        "user" => text(user()),
        "homedir" => text(
            std::env::home_dir()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        "tmpdir" => text(std::env::temp_dir().to_string_lossy().into_owned()),
        // `node:tty`: only the standard fds can be asked without `unsafe` (no `from_raw_fd`).
        "isatty:0" => Value::Bool(std::io::stdin().is_terminal()),
        "isatty:1" => Value::Bool(std::io::stdout().is_terminal()),
        "isatty:2" => Value::Bool(std::io::stderr().is_terminal()),
        other => return Err(HostError::TypeError(format!("unknown os query {other}"))),
    })
}
