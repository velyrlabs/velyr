//! The hook between loading a module and handing its text to the engine: TypeScript stripping and
//! similar source-to-source steps register a [`SourceTransform`] on the [`crate::ModuleHost`].

use std::path::Path;

use crate::Result;

/// What a module's file extension says it contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaType {
    JavaScript,
    TypeScript,
    Mts,
    Cts,
    Tsx,
}

impl MediaType {
    /// `.ts`, `.mts`, `.cts` and `.tsx` are TypeScript; every other extension is JavaScript.
    pub fn from_path(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("ts") => Self::TypeScript,
            Some("mts") => Self::Mts,
            Some("cts") => Self::Cts,
            Some("tsx") => Self::Tsx,
            _ => Self::JavaScript,
        }
    }

    pub fn is_typescript(self) -> bool {
        self != Self::JavaScript
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transformed {
    pub code: String,
    /// Source map JSON for `code`, when the transform produces one.
    pub source_map: Option<String>,
}

/// Rewrites a module's text after it is read and authorized. Runs on a blocking thread, so it may do
/// CPU-heavy work and file I/O.
pub trait SourceTransform: Send + Sync {
    /// Whether `media_type` needs this transform; the host skips the blocking-thread hop for the
    /// rest. Defaults to every media type.
    fn handles(&self, _media_type: MediaType) -> bool {
        true
    }

    /// `specifier` is the canonical path of the module, used in error messages and source maps.
    /// Must return `ErrorCode::InvalidData` with `file:line:column` for text it cannot parse.
    fn transform(
        &self,
        specifier: &str,
        source: &str,
        media_type: MediaType,
    ) -> Result<Transformed>;
}
