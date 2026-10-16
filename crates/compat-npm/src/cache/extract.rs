//! Safe tarball extraction: every entry is validated before it touches the disk.
//!
//! Layout rule: exactly one leading path component (`package/`) is stripped, like npm does.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha512};
use tar::{Archive, Entry, EntryType};
use velyr_runtime_errors::VelyrError;

use super::{META_FILE, io_error};

const COPY_BUFFER: usize = 64 * 1024;

/// Upper bounds applied while extracting one tarball.
#[derive(Debug, Clone, Copy)]
pub struct ExtractLimits {
    /// Sum of the sizes of all file entries.
    pub max_total_bytes: u64,
    /// Number of tar entries, directories and links included.
    pub max_entries: usize,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self {
            max_total_bytes: 1024 * 1024 * 1024,
            max_entries: 100_000,
        }
    }
}

pub(super) struct Stats {
    pub files: u64,
    pub bytes: u64,
}

/// Checks the sha512 of `tarball`, then extracts it into the not yet existing `dest`.
pub(super) fn verify_and_extract(
    tarball: &Path,
    expected_sha512: &[u8],
    dest: &Path,
    limits: ExtractLimits,
) -> Result<Stats, VelyrError> {
    let mut file = File::open(tarball).map_err(|e| io_error("cannot open tarball", e))?;
    let mut hasher = Sha512::new();
    std::io::copy(&mut file, &mut hasher).map_err(|e| io_error("cannot read tarball", e))?;
    if hasher.finalize().as_slice() != expected_sha512 {
        return Err(VelyrError::invalid_data("tarball integrity check failed"));
    }
    file.rewind()
        .map_err(|e| io_error("cannot read tarball", e))?;
    fs::create_dir(dest).map_err(|e| io_error("cannot create package directory", e))?;
    let root =
        fs::canonicalize(dest).map_err(|e| io_error("cannot resolve package directory", e))?;
    let mut extractor = Extractor {
        root,
        limits,
        entries: 0,
        stats: Stats { files: 0, bytes: 0 },
    };
    let mut archive = Archive::new(GzDecoder::new(file));
    for entry in archive.entries().map_err(bad_archive)? {
        extractor.extract(entry.map_err(bad_archive)?)?;
    }
    Ok(extractor.stats)
}

struct Extractor {
    /// Canonical package directory; nothing may be written outside of it.
    root: PathBuf,
    limits: ExtractLimits,
    entries: usize,
    stats: Stats,
}

impl Extractor {
    fn extract<R: Read>(&mut self, mut entry: Entry<'_, R>) -> Result<(), VelyrError> {
        let kind = entry.header().entry_type();
        if matches!(kind, EntryType::XGlobalHeader) {
            return Ok(());
        }
        self.entries += 1;
        if self.entries > self.limits.max_entries {
            return Err(VelyrError::quota_exceeded("tarball entries"));
        }
        let raw = entry.path().map_err(bad_archive)?.into_owned();
        let Some(rel) = strip_root(&raw)? else {
            return match kind {
                EntryType::Directory => Ok(()),
                _ => Err(unsafe_entry("entry outside the package directory")),
            };
        };
        if rel == Path::new(META_FILE) {
            return Err(unsafe_entry("entry uses a reserved name"));
        }
        match kind {
            EntryType::Directory => self.make_dir(&rel),
            EntryType::Regular | EntryType::Continuous => self.write_file(&rel, &mut entry),
            EntryType::Symlink => self.write_symlink(&rel, &entry),
            EntryType::Link => self.write_hardlink(&rel, &entry),
            _ => Err(unsafe_entry("unsupported entry type")),
        }
    }

    /// Creates `rel`'s parent directories and returns their canonical path, which is
    /// guaranteed to be inside the package directory.
    fn real_parent(&self, rel: &Path) -> Result<PathBuf, VelyrError> {
        self.real_dir(rel.parent().unwrap_or(Path::new("")))
    }

    fn real_dir(&self, rel_dir: &Path) -> Result<PathBuf, VelyrError> {
        let dir = self.root.join(rel_dir);
        fs::create_dir_all(&dir).map_err(|e| io_error("cannot create directory", e))?;
        let real = fs::canonicalize(&dir).map_err(|e| io_error("cannot resolve directory", e))?;
        if !real.starts_with(&self.root) {
            return Err(unsafe_entry("entry escapes the package directory"));
        }
        Ok(real)
    }

    fn slot(&self, rel: &Path) -> Result<PathBuf, VelyrError> {
        let name = rel
            .file_name()
            .ok_or_else(|| unsafe_entry("entry has no file name"))?;
        let path = self.real_parent(rel)?.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => Err(unsafe_entry("entry replaces a directory")),
            Ok(_) => fs::remove_file(&path)
                .map(|()| path)
                .map_err(|e| io_error("cannot replace entry", e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path),
            Err(e) => Err(io_error("cannot inspect entry", e)),
        }
    }

