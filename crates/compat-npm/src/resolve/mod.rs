//! Node package resolution for bare specifiers (ESM): `node_modules` walk, `exports`, `imports`,
//! conditions, self-reference and the `main`/`index.js` fallback (ADR-013).

pub mod exports;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use velyr_runtime_errors::VelyrError;
use velyr_runtime_modules::{BareSpecifierResolver, Result};

use crate::PackageJson;
use exports::{match_exports, match_imports};

/// Conditions matched by every ESM resolution; `default` always matches.
const BASE_CONDITIONS: [&str; 3] = ["velyr", "import", "default"];

#[derive(Debug, Clone)]
pub struct NpmResolver {
    conditions: Vec<String>,
    commonjs: bool,
}

impl Default for NpmResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl NpmResolver {
    pub fn new() -> Self {
        Self {
            conditions: BASE_CONDITIONS.map(str::to_owned).to_vec(),
            commonjs: false,
        }
    }

    /// Replaces the condition set (`default` always matches). `["velyr", "require", "node",
    /// "default"]` is what CommonJS `require` uses.
    pub fn with_conditions(mut self, conditions: &[&str]) -> Self {
        self.conditions = conditions.iter().map(|c| (*c).to_owned()).collect();
        self
    }

    /// Resolves packages whose target is CommonJS instead of rejecting them; whoever loads the
    /// file (the CommonJS transform) decides how to run it. Off by default.
    pub fn allow_commonjs(mut self) -> Self {
        self.commonjs = true;
        self
    }

    /// Adds the `node` condition; only valid once Node compatibility is linked (M7).
    pub fn with_node_condition(mut self) -> Self {
        self.conditions.push("node".to_owned());
        self
    }
}

impl BareSpecifierResolver for NpmResolver {
    fn resolve_bare(&self, specifier: &str, referrer: &Path) -> Result<PathBuf> {
        let _span = tracing::info_span!("velyr.compat_npm.resolve_specifier", specifier).entered();
        let target = if specifier.starts_with('#') {
            self.resolve_imports(specifier, referrer)?
        } else {
            self.resolve_package(specifier, referrer)?
        };
        if !self.commonjs {
            ensure_esm(specifier, &target)?;
        }
        Ok(target)
    }
}

/// A `package.json` and the directory that holds it.
struct Package {
    dir: PathBuf,
    manifest: PackageJson,
}

impl Package {
    fn load(dir: &Path) -> Result<Option<Self>> {
        let file = dir.join("package.json");
        let Ok(raw) = fs::read_to_string(&file) else {
            return Ok(None);
        };
        let manifest = PackageJson::parse(&raw).map_err(|err| {
            VelyrError::invalid_data(format!("invalid package config {}", file.display()))
                .with_cause(err)
        })?;
        Ok(Some(Self {
            dir: dir.to_owned(),
            manifest,
        }))
    }

    fn file(&self) -> PathBuf {
        self.dir.join("package.json")
    }
}

/// Nearest `package.json` above `start`, not crossing a `node_modules` directory.
fn package_scope(start: &Path) -> Result<Option<Package>> {
    for dir in start.ancestors() {
        if dir.file_name().is_some_and(|name| name == "node_modules") {
            return Ok(None);
        }
        if let Some(package) = Package::load(dir)? {
            return Ok(Some(package));
        }
    }
    Ok(None)
}

impl NpmResolver {
    fn resolve_package(&self, specifier: &str, referrer: &Path) -> Result<PathBuf> {
        let (name, subpath) = split_specifier(specifier)?;
        let from = referrer.parent().unwrap_or(referrer);
        if let Some(scope) = package_scope(from)?
            && scope.manifest.name.as_deref() == Some(name)
            && let Some(exports) = &scope.manifest.exports
        {
            return self.resolve_exports(&scope, exports, &subpath, referrer);
        }
        for ancestor in from.ancestors() {
            let dir = ancestor.join("node_modules").join(name);
            if dir.is_dir() {
                return self.resolve_in_package(&dir, &subpath, referrer);
            }
        }
        Err(VelyrError::not_found(format!(
            "Cannot find package '{name}' imported from {}",
            referrer.display()
        )))
    }

    fn resolve_in_package(&self, dir: &Path, subpath: &str, referrer: &Path) -> Result<PathBuf> {
        let package = Package::load(dir)?.unwrap_or_else(|| Package {
            dir: dir.to_owned(),
            manifest: PackageJson::default(),
        });
        if let Some(exports) = &package.manifest.exports {
            return self.resolve_exports(&package, exports, subpath, referrer);
        }
        if subpath == "." {
            return legacy_main(&package, referrer);
        }
        finalize(dir.join(subpath), referrer)
    }

