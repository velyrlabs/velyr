use std::fs;
use std::path::{Path, PathBuf};

use url::{ParseError, Url};
use velyr_runtime_errors::VelyrError;

use crate::strategy::BareSpecifierResolver;
use crate::{ModuleId, ModuleResolver, ResolveContext, ResolvedModule, Result};

#[derive(Debug, Default, Clone, Copy)]
pub struct FsResolver;

impl ModuleResolver for FsResolver {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &ModuleId,
        ctx: &ResolveContext,
    ) -> Result<ResolvedModule> {
        resolve_with(None, specifier, referrer, ctx)
    }
}

pub(crate) fn resolve_with(
    bare: Option<&dyn BareSpecifierResolver>,
    specifier: &str,
    referrer: &ModuleId,
    ctx: &ResolveContext,
) -> Result<ResolvedModule> {
    let mapped = apply_import_map(specifier, ctx);
    let path = locate(&mapped, referrer, bare)?;
    let path = fs::canonicalize(&path)
        .map_err(|_| VelyrError::not_found(format!("module not found: {specifier}")))?;
    let id = ModuleId::from_path(&path)
        .ok_or_else(|| VelyrError::invalid_data("module path is not representable as a URL"))?;
    Ok(ResolvedModule { id, path })
}

/// Exact key first, then the longest key ending in `/`. Relative values resolve against the map dir.
fn apply_import_map(specifier: &str, ctx: &ResolveContext) -> String {
    let target = match ctx.import_map.get(specifier) {
        Some(value) => Some(value.clone()),
        None => ctx
            .import_map
            .iter()
            .filter(|(key, _)| key.ends_with('/') && specifier.starts_with(key.as_str()))
            .max_by_key(|(key, _)| key.len())
            .map(|(key, value)| format!("{value}{}", &specifier[key.len()..])),
    };
    match target {
        Some(value) if value.starts_with("./") || value.starts_with("../") => ctx
            .import_map_dir
            .join(value)
            .to_string_lossy()
            .into_owned(),
        Some(value) => value,
        None => specifier.to_owned(),
    }
}

/// Classifies the specifier and returns the (not yet canonical) filesystem path.
fn locate(
    specifier: &str,
    referrer: &ModuleId,
    bare: Option<&dyn BareSpecifierResolver>,
) -> Result<PathBuf> {
    match Url::parse(specifier) {
        // One-letter schemes are Windows drive letters.
        Ok(url) if url.scheme().len() > 1 => from_url(&url, specifier),
        Ok(_) | Err(ParseError::RelativeUrlWithoutBase) => {
            from_path_like(specifier, referrer, bare)
        }
        Err(_) => Err(VelyrError::invalid_data(format!(
            "invalid specifier: {specifier}"
        ))),
    }
}

fn from_url(url: &Url, specifier: &str) -> Result<PathBuf> {
    match url.scheme() {
        "file" => url
            .to_file_path()
            .map_err(|()| VelyrError::invalid_data(format!("invalid file URL: {specifier}"))),
        "node" => Err(VelyrError::unsupported(format!(
            "node: modules need Node compatibility, which is not linked: {specifier}"
        ))),
        scheme => Err(VelyrError::unsupported(format!(
            "{scheme}: modules are not supported yet: {specifier}"
        ))),
    }
}

