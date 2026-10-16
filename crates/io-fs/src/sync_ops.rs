//! Building blocks for synchronous and callback-style filesystem APIs (`node:fs`): path
//! resolution with authorization on the canonical path, the fd table (open files live in the
//! isolate's resource table; the fd number is the resource id plus [`FD_BASE`]) and an error
//! type that carries what a Node-style error needs (`code`, `errno`, `syscall`, `path`).
//!
//! The functions here do not do the I/O themselves: they authorize, then hand back the path (or
//! the open file) so the caller performs exactly that operation on the blocking pool or inline.

// `FsError` carries five strings so a Node error can be built from it; it only exists on the
// failure path, where its size does not matter.
#![allow(clippy::result_large_err)]

use std::any::Any;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use velyr_engine_api::CallContext;
use velyr_runtime_errors::{ErrorCode, VelyrError};
use velyr_runtime_ops::OpContext;
use velyr_runtime_permissions::Request;
use velyr_runtime_resources::{QuotaClass, Resource, ResourceId};

/// Descriptors 0..=2 belong to stdio, so open files start at 3.
pub const FD_BASE: f64 = 3.0;

/// What a call needs from the permission model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// How the last path component is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    /// The path must exist; symlinks are followed (`stat`, `readFile`, `open` without create).
    Follow,
    /// The path may not exist yet: the canonical parent plus the name (`writeFile`, `mkdir`).
    /// A final component that already exists is followed; a dangling symlink is refused.
    Create,
    /// The directory entry itself, never followed (`lstat`, `readlink`, `unlink`, `rename`).
    Entry,
    /// Like `Create`, but any number of leading components may be missing (`mkdir -p`): the
    /// deepest existing ancestor is canonical, the rest is appended. `..` in the missing part is refused.
    Deep,
}

/// A failed call: everything the JS side needs to build a Node error.
#[derive(Debug, Clone)]
pub struct FsError {
    pub code: String,
    pub errno: i32,
    pub syscall: String,
    pub path: Option<String>,
    pub dest: Option<String>,
    pub message: String,
    /// The `VELYR_*` code of the kernel error behind this one, when there is one.
    pub cause: Option<String>,
}

impl FsError {
    pub fn new(code: &str, errno: i32, message: &str, syscall: &str, path: Option<&str>) -> Self {
        Self {
            code: code.into(),
            errno,
            syscall: syscall.into(),
            path: path.map(str::to_string),
            dest: None,
            message: message.into(),
            cause: None,
        }
    }

    /// Maps an OS error. Unknown kinds fall back to `EIO`.
    pub fn from_io(err: &io::Error, syscall: &str, path: Option<&str>) -> Self {
        let (code, default_errno, message) = classify(err.kind());
        let errno = err.raw_os_error().unwrap_or(default_errno);
        Self::new(code, errno, message, syscall, path)
    }

    pub fn bad_fd(syscall: &str) -> Self {
        Self::new("EBADF", 9, "bad file descriptor", syscall, None)
    }

    pub fn invalid(message: &str, syscall: &str, path: Option<&str>) -> Self {
        Self::new("EINVAL", 22, message, syscall, path)
    }

    /// A kernel error (permission denial, quota) as seen by Node code.
    pub fn from_velyr(err: &VelyrError, syscall: &str, path: Option<&str>) -> Self {
        let mut mapped = match err.code() {
            ErrorCode::PermissionDenied => Self::new(
                "ERR_ACCESS_DENIED",
                0,
                "Access to this API has been restricted",
                syscall,
                path,
            ),
            ErrorCode::QuotaExceeded => {
                Self::new("EMFILE", 24, "too many open files", syscall, path)
            }
            ErrorCode::BadResource => Self::bad_fd(syscall),
            _ => Self::new("EIO", 5, "i/o error", syscall, path),
        };
        mapped.cause = Some(err.code().as_str().to_string());
        mapped
    }

    pub fn with_dest(mut self, dest: &str) -> Self {
        self.dest = Some(dest.to_string());
        self
    }

    /// Wire form: `[code, errno, syscall, path, dest, message, cause]`; absent values are empty.
    pub fn to_parts(&self) -> Vec<Vec<u8>> {
        let text = |value: &Option<String>| value.clone().unwrap_or_default().into_bytes();
        vec![
            self.code.clone().into_bytes(),
            self.errno.to_string().into_bytes(),
            self.syscall.clone().into_bytes(),
            text(&self.path),
            text(&self.dest),
            self.message.clone().into_bytes(),
            text(&self.cause),
        ]
    }
}

