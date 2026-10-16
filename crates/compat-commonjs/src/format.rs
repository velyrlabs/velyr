//! Which loader a file goes through: ES module, CommonJS or JSON.

use std::path::Path;

use crate::analysis::has_module_syntax;
use crate::fscache;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Esm,
    CommonJs,
    Json,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Esm => "esm",
            Self::CommonJs => "commonjs",
            Self::Json => "json",
        }
    }
}

/// `type` of the nearest `package.json` above `dir` (missing or unreadable manifests have none).
fn package_type(dir: &Path) -> Option<String> {
    let manifest = dir
        .ancestors()
        .find_map(|d| fscache::global().manifest(d))?;
    manifest.get("type")?.as_str().map(str::to_owned)
}

/// `.mjs` is ESM, `.cjs` CommonJS, `.json` JSON. For `.js` (and extensionless files) the package
/// `type` decides; without one the source is sniffed for `import`/`export` like Node 22+.
/// TypeScript files are left to the TypeScript transform and count as ESM.
pub fn detect(path: &Path, read: impl FnOnce() -> Option<String>) -> Format {
    match path.extension().and_then(|e| e.to_str()) {
        Some("mjs" | "mts" | "ts" | "tsx") => Format::Esm,
        Some("cjs" | "cts") => Format::CommonJs,
        Some("json") => Format::Json,
        _ => match package_type(path.parent().unwrap_or(path)).as_deref() {
            Some("module") => Format::Esm,
            Some("commonjs") => Format::CommonJs,
            _ if read().is_some_and(|source| has_module_syntax(&source)) => Format::Esm,
            _ => Format::CommonJs,
        },
    }
}