fn from_path_like(
    specifier: &str,
    referrer: &ModuleId,
    bare: Option<&dyn BareSpecifierResolver>,
) -> Result<PathBuf> {
    if Path::new(specifier).is_absolute() {
        return Ok(PathBuf::from(specifier));
    }
    if specifier.starts_with("./") || specifier.starts_with("../") {
        let dir = referrer
            .to_path()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .ok_or_else(|| VelyrError::invalid_data("referrer is not a file module"))?;
        return Ok(dir.join(specifier));
    }
    if let Some(bare) = bare {
        let referrer = referrer
            .to_path()
            .ok_or_else(|| VelyrError::invalid_data("referrer is not a file module"))?;
        return bare.resolve_bare(specifier, &referrer);
    }
    Err(VelyrError::not_found(format!(
        "cannot resolve bare specifier '{specifier}': npm packages are not supported yet; \
         use a relative path or add an entry to the import map (velyr.json \"imports\")"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;
    use velyr_runtime_errors::ErrorCode;

    fn project() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(root.join("vendor")).unwrap();
        for f in [
            "a.js",
            "main.js",
            "sub/b.js",
            "vendor/lib.js",
            "vendor/x.js",
        ] {
            fs::write(root.join(f), "").unwrap();
        }
        (dir, root)
    }

    fn entry(root: &std::path::Path) -> ModuleId {
        ModuleId::from_path(&root.join("main.js")).unwrap()
    }

    fn resolve(root: &std::path::Path, spec: &str, ctx: &ResolveContext) -> Result<ResolvedModule> {
        FsResolver.resolve(spec, &entry(root), ctx)
    }

    #[test]
    fn spellings_converge_to_one_id() {
        let (_d, root) = project();
        let ctx = ResolveContext::default();
        let a = resolve(&root, "./a.js", &ctx).unwrap();
        let b = resolve(&root, "./sub/../a.js", &ctx).unwrap();
        let c = resolve(&root, root.join("a.js").to_str().unwrap(), &ctx).unwrap();
        let url = url::Url::from_file_path(root.join("a.js")).unwrap();
        let d = resolve(&root, url.as_str(), &ctx).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.id, c.id);
        assert_eq!(a.id, d.id);
        assert_eq!(a.id.as_str(), url.as_str());
    }

    #[test]
    fn relative_resolves_against_referrer_directory() {
        let (_d, root) = project();
        let referrer = ModuleId::from_path(&root.join("sub/b.js")).unwrap();
        let got = FsResolver
            .resolve("../a.js", &referrer, &ResolveContext::default())
            .unwrap();
        assert_eq!(got.path, root.join("a.js"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_converges_to_target() {
        let (_d, root) = project();
        std::os::unix::fs::symlink(root.join("a.js"), root.join("link.js")).unwrap();
        let ctx = ResolveContext::default();
        assert_eq!(
            resolve(&root, "./link.js", &ctx).unwrap().id,
            resolve(&root, "./a.js", &ctx).unwrap().id
        );
    }

    #[test]
    fn missing_file_is_not_found() {
        let (_d, root) = project();
        let err = resolve(&root, "./nope.js", &ResolveContext::default()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);
    }

    #[test]
    fn bare_specifier_is_not_found_with_hint() {
        let (_d, root) = project();
        let err = resolve(&root, "left-pad", &ResolveContext::default()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::NotFound);
        assert!(err.message().contains("import map"), "{}", err.message());
    }

    #[test]
    fn node_and_https_are_unsupported() {
        let (_d, root) = project();
        for spec in [
            "node:fs",
            "https://example.com/m.js",
            "http://example.com/m.js",
        ] {
            let err = resolve(&root, spec, &ResolveContext::default()).unwrap_err();
            assert_eq!(err.code(), ErrorCode::Unsupported, "{spec}");
        }
    }

    fn map(root: &std::path::Path, entries: &[(&str, &str)]) -> ResolveContext {
        ResolveContext {
            import_map: entries
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<_, _>>(),
            import_map_dir: root.to_path_buf(),
        }
    }

    #[test]
    fn import_map_exact_key() {
        let (_d, root) = project();
        let ctx = map(&root, &[("lib", "./vendor/lib.js")]);
        assert_eq!(
            resolve(&root, "lib", &ctx).unwrap().path,
            root.join("vendor/lib.js")
        );
    }

    #[test]
    fn import_map_prefix_key_prefers_longest() {
        let (_d, root) = project();
        let ctx = map(&root, &[("v/", "./vendor/"), ("v/x", "./a.js")]);
        assert_eq!(
            resolve(&root, "v/lib.js", &ctx).unwrap().path,
            root.join("vendor/lib.js")
        );
        assert_eq!(resolve(&root, "v/x", &ctx).unwrap().path, root.join("a.js"));
    }

    #[test]
    fn import_map_value_is_relative_to_map_dir_not_referrer() {
        let (_d, root) = project();
        let referrer = ModuleId::from_path(&root.join("sub/b.js")).unwrap();
        let ctx = map(&root, &[("lib", "./vendor/lib.js")]);
        let got = FsResolver.resolve("lib", &referrer, &ctx).unwrap();
        assert_eq!(got.path, root.join("vendor/lib.js"));
    }

    #[test]
    fn import_map_can_point_to_unsupported_scheme() {
        let (_d, root) = project();
        let ctx = map(&root, &[("fs", "node:fs")]);
        assert_eq!(
            resolve(&root, "fs", &ctx).unwrap_err().code(),
            ErrorCode::Unsupported
        );
    }
}
