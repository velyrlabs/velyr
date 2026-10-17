//! tsconfig data model: raw JSON shape, resolved options and the `extends` merge rules.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use velyr_runtime_errors::VelyrError;

/// `paths` entries with the directory their targets are relative to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub base: PathBuf,
    pub map: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompilerOptions {
    pub experimental_decorators: Option<bool>,
    pub emit_decorator_metadata: Option<bool>,
    pub use_define_for_class_fields: Option<bool>,
    pub target: Option<String>,
    pub verbatim_module_syntax: Option<bool>,
    pub strict: Option<bool>,
    pub strict_null_checks: Option<bool>,
    pub jsx: Option<String>,
    pub jsx_import_source: Option<String>,
    pub jsx_factory: Option<String>,
    pub jsx_fragment_factory: Option<String>,
    /// Absolute, resolved against the file that defined it.
    pub base_url: Option<PathBuf>,
    pub paths: Option<Paths>,
}

impl CompilerOptions {
    /// D6: explicit flag wins, else derived from `target`; neither keeps today's behavior (true).
    pub fn define_class_fields(&self) -> bool {
        if let Some(flag) = self.use_define_for_class_fields {
            return flag;
        }
        self.target
            .as_deref()
            .is_none_or(target_defines_class_fields)
    }

    /// `strictNullChecks` falls back to `strict`, which defaults to off (tsc).
    pub fn strict_null_checks(&self) -> bool {
        self.strict_null_checks.or(self.strict).unwrap_or(false)
    }

    /// `self` wins per key; `paths` is replaced as a unit, never merged.
    pub(crate) fn over(self, base: Self) -> Self {
        Self {
            experimental_decorators: self
                .experimental_decorators
                .or(base.experimental_decorators),
            emit_decorator_metadata: self
                .emit_decorator_metadata
                .or(base.emit_decorator_metadata),
            use_define_for_class_fields: self
                .use_define_for_class_fields
                .or(base.use_define_for_class_fields),
            target: self.target.or(base.target),
            verbatim_module_syntax: self.verbatim_module_syntax.or(base.verbatim_module_syntax),
            strict: self.strict.or(base.strict),
            strict_null_checks: self.strict_null_checks.or(base.strict_null_checks),
            jsx: self.jsx.or(base.jsx),
            jsx_import_source: self.jsx_import_source.or(base.jsx_import_source),
            jsx_factory: self.jsx_factory.or(base.jsx_factory),
            jsx_fragment_factory: self.jsx_fragment_factory.or(base.jsx_fragment_factory),
            base_url: self.base_url.or(base.base_url),
            paths: self.paths.or(base.paths),
        }
    }
}

fn target_defines_class_fields(target: &str) -> bool {
    let target = target.to_ascii_lowercase();
    match target.strip_prefix("es").map(str::parse::<u32>) {
        Some(Ok(year)) if year >= 2000 => year >= 2022,
        Some(Ok(_)) => false,
        _ => true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TsConfig {
    pub path: PathBuf,
    pub options: CompilerOptions,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum Extends {
    One(String),
    Many(Vec<String>),
}

impl Extends {
    pub(crate) fn into_vec(self) -> Vec<String> {
        match self {
            Extends::One(one) => vec![one],
            Extends::Many(many) => many,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawOptions {
    experimental_decorators: Option<bool>,
    emit_decorator_metadata: Option<bool>,
    use_define_for_class_fields: Option<bool>,
    target: Option<String>,
    verbatim_module_syntax: Option<bool>,
    strict: Option<bool>,
    strict_null_checks: Option<bool>,
    jsx: Option<String>,
    jsx_import_source: Option<String>,
    jsx_factory: Option<String>,
    jsx_fragment_factory: Option<String>,
    base_url: Option<String>,
    paths: Option<BTreeMap<String, Vec<String>>>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawConfig {
    pub(crate) extends: Option<Extends>,
    compiler_options: Option<RawOptions>,
}

pub(crate) fn parse_raw(path: &Path, text: &str) -> Result<RawConfig, VelyrError> {
    let parsed = jsonc_parser::parse_to_serde_value::<Option<RawConfig>>(text, &Default::default());
    parsed
        .map(Option::unwrap_or_default)
        .map_err(|e| VelyrError::invalid_data(format!("{}: {e}", path.display())))
}

impl RawConfig {
    /// Own options only (no `extends`); relative paths are anchored at `dir`.
    pub(crate) fn options(&mut self, dir: &Path) -> CompilerOptions {
        let raw = self.compiler_options.take().unwrap_or_default();
        CompilerOptions {
            experimental_decorators: raw.experimental_decorators,
            emit_decorator_metadata: raw.emit_decorator_metadata,
            use_define_for_class_fields: raw.use_define_for_class_fields,
            target: raw.target,
            verbatim_module_syntax: raw.verbatim_module_syntax,
            strict: raw.strict,
            strict_null_checks: raw.strict_null_checks,
            jsx: raw.jsx,
            jsx_import_source: raw.jsx_import_source,
            jsx_factory: raw.jsx_factory,
            jsx_fragment_factory: raw.jsx_fragment_factory,
            base_url: raw.base_url.map(|url| normalize(&dir.join(url))),
            paths: raw.paths.map(|map| Paths {
                base: dir.to_path_buf(),
                map,
            }),
        }
    }
}

/// Lexical `.`/`..` removal (no filesystem access, so symlinks are not followed).
pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}
