//! Node compatibility wiring for the node and full profiles: where the `node:` facades live,
//! the resolver that maps `node:x` onto them, and what `process` reports.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use velyr_compat_node::{NodeBuiltinResolver, ProcessConfig};
use velyr_runtime_core::ModuleResolverHandle;

/// `<user cache>/velyr/node-facades`, created private (0700) and canonical: the one directory the
/// `ImportLocal` grant of [`crate::permissions::build`] covers besides the project. Without a
/// user cache directory it is a per-process directory under the system temp dir.
pub fn facade_dir() -> io::Result<&'static Path> {
    static DIR: OnceLock<io::Result<PathBuf>> = OnceLock::new();
    DIR.get_or_init(create_facade_dir)
        .as_deref()
        .map_err(|err| io::Error::new(err.kind(), err.to_string()))
}

fn create_facade_dir() -> io::Result<PathBuf> {
    let dir = velyr_tooling_typescript::platform_cache_dir()
        .map(|transform| transform.with_file_name("node-facades"))
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("velyr-node-facades-{}", std::process::id()))
        });
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Fails when the directory belongs to someone else, which is the right outcome.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    dunce::canonicalize(&dir)
}

/// Wraps `inner` so `import 'node:x'` / `import 'x'` reach the builtins. The facades are served
/// from memory (`/$velyr/node/<name>.js`); `facade_dir` only backs the Windows fallback.
pub fn resolver(inner: ModuleResolverHandle) -> ModuleResolverHandle {
    let built = facade_dir().and_then(|dir| NodeBuiltinResolver::new(inner.clone(), dir));
    match built {
        Ok(resolver) => Arc::new(resolver),
        Err(err) => {
            eprintln!("warning: node: builtins are unavailable for import: {err}");
            inner
        }
    }
}

/// `process.argv`, `process.execPath` and `process.exit` of the main isolate.
pub fn process_config(script: &Path) -> ProcessConfig {
    ProcessConfig {
        script: Some(script.to_string_lossy().into_owned()),
        // shortcut: exits the whole process right here, skipping `exit` listeners and a graceful
        // drain; upgrade once the host emits `beforeExit`/`exit` (compat-node `emit_*`).
        on_exit: Some(Arc::new(|code| std::process::exit(code))),
        ..ProcessConfig::default()
    }
}
