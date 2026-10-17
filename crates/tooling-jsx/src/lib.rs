//! JSX to JavaScript for the module host (ADR-012): oxc parses and rewrites JSX, automatic
//! runtime from `react` by default. `.tsx` is also type-stripped in the same pass.
//! Config comes from `velyr.json` `compilerOptions`, falling back to `tsconfig.json`; the
//! `@jsx`, `@jsxFrag`, `@jsxRuntime` and `@jsxImportSource` pragmas override it per file.
//! Register with `ModuleHost::with_transform(Arc::new(JsxTransform::discover(dir)?))`.

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{JsxOptions, JsxRuntime, TransformOptions, Transformer};
use serde_json::{Map, Value};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{MediaType, Result, SourceTransform, Transformed};

const KEYS: [&str; 4] = ["jsx", "jsxImportSource", "jsxFactory", "jsxFragmentFactory"];

/// Project-wide JSX settings. `Default` is the automatic runtime from `react`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct JsxConfig {
    pub classic: bool,
    pub import_source: Option<String>,
    pub factory: Option<String>,
    pub fragment_factory: Option<String>,
}

impl JsxConfig {
    /// Reads the `jsx*` keys of a `compilerOptions` object.
    pub fn from_compiler_options(options: &Value) -> Result<Self> {
        let text = |key: &str| options.get(key).and_then(Value::as_str).map(str::to_owned);
        let classic = match text("jsx").as_deref() {
            None | Some("automatic" | "react-jsx" | "react-jsxdev") => false,
            Some("classic" | "react") => true,
            Some(other) => {
                return Err(VelyrError::invalid_data(format!(
                    "unsupported compilerOptions.jsx \"{other}\" (use react-jsx or react)"
                )));
            }
        };
        Ok(Self {
            classic,
            import_source: text("jsxImportSource"),
            factory: text("jsxFactory"),
            fragment_factory: text("jsxFragmentFactory"),
        })
    }

    /// Finds the nearest directory (from `start` upwards) with a `velyr.json` or `tsconfig.json`.
    /// Each key comes from `velyr.json` when it sets it, else from `tsconfig.json`.
    /// `tsconfig.json` must be strict JSON (no comments, no `extends` resolution).
    pub fn discover(start: &Path) -> Result<Self> {
        let Some(dir) = start
            .ancestors()
            .find(|d| d.join("velyr.json").is_file() || d.join("tsconfig.json").is_file())
        else {
            return Ok(Self::default());
        };
        let velyr = compiler_options(&dir.join("velyr.json"))?;
        let tsconfig = compiler_options(&dir.join("tsconfig.json"))?;
        let merged: Map<String, Value> = KEYS
            .iter()
            .filter_map(|key| {
                let value = velyr.get(*key).or_else(|| tsconfig.get(*key))?;
                Some(((*key).to_owned(), value.clone()))
            })
            .collect();
        Self::from_compiler_options(&Value::Object(merged))
    }

    fn options(&self) -> TransformOptions {
        let runtime = if self.classic {
            JsxRuntime::Classic
        } else {
            JsxRuntime::Automatic
        };
        TransformOptions {
            jsx: JsxOptions {
                runtime,
                import_source: self.import_source.clone(),
                pragma: self.factory.clone(),
                pragma_frag: self.fragment_factory.clone(),
                ..JsxOptions::default()
            },
            ..TransformOptions::default()
        }
    }
}

/// The `compilerOptions` object of a JSON file; empty when the file or the key is absent.
fn compiler_options(path: &Path) -> Result<Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(Value::Null);
    };
    let json: Value = serde_json::from_str(&text)
        .map_err(|e| VelyrError::invalid_data(format!("{}: {e}", path.display())))?;
    Ok(json.get("compilerOptions").cloned().unwrap_or(Value::Null))
}

#[derive(Default)]
pub struct JsxTransform {
    config: JsxConfig,
}

impl JsxTransform {
    pub fn new(config: JsxConfig) -> Self {
        Self { config }
    }

    /// Config discovered from `start` upwards, see [`JsxConfig::discover`].
    pub fn discover(start: &Path) -> Result<Self> {
        Ok(Self::new(JsxConfig::discover(start)?))
    }
}

/// `.jsx` shares `MediaType::JavaScript`, so the specifier tells it apart.
fn source_type(specifier: &str, media_type: MediaType) -> Option<SourceType> {
    let file = match media_type {
        MediaType::Tsx => "module.tsx",
        MediaType::JavaScript if specifier.ends_with(".jsx") => "module.jsx",
        _ => return None,
    };
    SourceType::from_path(file)
        .ok()
        .map(|t| t.with_module(true))
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

impl SourceTransform for JsxTransform {
    fn handles(&self, media_type: MediaType) -> bool {
        matches!(media_type, MediaType::Tsx | MediaType::JavaScript)
    }

    fn transform(
        &self,
        specifier: &str,
        source: &str,
        media_type: MediaType,
    ) -> Result<Transformed> {
        let Some(source_type) = source_type(specifier, media_type) else {
            return Ok(Transformed {
                code: source.to_owned(),
                source_map: None,
            });
        };
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, source_type).parse();
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
        let semantic = SemanticBuilder::new().with_enum_eval(true).build(&program);
        let path = Path::new(specifier);
        let transformed = Transformer::new(&allocator, path, &self.config.options())
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
}
