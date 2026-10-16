//! CommonJS for the runtime (Node 24 semantics): the `require` loader, the ESM facade that lets
//! `import` read a CommonJS file, and the static analysis behind its named exports.
//!
//! Two halves share one cache (`require.cache`) inside the isolate:
//! - [`install_commonjs`] evaluates [`COMMONJS_JS`] with the `op_cjs_*` host functions: `require`,
//!   `require.resolve`, JSON, `.js`/`.cjs`, cycles with partial exports, `import()` from CommonJS.
//! - [`CommonJsTransform`] is a `SourceTransform` for the module host: a CommonJS (or JSON) file
//!   that ESM imports becomes a facade module with `default = module.exports` plus detected names.
//!
//! `require(esm)` returns the module namespace through the runtime's `EsmRequire` slot (install it
//! next to [`install_commonjs`]); a graph with top-level await throws `ERR_REQUIRE_ASYNC_MODULE`.
//!
//! Known limits: `require(esm)` ignores the `"module.exports"` export name; bare specifiers need `import`-side support
//! in compat-npm's resolver (it rejects CommonJS packages); TypeScript sources are not handled.

mod analysis;
mod format;
mod fscache;
mod ops;
mod resolve;

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use velyr_engine_api::{HostFunctionDef, Isolate, JsException, Source};
use velyr_runtime_modules::{MediaType, Result, SourceTransform, Transformed, authorize_path};
use velyr_runtime_ops::CapabilityKind;
use velyr_runtime_permissions::CapabilitySet;

use analysis::analyze;
use format::{Format, detect};
use ops::CommonJsState;

pub use format::Format as ModuleFormat;
pub use fscache::clear_fs_caches;

/// Bootstrap script: installs `require` machinery behind `Symbol.for("velyr.commonjs")`.
pub const COMMONJS_JS: &str = include_str!("commonjs.js");

/// Conditions `require` matches in package `exports`.
pub const REQUIRE_CONDITIONS: [&str; 4] = ["velyr", "require", "node", "default"];

/// The CommonJS ops with the capability each declares, for the registry audit.
pub fn commonjs_ops() -> Vec<(HostFunctionDef, CapabilityKind)> {
    ops::OPS.to_vec()
}

pub fn commonjs_host_functions() -> Vec<HostFunctionDef> {
    commonjs_ops().into_iter().map(|(def, _)| def).collect()
}

/// Runs [`COMMONJS_JS`]. Call after the ops are registered; `permissions` gate every file read.
pub fn install_commonjs(
    isolate: &mut impl Isolate,
    permissions: Arc<CapabilitySet>,
) -> std::result::Result<(), JsException> {
    isolate.set_slot(CommonJsState {
        permissions,
        conditions: conditions(),
    });
    isolate
        .eval_script(&Source {
            name: "velyr:commonjs".into(),
            text: COMMONJS_JS.into(),
        })
        .map(drop)
}

const MAX_REEXPORT_DEPTH: usize = 8;

/// Turns CommonJS and JSON files that ESM imports into facade modules; ESM passes through.
pub struct CommonJsTransform {
    permissions: Arc<CapabilitySet>,
}

impl CommonJsTransform {
    /// `permissions` also gate the files read to follow `module.exports = require("x")`.
    pub fn new(permissions: Arc<CapabilitySet>) -> Self {
        Self { permissions }
    }

    /// Names detected in `source`, following re-exports through files `permissions` allow.
    fn named_exports(&self, path: &Path, source: &str, seen: &mut Vec<String>) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let Some(found) = analyze(source) else {
            return names;
        };
        names.extend(found.exports);
        for request in found.reexports {
            let Ok(target) = resolve::resolve(&request, path, &crate::conditions()) else {
                continue;
            };
            let key = target.to_string_lossy().into_owned();
            if seen.len() >= MAX_REEXPORT_DEPTH
                || seen.contains(&key)
                || authorize_path(&target, &self.permissions).is_err()
            {
                continue;
            }
            let Ok(text) = fs::read_to_string(&target) else {
                continue;
            };
            if detect(&target, || Some(text.clone())) == Format::CommonJs {
                seen.push(key);
                names.extend(self.named_exports(&target, &text, seen));
            }
        }
        names.remove("default");
        names
    }
}

fn conditions() -> Vec<String> {
    REQUIRE_CONDITIONS.map(str::to_owned).to_vec()
}

fn literal(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

fn facade(path: &Path, names: &BTreeSet<String>) -> String {
    let mut code = format!(
        "const __m = globalThis[Symbol.for(\"velyr.commonjs\")].importCjs({});\nexport default __m;\nexport {{ __m as \"module.exports\" }};\n",
        literal(&path.to_string_lossy())
    );
    for (index, name) in names.iter().enumerate() {
        let name = literal(name);
        code.push_str(&format!(
            "const __e{index} = __m[{name}];\nexport {{ __e{index} as {name} }};\n"
        ));
    }
    code
}

impl SourceTransform for CommonJsTransform {
    fn transform(
        &self,
        specifier: &str,
        source: &str,
        media_type: MediaType,
    ) -> Result<Transformed> {
        let path = Path::new(specifier);
        let format = if media_type == MediaType::JavaScript {
            detect(path, || Some(source.to_owned()))
        } else {
            Format::Esm
        };
        let code = match format {
            Format::Esm => source.to_owned(),
            Format::Json => facade(path, &BTreeSet::new()),
            Format::CommonJs => {
                let names = self.named_exports(path, source, &mut vec![specifier.to_owned()]);
                facade(path, &names)
            }
        };
        Ok(Transformed {
            code,
            source_map: None,
        })
    }
}