fn classify(kind: io::ErrorKind) -> (&'static str, i32, &'static str) {
    use io::ErrorKind as K;
    match kind {
        K::NotFound => ("ENOENT", 2, "no such file or directory"),
        K::PermissionDenied => ("EACCES", 13, "permission denied"),
        K::AlreadyExists => ("EEXIST", 17, "file already exists"),
        K::NotADirectory => ("ENOTDIR", 20, "not a directory"),
        K::IsADirectory => ("EISDIR", 21, "illegal operation on a directory"),
        K::DirectoryNotEmpty => ("ENOTEMPTY", 39, "directory not empty"),
        K::InvalidInput | K::InvalidFilename => ("EINVAL", 22, "invalid argument"),
        K::CrossesDevices => ("EXDEV", 18, "cross-device link not permitted"),
        K::ReadOnlyFilesystem => ("EROFS", 30, "read-only file system"),
        K::StorageFull => ("ENOSPC", 28, "no space left on device"),
        K::ResourceBusy => ("EBUSY", 16, "resource busy or locked"),
        K::FileTooLarge => ("EFBIG", 27, "file too large"),
        K::NotSeekable => ("ESPIPE", 29, "invalid seek"),
        K::TooManyLinks => ("EMLINK", 31, "too many links"),
        K::Unsupported => ("ENOTSUP", 95, "operation not supported on socket"),
        _ => ("EIO", 5, "i/o error"),
    }
}

fn op_context<'a>(
    ctx: &'a mut dyn CallContext,
    syscall: &str,
) -> Result<&'a mut OpContext, FsError> {
    ctx.isolate_slot::<OpContext>()
        .ok_or_else(|| FsError::new("EIO", 5, "ops need an installed OpContext", syscall, None))
}

fn resolve(raw: &str, mode: PathMode) -> io::Result<PathBuf> {
    let path = Path::new(raw);
    match mode {
        PathMode::Follow => path.canonicalize(),
        PathMode::Create => match path.canonicalize() {
            Ok(canonical) => Ok(canonical),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                if path.symlink_metadata().is_ok() {
                    // Writing through a dangling link would create a file nobody authorized.
                    return Err(io::Error::from(io::ErrorKind::PermissionDenied));
                }
                entry_path(path)
            }
            Err(err) => Err(err),
        },
        PathMode::Entry => entry_path(path),
        PathMode::Deep => deep_path(path),
    }
}

fn deep_path(path: &Path) -> io::Result<PathBuf> {
    let mut missing = Vec::new();
    let mut cursor = path;
    loop {
        match cursor.canonicalize() {
            Ok(mut base) => {
                base.extend(missing.iter().rev());
                return Ok(base);
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let name = cursor
                    .file_name()
                    .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
                missing.push(name);
                cursor = match cursor.parent() {
                    Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
                    Some(parent) => parent,
                    None => return Err(err),
                };
            }
            Err(err) => return Err(err),
        }
    }
}

/// The canonical parent plus the final component, which is not followed.
fn entry_path(path: &Path) -> io::Result<PathBuf> {
    let Some(name) = path.file_name() else {
        return path.canonicalize();
    };
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    Ok(parent.canonicalize()?.join(name))
}

/// Resolves `raw`, authorizes the canonical path for `access` and returns it. The caller must
/// act on the returned path, never on `raw` again. A path that cannot be resolved is authorized
/// as given first, so a caller without the grant sees a denial and never learns whether the
/// file exists.
pub fn authorize_path(
    ctx: &mut dyn CallContext,
    syscall: &str,
    raw: &str,
    mode: PathMode,
    access: Access,
) -> Result<PathBuf, FsError> {
    if raw.contains('\0') {
        return Err(FsError::invalid("invalid path", syscall, Some(raw)));
    }
    if raw.is_empty() {
        return Err(FsError::from_io(
            &io::ErrorKind::NotFound.into(),
            syscall,
            Some(raw),
        ));
    }
    let op_ctx = op_context(ctx, syscall)?;
    let check = |op_ctx: &OpContext, target: &str| {
        let request = match access {
            Access::Read => Request::FsRead(target),
            Access::Write => Request::FsWrite(target),
        };
        op_ctx
            .authorize("op_node_fs", request)
            .map_err(|err| FsError::from_velyr(&err, syscall, Some(raw)))
    };
    match resolve(raw, mode) {
        Ok(canonical) => {
            let text = canonical
                .to_str()
                .ok_or_else(|| FsError::invalid("path is not valid UTF-8", syscall, Some(raw)))?;
            if let Err(denied) = check(op_ctx, text) {
                // The permission model resolves symlinks, so a link to somewhere outside the grant
                // would hide the link itself. An entry that is a symlink is never followed here, so
                // it is authorized by the directory that holds it instead.
                let is_link = mode == PathMode::Entry
                    && canonical
                        .symlink_metadata()
                        .is_ok_and(|meta| meta.is_symlink());
                let parent = canonical
                    .parent()
                    .and_then(Path::to_str)
                    .filter(|_| is_link);
                match parent {
                    Some(parent) => check(op_ctx, parent)?,
                    None => return Err(denied),
                }
            }
            Ok(canonical)
        }
        Err(err) => {
            check(op_ctx, raw)?;
            Err(FsError::from_io(&err, syscall, Some(raw)))
        }
    }
}

