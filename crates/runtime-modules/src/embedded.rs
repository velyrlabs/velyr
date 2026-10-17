//! Modules compiled into the binary and served without touching the filesystem or permissions.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use velyr_runtime_errors::VelyrError;

use crate::Result;

/// Module name the transform emits for oxc runtime helpers (`{name}/helpers/{helper}`).
pub const HELPER_MODULE_NAME: &str = "velyr-oxc-runtime";
/// Version of `@oxc-project/runtime` the vendored helpers come from.
pub const OXC_HELPERS_VERSION: &str = "0.154.0";

const SPECIFIER_PREFIX: &str = "velyr-oxc-runtime/helpers/";
const PATH_PREFIX: &str = "/$velyr/oxc-runtime/helpers/";

const FACADE_PATH_PREFIX: &str = "/$velyr/node/";

/// The registered in-memory facades: `names` is the whole set of valid `/$velyr/node/<name>.js`
/// paths (exact match, no prefix rule) and `source` builds the text of one of them.
struct FacadeTable {
    names: &'static [&'static str],
    source: fn(&str) -> Option<String>,
}

static FACADES: OnceLock<FacadeTable> = OnceLock::new();

/// First registration wins; later calls (one per resolver, workers included) are no-ops.
pub(crate) fn register_facades(names: &'static [&'static str], source: fn(&str) -> Option<String>) {
    let _ = FACADES.set(FacadeTable { names, source });
}

const FILES: &[(&str, &str)] = &[
    ("decorate", include_str!("../helpers/decorate.js")),
    ("decorateParam", include_str!("../helpers/decorateParam.js")),
    (
        "decorateMetadata",
        include_str!("../helpers/decorateMetadata.js"),
    ),
    ("usingCtx", include_str!("../helpers/usingCtx.js")),
    ("applyDecs2311", include_str!("../helpers/applyDecs2311.js")),
    ("typeof", include_str!("../helpers/typeof.js")),
    ("checkInRHS", include_str!("../helpers/checkInRHS.js")),
    (
        "setFunctionName",
        include_str!("../helpers/setFunctionName.js"),
    ),
    ("toPropertyKey", include_str!("../helpers/toPropertyKey.js")),
    ("toPrimitive", include_str!("../helpers/toPrimitive.js")),
];

/// `Ok(None)` when `specifier` is not an embedded one; `NotFound` for an unknown helper.
/// A `./name.js` specifier from an embedded `referrer` stays inside the table.
pub(crate) fn resolve(specifier: &str, referrer: &Path) -> Result<Option<PathBuf>> {
    let name = match specifier.strip_prefix(SPECIFIER_PREFIX) {
        Some(name) => name,
        None if is_embedded(referrer) => match specifier
            .strip_prefix("./")
            .and_then(|rest| rest.strip_suffix(".js"))
        {
            Some(name) => name,
            None => return Ok(None),
        },
        None => return Ok(None),
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
    path.to_str()
        .is_some_and(|path| helper_text(path).is_some() || facade_name(path).is_some())
}

fn helper_text(path: &str) -> Option<&'static str> {
    let name = path.strip_prefix(PATH_PREFIX)?.strip_suffix(".js")?;
    FILES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, text)| *text)
}

/// The facade name when `path` is exactly `/$velyr/node/<name>.js` for a registered name.
fn facade_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix(FACADE_PATH_PREFIX)?.strip_suffix(".js")?;
    FACADES.get()?.names.contains(&name).then_some(name)
}

/// The only place that maps an embedded path to its text.
pub(crate) fn source(path: &Path) -> Option<Cow<'static, str>> {
    let path = path.to_str()?;
    if let Some(text) = helper_text(path) {
        return Some(Cow::Borrowed(text));
    }
    let name = facade_name(path)?;
    (FACADES.get()?.source)(name).map(Cow::Owned)
}
