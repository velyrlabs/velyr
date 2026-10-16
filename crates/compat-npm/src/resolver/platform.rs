//! Node-style platform identifiers used to filter `os` / `cpu` constraints.

/// Platform names as in `process.platform` / `process.arch` (`darwin`, `linux`, `win32`; `arm64`, `x64`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: String,
    pub cpu: String,
}

impl Platform {
    pub fn new(os: impl Into<String>, cpu: impl Into<String>) -> Self {
        Self {
            os: os.into(),
            cpu: cpu.into(),
        }
    }

    pub fn current() -> Self {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        };
        let cpu = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            "x86" => "ia32",
            other => other,
        };
        Self::new(os, cpu)
    }

    pub(super) fn accepts(&self, os: &[String], cpu: &[String]) -> bool {
        allows(os, &self.os) && allows(cpu, &self.cpu)
    }
}

/// npm semantics: empty list allows all; `!x` blocks `x`; any positive entry makes the list an allowlist.
fn allows(list: &[String], value: &str) -> bool {
    if list.iter().any(|e| e.strip_prefix('!') == Some(value)) {
        return false;
    }
    let mut positives = list.iter().filter(|e| !e.starts_with('!')).peekable();
    positives.peek().is_none() || positives.any(|e| e == value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn allowlist_and_blocklist() {
        assert!(allows(&[], "linux"));
        assert!(allows(&v(&["darwin", "linux"]), "linux"));
        assert!(!allows(&v(&["darwin"]), "linux"));
        assert!(!allows(&v(&["!linux"]), "linux"));
        assert!(allows(&v(&["!win32"]), "linux"));
    }
}
