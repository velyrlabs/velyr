//! `velyr.json` (JSONC): discovery, parsing and the typed schema (ADR-013).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use velyr_runtime_resources::QuotaConfig;
use velyr_runtime_workers::auto;

use crate::permissions::{ConfigError, Permissions};

const FILE_NAME: &str = "velyr.json";

// Parsed and validated now; the consumers (workers, module loader, transforms) land later.
#[allow(dead_code)]
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub permissions: Option<Permissions>,
    pub quotas: Option<Quotas>,
    pub workers: Option<Workers>,
    /// Import map: specifier to target.
    pub imports: Option<BTreeMap<String, String>>,
    #[serde(rename = "compilerOptions")]
    pub compiler_options: Option<CompilerOptions>,
    /// Packages whose install scripts `velyr install` may run.
    #[serde(rename = "allowScripts")]
    pub allow_scripts: Option<Vec<String>>,
    /// OTLP trace export; read at startup by `telemetry::init`.
    #[allow(dead_code)] // the field is only read with the `telemetry` feature
    pub telemetry: Option<crate::telemetry::TelemetryConfig>,
}

/// `quotas` of `velyr.json`: the runtime quotas plus `memoryMb`, the budget `--workers=auto` uses.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "serde_json::Value")]
pub struct Quotas {
    #[cfg_attr(not(feature = "web"), allow(dead_code))] // read by `velyr serve`
    pub memory_mb: Option<u64>,
    pub resources: QuotaConfig,
}

// Not `#[serde(flatten)]`: that would drop `deny_unknown_fields` of `QuotaConfig`.
impl TryFrom<serde_json::Value> for Quotas {
    type Error = String;

    fn try_from(mut value: serde_json::Value) -> Result<Self, String> {
        let memory_mb = match value.as_object_mut().and_then(|map| map.remove("memoryMb")) {
            Some(mb) => Some(
                mb.as_u64()
                    .ok_or("quotas.memoryMb must be a positive integer")?,
            ),
            None => None,
        };
        let resources = QuotaConfig::deserialize(value).map_err(|e| e.to_string())?;
        Ok(Self {
            memory_mb,
            resources,
        })
    }
}

/// `workers` of `velyr.json` and `velyr serve --workers`: a count or `auto`.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum Workers {
    Count(u32),
    Auto(AutoKeyword),
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoKeyword {
    Auto,
}

impl Workers {
    /// How many server workers this asks for; at least 1. `auto` is capped by the CPUs and by
    /// the memory budget (`memory_mb` is `quotas.memoryMb`).
    #[cfg_attr(not(feature = "web"), allow(dead_code))] // read by `velyr serve`
    pub fn count(self, memory_mb: Option<u64>) -> Result<usize, String> {
        match self {
            Self::Count(0) => Err("workers must be at least 1".into()),
            Self::Count(count) => Ok(count as usize),
            Self::Auto(_) => Ok(auto::workers(&auto::Environment::system(), memory_mb)),
        }
    }
}

impl std::str::FromStr for Workers {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let workers = if text == "auto" {
            Self::Auto(AutoKeyword::Auto)
        } else {
            Self::Count(
                text.parse()
                    .map_err(|_| "expected a positive number or `auto`".to_owned())?,
            )
        };
        if matches!(workers, Self::Count(0)) {
            return Err("workers must be at least 1".into());
        }
        Ok(workers)
    }
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CompilerOptions {
    pub jsx: Option<String>,
    pub jsx_import_source: Option<String>,
}

/// A parsed config plus the directory it was found in (the project root).
#[derive(Debug)]
pub struct Loaded {
    pub dir: PathBuf,
    pub config: Config,
}

/// Walks up from `start` and loads the first `velyr.json` found.
pub fn discover(start: &Path) -> Result<Option<Loaded>, ConfigError> {
    let Some(dir) = start.ancestors().find(|d| d.join(FILE_NAME).is_file()) else {
        return Ok(None);
    };
    let path = dir.join(FILE_NAME);
    let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Read {
        path: path.clone(),
        source,
    })?;
    let config = parse(&text).map_err(|message| ConfigError::Parse { path, message })?;
    Ok(Some(Loaded {
        dir: dir.to_path_buf(),
        config,
    }))
}

fn parse(text: &str) -> Result<Config, String> {
    let parsed = jsonc_parser::parse_to_serde_value::<Option<Config>>(text, &Default::default());
    parsed
        .map(Option::unwrap_or_default)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_trailing_commas_are_accepted() {
        let config = parse("{ // c\n \"quotas\": { \"timers\": 3, }, /* b */ }").unwrap();
        assert_eq!(config.quotas.unwrap().resources.timers, Some(3));
    }

    #[test]
    fn allow_scripts_is_a_list_of_names() {
        let config = parse("{\"allowScripts\": [\"esbuild\", \"sharp\"]}").unwrap();
        assert_eq!(config.allow_scripts.unwrap(), ["esbuild", "sharp"]);
    }

    #[test]
    fn unknown_key_is_named() {
        for text in [
            "{\"nope\": 1}",
            "{\"quotas\": {\"nope\": 1}}",
            "{\"compilerOptions\": {\"nope\": 1}}",
            "{\"permissions\": {\"allow-nope\": true}}",
        ] {
            assert!(parse(text).unwrap_err().contains("nope"), "{text}");
        }
    }

    #[test]
    fn quotas_accept_memory_mb() {
        let config = parse("{\"quotas\": {\"memoryMb\": 256, \"timers\": 3}}").unwrap();
        let quotas = config.quotas.unwrap();
        assert_eq!(
            (quotas.memory_mb, quotas.resources.timers),
            (Some(256), Some(3))
        );
    }

    #[test]
    fn empty_file_is_an_empty_config() {
        assert!(parse("// nothing\n").unwrap().permissions.is_none());
    }

    #[test]
    fn workers_accepts_count_or_auto_only() {
        assert!(matches!(
            parse("{\"workers\": 4}").unwrap().workers,
            Some(Workers::Count(4))
        ));
        assert!(matches!(
            parse("{\"workers\": \"auto\"}").unwrap().workers,
            Some(Workers::Auto(_))
        ));
        assert!(parse("{\"workers\": \"many\"}").is_err());
    }

    #[test]
    fn discover_walks_up_to_the_nearest_file() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "{}").unwrap();
        let loaded = discover(&nested).unwrap().unwrap();
        assert_eq!(loaded.dir, dir.path());
    }
}
