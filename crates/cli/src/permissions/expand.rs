//! Flag/config values to `Capability` values, one permission kind at a time.

use std::net::IpAddr;
use std::path::Path;

use velyr_runtime_permissions::{
    AddrPattern, Capability, HostPattern, NamePattern, PathPattern, PortRange, ProgramPattern,
};

use super::ConfigError;

const ALL_PORTS: PortRange = PortRange {
    start: 0,
    end: u16::MAX,
};
const ANY_ADDRS: [&str; 2] = ["0.0.0.0/0", "::/0"];
/// API names that `--allow-sys` / `--deny-sys` can scope.
const SYS_NAMES: [&str; 3] = ["networkInterfaces", "userInfo", "loadavg"];
const DEFAULT_INSPECT_ADDR: &str = "127.0.0.1";

#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Read,
    Write,
    Net,
    Listen,
    Env,
    Sys,
    Run,
    Native,
    Import,
    Inspect,
}

/// `--allow-all` only: a bare `*` never reaches IP literals in non-public ranges, so loopback is granted explicitly.
pub fn loopback_net_grants() -> Vec<Capability> {
    ["127.0.0.0/8", "::1/128"]
        .map(|cidr| Capability::NetConnect(HostPattern::new(cidr), ALL_PORTS))
        .to_vec()
}

impl Kind {
    /// Kinds granted by `--allow-all`.
    pub const ALLOW_ALL: [Kind; 8] = [
        Kind::Read,
        Kind::Write,
        Kind::Net,
        Kind::Listen,
        Kind::Env,
        Kind::Sys,
        Kind::Run,
        Kind::Import,
    ];

    fn name(self) -> &'static str {
        match self {
            Kind::Read => "read",
            Kind::Write => "write",
            Kind::Net => "net",
            Kind::Listen => "listen",
            Kind::Env => "env",
            Kind::Sys => "sys",
            Kind::Run => "run",
            Kind::Native => "native",
            Kind::Import => "import",
            Kind::Inspect => "inspect",
        }
    }

    fn flag(self, deny: bool) -> String {
        match (deny, self) {
            (false, Kind::Inspect) => "inspect".to_owned(),
            (false, _) => format!("allow-{}", self.name()),
            (true, _) => format!("deny-{}", self.name()),
        }
    }

    /// An empty `scope` means the whole kind.
    pub fn capabilities(
        self,
        scope: &[String],
        cwd: &Path,
        deny: bool,
    ) -> Result<Vec<Capability>, ConfigError> {
        if scope.is_empty() {
            return Ok(self.unrestricted(cwd, deny));
        }
        let mut caps = Vec::new();
        for value in scope {
            let scoped = self
                .scoped(value, cwd)
                .map_err(|reason| ConfigError::InvalidValue {
                    flag: self.flag(deny),
                    value: value.clone(),
                    reason,
                })?;
            caps.extend(scoped);
        }
        Ok(caps)
    }

    fn unrestricted(self, cwd: &Path, deny: bool) -> Vec<Capability> {
        let root = || path_pattern(cwd, filesystem_root(cwd));
        let addrs = || ANY_ADDRS.map(AddrPattern::new);
        match self {
            Kind::Read => vec![Capability::FsRead(root())],
            Kind::Write => vec![Capability::FsWrite(root())],
            Kind::Native => vec![Capability::NativeLoad(root())],
            Kind::Net => vec![Capability::NetConnect(HostPattern::new("*"), ALL_PORTS)],
            Kind::Listen => addrs()
                .map(|a| Capability::NetListen(a, ALL_PORTS))
                .to_vec(),
            Kind::Env => env_capabilities("*"),
            Kind::Sys => vec![Capability::SysInfo(NamePattern::new("*"))],
            Kind::Run => vec![Capability::ProcessSpawn(ProgramPattern::new("*"))],
            Kind::Import => vec![Capability::ImportRemote(HostPattern::new("*"))],
            Kind::Inspect if deny => addrs().map(Capability::InspectorBind).to_vec(),
            Kind::Inspect => vec![Capability::InspectorBind(AddrPattern::new(
                DEFAULT_INSPECT_ADDR,
            ))],
        }
    }

    fn scoped(self, value: &str, cwd: &Path) -> Result<Vec<Capability>, &'static str> {
        if value.is_empty() {
            return Err("empty value");
        }
        Ok(match self {
            Kind::Read => vec![Capability::FsRead(path_pattern(cwd, value))],
            Kind::Write => vec![Capability::FsWrite(path_pattern(cwd, value))],
            Kind::Native => vec![Capability::NativeLoad(path_pattern(cwd, value))],
            Kind::Env => env_capabilities(value),
            Kind::Sys if SYS_NAMES.contains(&value) => {
                vec![Capability::SysInfo(NamePattern::new(value))]
            }
            Kind::Sys => return Err("unknown system information name"),
            Kind::Run => vec![Capability::ProcessSpawn(ProgramPattern::new(value))],
            Kind::Import => vec![Capability::ImportRemote(HostPattern::new(value))],
            Kind::Net => {
                let (host, ports) = host_and_ports(value)?;
                vec![Capability::NetConnect(HostPattern::new(host), ports)]
            }
            Kind::Listen => {
                let (addr, ports) = host_and_ports(value)?;
                vec![Capability::NetListen(AddrPattern::new(addr), ports)]
            }
            // The capability carries no port; the port is validated and dropped.
            Kind::Inspect => {
                let (addr, _) = host_and_ports(value)?;
                vec![Capability::InspectorBind(AddrPattern::new(addr))]
            }
        })
    }
}

/// Relative values resolve against `cwd`; canonicalization happens when the set is built.
pub fn path_pattern(cwd: &Path, value: impl AsRef<Path>) -> PathPattern {
    PathPattern::new(cwd.join(value).to_string_lossy())
}

fn filesystem_root(cwd: &Path) -> std::path::PathBuf {
    cwd.ancestors().last().unwrap_or(cwd).to_path_buf()
}

fn env_capabilities(pattern: &str) -> Vec<Capability> {
    vec![
        Capability::EnvRead(NamePattern::new(pattern)),
        Capability::EnvWrite(NamePattern::new(pattern)),
    ]
}

/// `host`, `host:port`, `[v6]`, `[v6]:port` or a bare IP literal; no port means every port.
fn host_and_ports(value: &str) -> Result<(String, PortRange), &'static str> {
    if value.parse::<IpAddr>().is_ok() {
        return Ok((value.to_owned(), ALL_PORTS));
    }
    let (host, tail) = match value.strip_prefix('[') {
        Some(rest) => {
            let end = rest.find(']').ok_or("unclosed '['")?;
            (&value[..end + 2], &rest[end + 1..])
        }
        None => value.find(':').map_or((value, ""), |i| value.split_at(i)),
    };
    if host.is_empty() {
        return Err("empty host");
    }
    let ports = match tail.strip_prefix(':') {
        None if tail.is_empty() => ALL_PORTS,
        None => return Err("expected ':port' after the host"),
        Some(port) => {
            let port: u16 = port.parse().map_err(|_| "invalid port")?;
            PortRange {
                start: port,
                end: port,
            }
        }
    };
    Ok((host.to_owned(), ports))
}