/// Re-resolves a path [`authorize_path`] returned, right before the I/O on it. A job that runs
/// after the JS turn that authorized it (async ops) would otherwise follow a parent directory or
/// a final name swapped for a link in between. The path must still resolve to itself.
///
/// shortcut: this narrows the window to the syscall itself but does not close it; closing it
/// needs fd-relative resolution (`openat` + `O_NOFOLLOW`, `openat2`), see VELYR-179.
pub fn verify_unchanged(
    path: &Path,
    mode: PathMode,
    syscall: &str,
    raw: &str,
) -> Result<(), FsError> {
    let text = path.to_str().unwrap_or_default();
    match resolve(text, mode) {
        Ok(current) if current == path => Ok(()),
        // Gone since the check: let the operation itself report it.
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => {
            let denied = VelyrError::permission_denied("fs", "<redacted>");
            Err(FsError::from_velyr(&denied, syscall, Some(raw)))
        }
    }
}

/// An open file in the resource table. The `Arc` lets async jobs keep using the file while the
/// isolate thread closes the table entry.
pub struct FsFile(pub Arc<File>);

impl Resource for FsFile {
    fn kind(&self) -> &'static str {
        "fs.file"
    }

    fn close(self: Box<Self>) {}

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Registers `file` and returns its fd (>= 3).
pub fn insert_file(ctx: &mut dyn CallContext, file: File, syscall: &str) -> Result<f64, FsError> {
    let op_ctx = op_context(ctx, syscall)?;
    let id = op_ctx
        .resources
        .insert(Box::new(FsFile(Arc::new(file))), QuotaClass::Files, None)
        .map_err(|err| FsError::from_velyr(&err, syscall, None))?;
    Ok(id.to_js_number() + FD_BASE)
}

fn resource_id(fd: f64, syscall: &str) -> Result<ResourceId, FsError> {
    if fd < FD_BASE {
        return Err(FsError::bad_fd(syscall));
    }
    ResourceId::from_js_number(fd - FD_BASE).map_err(|_| FsError::bad_fd(syscall))
}

/// The open file behind `fd`; anything that is not a live file is `EBADF`.
pub fn file_for_fd(
    ctx: &mut dyn CallContext,
    fd: f64,
    syscall: &str,
) -> Result<Arc<File>, FsError> {
    let id = resource_id(fd, syscall)?;
    let op_ctx = op_context(ctx, syscall)?;
    op_ctx
        .resources
        .get::<FsFile>(id)
        .map(|file| file.0.clone())
        .map_err(|_| FsError::bad_fd(syscall))
}

/// Closes `fd`; a second close is `EBADF`.
pub fn close_fd(ctx: &mut dyn CallContext, fd: f64, syscall: &str) -> Result<(), FsError> {
    let id = resource_id(fd, syscall)?;
    let op_ctx = op_context(ctx, syscall)?;
    op_ctx
        .resources
        .get::<FsFile>(id)
        .map_err(|_| FsError::bad_fd(syscall))?;
    op_ctx
        .resources
        .close(id)
        .map_err(|_| FsError::bad_fd(syscall))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_resolution_keeps_missing_names_and_refuses_parent_components() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let deep = resolve(base.join("a/b/c").to_str().unwrap(), PathMode::Deep).unwrap();
        assert_eq!(deep, base.join("a/b/c"));
        let escape = base.join("a/../../x");
        assert!(resolve(escape.to_str().unwrap(), PathMode::Deep).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn create_refuses_a_dangling_link_but_entry_names_the_link_itself() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let link = base.join("dangling");
        std::os::unix::fs::symlink(base.join("missing"), &link).unwrap();
        let text = link.to_str().unwrap();
        assert!(resolve(text, PathMode::Create).is_err());
        assert_eq!(resolve(text, PathMode::Entry).unwrap(), link);
    }

    #[test]
    fn io_errors_map_to_node_codes() {
        let missing = io::Error::from(io::ErrorKind::NotFound);
        let error = FsError::from_io(&missing, "open", Some("/x"));
        assert_eq!(
            (error.code.as_str(), error.syscall.as_str()),
            ("ENOENT", "open")
        );
        assert_eq!(FsError::bad_fd("close").code, "EBADF");
        assert_eq!(resource_id(2.0, "read").unwrap_err().code, "EBADF");
    }
}