    fn resolve_exports(
        &self,
        package: &Package,
        exports: &Value,
        subpath: &str,
        referrer: &Path,
    ) -> Result<PathBuf> {
        let not_exported = || {
            VelyrError::not_found(format!(
                "Package subpath '{subpath}' is not defined by \"exports\" in {} imported from {}",
                package.file().display(),
                referrer.display()
            ))
        };
        match match_exports(exports, subpath, &self.conditions, &package.dir)? {
            Some(path) => finalize(path, referrer),
            None => Err(not_exported()),
        }
    }

    fn resolve_imports(&self, specifier: &str, referrer: &Path) -> Result<PathBuf> {
        if specifier == "#" || specifier.starts_with("#/") {
            return Err(VelyrError::invalid_data(format!(
                "invalid import specifier '{specifier}'"
            )));
        }
        let from = referrer.parent().unwrap_or(referrer);
        let not_defined = |location: String| {
            VelyrError::not_found(format!(
                "Package import specifier '{specifier}' is not defined{location} imported from {}",
                referrer.display()
            ))
        };
        let Some(package) = package_scope(from)? else {
            return Err(not_defined(String::new()));
        };
        let location = format!(" in package {}", package.file().display());
        let package_file = package.file();
        let external = |target: &str| self.resolve_package(target, &package_file);
        let imports = package.manifest.imports.clone().unwrap_or(Value::Null);
        match match_imports(
            &imports,
            specifier,
            &self.conditions,
            &package.dir,
            &external,
        )? {
            Some(path) => finalize(path, referrer),
            None => Err(not_defined(location)),
        }
    }
}

/// Splits `@scope/name/rest` into the package name and the `./rest` subpath (`.` when empty).
fn split_specifier(specifier: &str) -> Result<(&str, String)> {
    let invalid = || VelyrError::invalid_data(format!("invalid package specifier '{specifier}'"));
    let scope_len = match specifier.strip_prefix('@') {
        Some(rest) => rest.find('/').map(|slash| slash + 2).ok_or_else(invalid)?,
        None => 0,
    };
    let name_end = specifier[scope_len..]
        .find('/')
        .map_or(specifier.len(), |slash| scope_len + slash);
    let (name, rest) = specifier.split_at(name_end);
    let bare_name = &name[scope_len..];
    let scope_is_empty = scope_len == 2;
    if bare_name.is_empty() || scope_is_empty || name.starts_with('.') || name.contains(['\\', '%'])
    {
        return Err(invalid());
    }
    Ok((name, format!(".{rest}")))
}

/// Legacy entry: `main` (as is, `.js`, `/index.js`), then `index.js`.
fn legacy_main(package: &Package, referrer: &Path) -> Result<PathBuf> {
    let from_main = package.manifest.main.iter().flat_map(|main| {
        let base = package.dir.join(main);
        let with_js = PathBuf::from(format!("{}.js", base.display()));
        [base.clone(), with_js, base.join("index.js")]
    });
    from_main
        .chain([package.dir.join("index.js")])
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            VelyrError::not_found(format!(
                "Cannot find package entry of {} imported from {}",
                package.dir.display(),
                referrer.display()
            ))
        })
}

fn finalize(path: PathBuf, referrer: &Path) -> Result<PathBuf> {
    if path.is_file() {
        return Ok(path);
    }
    Err(VelyrError::not_found(format!(
        "Cannot find module '{}' imported from {}",
        path.display(),
        referrer.display()
    )))
}

/// The package scope `type` decides `.js`; CommonJS needs Node compatibility (M7).
fn ensure_esm(specifier: &str, target: &Path) -> Result<()> {
    let is_module = match target.extension().and_then(|ext| ext.to_str()) {
        Some("cjs") => false,
        Some("js") => {
            let scope = package_scope(target.parent().unwrap_or(target))?;
            scope.is_some_and(|package| package.manifest.r#type.as_deref() == Some("module"))
        }
        _ => true,
    };
    if is_module {
        return Ok(());
    }
    Err(VelyrError::unsupported(format!(
        "'{specifier}' resolves to a CommonJS module ({}); CommonJS needs Node compatibility, \
         which is not linked",
        target.display()
    )))
}
