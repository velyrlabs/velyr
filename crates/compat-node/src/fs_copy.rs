//! `fs.copyFile` and `fs.cp`: blocking copy work on paths `fs_ops` already authorized.

// `FsError` is large but only exists on the failure path.
#![allow(clippy::result_large_err)]

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

use velyr_io_fs::sync_ops::FsError;

use crate::fs_jobs::Outcome;

/// `fs.copyFile`; `exclusive` fails when the destination exists.
pub(crate) fn copy_file(
    src: &Path,
    dest: &Path,
    raw: (String, String),
    exclusive: bool,
) -> Outcome {
    let fail = |err: io::Error| FsError::from_io(&err, "copyfile", Some(&raw.0)).with_dest(&raw.1);
    let mut from = File::open(src).map_err(fail)?;
    let mut options = OpenOptions::new();
    options.write(true);
    if exclusive {
        options.create_new(true);
    } else {
        options.create(true).truncate(true);
    }
    let mut to = options.open(dest).map_err(fail)?;
    io::copy(&mut from, &mut to).map_err(fail)?;
    to.set_permissions(from.metadata().map_err(fail)?.permissions())
        .map_err(fail)?;
    Ok(vec![])
}

/// `fs.cp`: files, and with `recursive` directory trees. Symlinks are copied as links.
pub(crate) fn copy_tree(
    src: &Path,
    dest: &Path,
    raw: (String, String),
    recursive: bool,
    force: bool,
    error_on_exist: bool,
) -> Outcome {
    let fail = |err: io::Error| FsError::from_io(&err, "cp", Some(&raw.0)).with_dest(&raw.1);
    let meta = fs::symlink_metadata(src).map_err(fail)?;
    if meta.is_dir() {
        if !recursive {
            let message = format!(
                "Recursive option is not enabled, cannot copy a directory: {}",
                raw.0
            );
            return Err(FsError::new(
                "ERR_FS_EISDIR",
                21,
                &message,
                "cp",
                Some(&raw.0),
            ));
        }
        if dest.starts_with(src) {
            let message = format!("Cannot copy {} to a subdirectory of self {}", raw.0, raw.1);
            return Err(FsError::new(
                "ERR_FS_CP_EINVAL",
                22,
                &message,
                "cp",
                Some(&raw.0),
            ));
        }
    }
    copy_entry(src, dest, force, error_on_exist).map_err(|err| match err {
        CopyError::Exists => {
            let message = format!(
                "Target already exists: cp returned EEXIST ({} already exists)",
                raw.1
            );
            FsError::new("ERR_FS_CP_EEXIST", 17, &message, "cp", Some(&raw.1))
        }
        CopyError::Io(err) => fail(err),
    })?;
    Ok(vec![])
}

enum CopyError {
    Exists,
    Io(io::Error),
}

impl From<io::Error> for CopyError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

fn copy_entry(src: &Path, dest: &Path, force: bool, error_on_exist: bool) -> Result<(), CopyError> {
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() {
        match fs::create_dir(dest) {
            Ok(()) => {}
            // A pre-existing link is not "a directory": copying into it would leave the tree.
            Err(err)
                if err.kind() == io::ErrorKind::AlreadyExists
                    && fs::symlink_metadata(dest).is_ok_and(|found| found.is_dir()) => {}
            Err(err) => return Err(err.into()),
        }
        for entry in fs::read_dir(src)? {
            let name = entry?.file_name();
            copy_entry(&src.join(&name), &dest.join(&name), force, error_on_exist)?;
        }
        return Ok(fs::set_permissions(dest, meta.permissions())?);
    }
    if fs::symlink_metadata(dest).is_ok() {
        if error_on_exist && !force {
            return Err(CopyError::Exists);
        }
        if !force {
            return Ok(());
        }
        fs::remove_file(dest)?;
    }
    if meta.is_symlink() {
        return Ok(symlink(&fs::read_link(src)?, dest)?);
    }
    fs::copy(src, dest)?;
    Ok(())
}

#[cfg(unix)]
pub(crate) fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
pub(crate) fn symlink(_target: &Path, _link: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}
