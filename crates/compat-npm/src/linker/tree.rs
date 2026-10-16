//! Materializes one cached package directory: hardlink first, plain copy as the fallback.

use std::io;
use std::path::Path;

use velyr_runtime_errors::VelyrError;

/// Cache bookkeeping file that must never reach `node_modules`.
const CACHE_META: &str = ".velyr-meta.json";

/// Links `src` into `dst`; `link` is injectable so the copy fallback can be tested without a
/// second filesystem.
pub(super) fn link_tree(
    src: &Path,
    dst: &Path,
    link: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> Result<(), VelyrError> {
    walk(src, dst, link, true).map_err(|e| {
        VelyrError::internal(format!("cannot materialize {}", dst.display())).with_cause(e)
    })
}

pub(super) fn hardlink(src: &Path, dst: &Path) -> io::Result<()> {
    std::fs::hard_link(src, dst)
}

fn walk(
    src: &Path,
    dst: &Path,
    link: &dyn Fn(&Path, &Path) -> io::Result<()>,
    top: bool,
) -> io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if top && name == CACHE_META {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(&name));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&from, &to, link, false)?;
        } else if kind.is_file() && link(&from, &to).is_err() {
            // Cross-device, unsupported or over the link limit: a copy is always correct.
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