    fn make_dir(&self, rel: &Path) -> Result<(), VelyrError> {
        self.real_dir(rel).map(|_| ())
    }

    fn write_file<R: Read>(
        &mut self,
        rel: &Path,
        entry: &mut Entry<'_, R>,
    ) -> Result<(), VelyrError> {
        let size = entry.size();
        self.stats.bytes = self.stats.bytes.saturating_add(size);
        if self.stats.bytes > self.limits.max_total_bytes {
            return Err(VelyrError::quota_exceeded("tarball size"));
        }
        let path = self.slot(rel)?;
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| io_error("cannot create file", e))?;
        copy_entry(entry, &mut out)?;
        drop(out);
        set_file_mode(&path, entry.header().mode().unwrap_or(0o644))?;
        self.stats.files += 1;
        Ok(())
    }

    fn write_symlink<R: Read>(&self, rel: &Path, entry: &Entry<'_, R>) -> Result<(), VelyrError> {
        let target = entry
            .link_name()
            .map_err(bad_archive)?
            .ok_or_else(|| unsafe_entry("symlink without target"))?;
        let link = self.slot(rel)?;
        let parent = link.parent().unwrap_or(&self.root);
        if !symlink_stays_inside(&self.root, parent, &target) {
            return Err(unsafe_entry("symlink target escapes the package directory"));
        }
        create_symlink(&target, &link)
    }

    fn write_hardlink<R: Read>(&self, rel: &Path, entry: &Entry<'_, R>) -> Result<(), VelyrError> {
        let target = entry
            .link_name()
            .map_err(bad_archive)?
            .ok_or_else(|| unsafe_entry("hardlink without target"))?;
        let target_rel =
            strip_root(&target)?.ok_or_else(|| unsafe_entry("hardlink target is not a file"))?;
        let source = fs::canonicalize(self.root.join(target_rel))
            .map_err(|_| unsafe_entry("hardlink target was not extracted"))?;
        let is_file = fs::metadata(&source).is_ok_and(|m| m.is_file());
        if !source.starts_with(&self.root) || !is_file {
            return Err(unsafe_entry(
                "hardlink target escapes the package directory",
            ));
        }
        let link = self.slot(rel)?;
        fs::hard_link(&source, &link).map_err(|e| io_error("cannot create hardlink", e))
    }
}

/// Normalizes an archive path and drops its first component. `None` means the root itself.
fn strip_root(path: &Path) -> Result<Option<PathBuf>, VelyrError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            _ => return Err(unsafe_entry("absolute or parent path in tarball")),
        }
    }
    if parts.len() < 2 {
        return Ok(None);
    }
    Ok(Some(parts[1..].iter().collect()))
}

/// Accepts relative targets whose `..` run only leads the path and that land inside `root`.
fn symlink_stays_inside(root: &Path, parent: &Path, target: &Path) -> bool {
    let mut resolved = parent.to_path_buf();
    let mut leading = true;
    for component in target.components() {
        match component {
            Component::ParentDir if leading => {
                resolved.pop();
            }
            Component::Normal(part) => {
                leading = false;
                resolved.push(part);
            }
            Component::CurDir => {}
            _ => return false,
        }
    }
    !target.as_os_str().is_empty() && resolved.starts_with(root)
}

fn copy_entry(from: &mut impl Read, to: &mut impl Write) -> Result<(), VelyrError> {
    let mut buffer = vec![0u8; COPY_BUFFER];
    loop {
        let read = from.read(&mut buffer).map_err(bad_archive)?;
        if read == 0 {
            return Ok(());
        }
        to.write_all(&buffer[..read])
            .map_err(|e| io_error("cannot write file", e))?;
    }
}

fn bad_archive(cause: std::io::Error) -> VelyrError {
    VelyrError::invalid_data("invalid or truncated tarball").with_cause(cause)
}

fn unsafe_entry(reason: &str) -> VelyrError {
    VelyrError::invalid_data(format!("unsafe tarball rejected: {reason}"))
}

/// Keeps only the executable bit: files become 0o755 or 0o644 (no setuid/setgid/sticky).
#[cfg(unix)]
fn set_file_mode(path: &Path, mode: u32) -> Result<(), VelyrError> {
    use std::os::unix::fs::PermissionsExt;
    let normalized = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(normalized))
        .map_err(|e| io_error("cannot set file mode", e))
}

#[cfg(not(unix))]
fn set_file_mode(_path: &Path, _mode: u32) -> Result<(), VelyrError> {
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> Result<(), VelyrError> {
    std::os::unix::fs::symlink(target, link).map_err(|e| io_error("cannot create symlink", e))
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _link: &Path) -> Result<(), VelyrError> {
    Err(VelyrError::unsupported(
        "symlinks in tarballs need a Unix host",
    ))
}
