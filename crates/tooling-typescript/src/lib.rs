//! TypeScript to JavaScript for the module host (ADR-012): oxc parses, strips types (no type
//! checking; `isolatedModules` is assumed) and prints with a source map. Results are cached on disk.
//! Register with `ModuleHost::with_transform(Arc::new(TypeScriptTransform::with_platform_cache()))`.

mod cache;
mod options;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::Transformer;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{MediaType, Result, SourceTransform, Transformed};
use velyr_tooling_tsconfig::{TsConfig, TsconfigLoader};

pub use cache::platform_cache_dir;
pub use options::{transform_options, warn_once};

/// Must match the `=x.y.z` of every `oxc_*` dependency: output can change between versions.
const OXC_VERSION: &str = "0.154.0";

pub struct TypeScriptTransform {
    cache_dir: Option<PathBuf>,
    hits: AtomicU64,
    loader: Option<Arc<TsconfigLoader>>,
}

impl TypeScriptTransform {
    /// `cache_dir` of `None` disables the disk cache.
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self {
            cache_dir,
            hits: AtomicU64::new(0),
            loader: None,
        }
    }

    /// Derives per-file transform options from the nearest `tsconfig.json`.
    pub fn with_tsconfig(mut self, loader: Arc<TsconfigLoader>) -> Self {
        self.loader = Some(loader);
        self
    }

    /// Caches under [`platform_cache_dir`].
    pub fn with_platform_cache() -> Self {
        Self::new(platform_cache_dir())
    }

    /// How many transforms were served from the disk cache.
    pub fn cache_hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }
}

fn source_type(media_type: MediaType) -> SourceType {
    let file = match media_type {
        MediaType::JavaScript => "module.mjs",
        MediaType::TypeScript => "module.ts",
        MediaType::Mts => "module.mts",
        MediaType::Cts => "module.cts",
        MediaType::Tsx => "module.tsx",
    };
    let source_type = SourceType::from_path(file).unwrap_or_default();
    if media_type == MediaType::Cts {
        source_type
    } else {
        source_type.with_module(true)
    }
}

/// 1-based line and column (in characters) of a byte offset.
fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let before = &source[..offset.min(source.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

fn failure(specifier: &str, source: &str, message: &str, offset: Option<usize>) -> VelyrError {
    let (line, column) = offset.map_or((1, 1), |offset| line_column(source, offset));
    VelyrError::invalid_data(format!("{specifier}:{line}:{column}: {message}"))
}

fn strip(
    specifier: &str,
    source: &str,
    media_type: MediaType,
    config: Option<&TsConfig>,
) -> Result<Transformed> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, source_type(media_type)).parse();
    if let Some(first) = parsed.diagnostics.first() {
        return Err(failure(
            specifier,
            source,
            &first.message,
            first
                .labels
                .first()
                .and_then(|label| usize::try_from(label.offset()).ok()),
        ));
    }
    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .with_enum_eval(true)
        .build(&program);
    let path = Path::new(specifier);
    let transformed = Transformer::new(&allocator, path, &transform_options(media_type, config))
        .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    if let Some(first) = transformed.diagnostics.first() {
        return Err(failure(
            specifier,
            source,
            &first.message,
            first
                .labels
                .first()
                .and_then(|label| usize::try_from(label.offset()).ok()),
        ));
    }
    let printed = Codegen::new()
        .with_options(CodegenOptions {
            source_map_path: Some(path.to_path_buf()),
            ..CodegenOptions::default()
        })
        .build(&program);
    Ok(Transformed {
        code: printed.code,
        source_map: printed.map.map(|map| map.to_json_string()),
    })
}

impl SourceTransform for TypeScriptTransform {
    fn handles(&self, media_type: MediaType) -> bool {
        media_type.is_typescript()
    }

    fn transform(
        &self,
        specifier: &str,
        source: &str,
        media_type: MediaType,
    ) -> Result<Transformed> {
        if !media_type.is_typescript() {
            return Ok(Transformed {
                code: source.to_owned(),
                source_map: None,
            });
        }
        let config = self
            .loader
            .as_ref()
            .and_then(|l| warn_once(l.for_file(Path::new(specifier)), specifier));
        let options = format!("{:?}", transform_options(media_type, config.as_deref()));
        let key = cache::key(&[
            env!("CARGO_PKG_NAME").as_bytes(),
            env!("CARGO_PKG_VERSION").as_bytes(),
            OXC_VERSION.as_bytes(),
            options.as_bytes(),
            format!("{media_type:?}").as_bytes(),
            specifier.as_bytes(),
            source.as_bytes(),
        ]);
        if let Some(hit) = self
            .cache_dir
            .as_deref()
            .and_then(|dir| cache::read(dir, &key))
        {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(hit);
        }
        let done = strip(specifier, source, media_type, config.as_deref())?;
        if let Some(dir) = &self.cache_dir {
            cache::write(dir, &key, &done);
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::OXC_VERSION;

    /// `OXC_VERSION` keys the disk cache, so it must follow the pinned `oxc_*` dependencies.
    #[test]
    fn oxc_version_matches_cargo_toml_and_lock() {
        let manifest = include_str!("../Cargo.toml");
        let pins: Vec<&str> = manifest
            .lines()
            .filter(|l| l.starts_with("oxc_"))
            .filter_map(|l| l.split('"').nth(1))
            .collect();
        assert!(!pins.is_empty());
        let expected = format!("={OXC_VERSION}");
        assert!(pins.iter().all(|p| *p == expected), "{pins:?}");

        let lock = include_str!("../../../Cargo.lock");
        for name in ["oxc_parser", "oxc_transformer", "oxc_codegen"] {
            let header = format!("name = \"{name}\"\nversion = \"{OXC_VERSION}\"");
            assert!(
                lock.contains(&header),
                "{name} in Cargo.lock is not {OXC_VERSION}"
            );
        }
    }
}
