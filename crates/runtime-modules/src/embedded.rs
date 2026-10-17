//! Modules compiled into the binary and served without touching the filesystem or permissions.

use std::path::{Path, PathBuf};

use velyr_runtime_errors::VelyrError;

use crate::Result;

/// Module name the transform emits for oxc runtime helpers (`{name}/helpers/{helper}`).
pub const HELPER_MODULE_NAME: &str = "velyr-oxc-runtime";
/// Version of `@oxc-project/runtime` the vendored helpers come from.
pub const OXC_HELPERS_VERSION: &str = "0.154.0";

const SPECIFIER_PREFIX: &str = "velyr-oxc-runtime/helpers/";
const PATH_PREFIX: &str = "/$velyr/oxc-runtime/helpers/";

const FILES: &[(&str, &str)] = &[
    ("decorate", include_str!("../helpers/decorate.js")),
    ("decorateParam", include_str!("../helpers/decorateParam.js")),
    (
        "decorateMetadata",
        include_str!("../helpers/decorateMetadata.js"),
    ),
    ("usingCtx", include_str!("../helpers/usingCtx.js")),
];

/// `Ok(None)` when `specifier` is not an embedded one; `NotFound` for an unknown helper.
pub(crate) fn resolve(specifier: &str) -> Result<Option<PathBuf>> {
    let Some(name) = specifier.strip_prefix(SPECIFIER_PREFIX) else {
        return Ok(None);
    };
    if !FILES.iter().any(|(known, _)| *known == name) {
        return Err(VelyrError::not_found(format!(
            "unknown runtime helper: {specifier}"
        )));
    }
    Ok(Some(PathBuf::from(format!("{PATH_PREFIX}{name}.js"))))
}

/// Single predicate for "this path is served from the binary, never from disk".
pub fn is_embedded(path: &Path) -> bool {
    source(path).is_some()
}

/// The only place that maps an embedded path to its text.
pub(crate) fn source(path: &Path) -> Option<&'static str> {
    let name = path
        .to_str()?
        .strip_prefix(PATH_PREFIX)?
        .strip_suffix(".js")?;
    FILES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, text)| *text)
}
