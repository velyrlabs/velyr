//! The blocking half of `node:fs`: plain `std::fs` work on paths and files that `fs_ops` already
//! authorized. Nothing here touches the permission model or the isolate.

// `FsError` is large but only exists on the failure path.
#![allow(clippy::result_large_err)]

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use velyr_io_fs::sync_ops::{FsError, directory_read_error};

pub(crate) type Parts = Vec<Vec<u8>>;
pub(crate) type Outcome = Result<Parts, FsError>;

/// Maps an `io::Error` for `syscall` on the path the caller passed in.
pub(crate) fn at(syscall: &'static str, raw: String) -> impl Fn(io::Error) -> FsError {
    move |err| FsError::from_io(&err, syscall, Some(&raw))
}

fn text(value: impl ToString) -> Vec<u8> {
    value.to_string().into_bytes()
}

/// Parsed `fs.open` flags (`r`, `r+`, `w`, `wx`, `w+`, `a`, `ax`, `a+`, ... with optional `s`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Flags {
    pub read: bool,
    pub write: bool,
    pub append: bool,
    pub create: bool,
    pub truncate: bool,
    pub exclusive: bool,
}

impl Flags {
    pub fn parse(flags: &str) -> Option<Self> {
        let none = Self {
            read: false,
            write: false,
            append: false,
            create: false,
            truncate: false,
            exclusive: false,
        };
        let normalized: String = flags.chars().filter(|c| *c != 's').collect();
        let (base, exclusive) = match normalized.as_str() {
            "wx" | "xw" => ("w", true),
            "wx+" | "xw+" => ("w+", true),
            "ax" | "xa" => ("a", true),
            "ax+" | "xa+" => ("a+", true),
            other => (other, false),
        };
        let mut parsed = match base {
            "r" => Self { read: true, ..none },
            "r+" => Self {
                read: true,
                write: true,
                ..none
            },
            "w" => Self {
                write: true,
                create: true,
                truncate: true,
                ..none
            },
            "w+" => Self {
                read: true,
                write: true,
                create: true,
                truncate: true,
                ..none
            },
            "a" => Self {
                append: true,
                create: true,
                ..none
            },
            "a+" => Self {
                read: true,
                append: true,
                create: true,
                ..none
            },
            _ => return None,
        };
        parsed.exclusive = exclusive;
        Some(parsed)
    }

    /// Whether the call changes the filesystem (needs `FsWrite`).
    pub fn writes(&self) -> bool {
        self.write || self.append
    }

    pub fn open(&self, path: &Path, mode: u32) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options
            .read(self.read)
            .write(self.write)
            .append(self.append)
            .truncate(self.truncate)
            .create(self.create && !self.exclusive)
            .create_new(self.exclusive);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
        #[cfg(not(unix))]
        let _ = mode;
        options.open(path)
    }
}

fn millis(time: io::Result<SystemTime>) -> f64 {
    match time {
        Ok(time) => match time.duration_since(UNIX_EPOCH) {
            Ok(after) => after.as_secs_f64() * 1000.0,
            Err(before) => -(before.duration().as_secs_f64() * 1000.0),
        },
        Err(_) => 0.0,
    }
}

/// `[dev, mode, nlink, uid, gid, rdev, blksize, ino, size, blocks, atimeMs, mtimeMs, ctimeMs,
/// birthtimeMs]` as decimal text.
pub(crate) fn stat_parts(meta: &Metadata) -> Parts {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let ms = |sec: i64, nsec: i64| sec as f64 * 1000.0 + nsec as f64 / 1e6;
        let ctime = ms(meta.ctime(), meta.ctime_nsec());
        let birth = match meta.created() {
            Ok(time) => millis(Ok(time)),
            Err(_) => ctime,
        };
        vec![
            text(meta.dev()),
            text(meta.mode()),
            text(meta.nlink()),
            text(meta.uid()),
            text(meta.gid()),
            text(meta.rdev()),
            text(meta.blksize()),
            text(meta.ino()),
            text(meta.size()),
            text(meta.blocks()),
            text(ms(meta.atime(), meta.atime_nsec())),
            text(ms(meta.mtime(), meta.mtime_nsec())),
            text(ctime),
            text(birth),
        ]
    }
    #[cfg(not(unix))]
    {
        let kind = if meta.is_dir() { 0o040000 } else { 0o100000 };
        let mode = kind
            | if meta.permissions().readonly() {
                0o444
            } else {
                0o666
            };
        let mtime = millis(meta.modified());
        vec![
            text(0),
            text(mode),
            text(1),
            text(0),
            text(0),
            text(0),
            text(4096),
            text(0),
            text(meta.len()),
            text(meta.len().div_ceil(512)),
            text(millis(meta.accessed())),
            text(mtime),
            text(mtime),
            text(millis(meta.created())),
        ]
    }
}

fn kind_char(kind: fs::FileType) -> u8 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if kind.is_fifo() {
            return b'p';
        }
        if kind.is_socket() {
            return b's';
        }
        if kind.is_block_device() {
            return b'b';
        }
        if kind.is_char_device() {
            return b'c';
        }
    }
    match (kind.is_file(), kind.is_dir(), kind.is_symlink()) {
        (true, ..) => b'f',
        (_, true, _) => b'd',
        (.., true) => b'l',
        _ => b'u',
    }
}

/// Alternating name and type letter (`f d l p s b c u`) for each entry, sorted by name.
pub(crate) fn read_dir(path: &Path, raw: String) -> Outcome {
    let fail = at("scandir", raw);
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(&fail)? {
        let entry = entry.map_err(&fail)?;
        let kind = entry.file_type().map_err(&fail)?;
        entries.push((
            entry.file_name().to_string_lossy().into_owned(),
            kind_char(kind),
        ));
    }
    entries.sort();
    Ok(entries
        .into_iter()
        .flat_map(|(name, kind)| [name.into_bytes(), vec![kind]])
        .collect())
}

/// Reads the whole file; a directory fails with `EISDIR` on the read, like Node.
pub(crate) fn read_all(path: &Path, raw: String) -> Outcome {
    let mut file = File::open(path).map_err(|err| {
        let err = directory_read_error(path, err);
        let syscall = if err.kind() == io::ErrorKind::IsADirectory {
            "read"
        } else {
            "open"
        };
        at(syscall, raw.clone())(err)
    })?;
    let mut data = Vec::new();
    file.read_to_end(&mut data).map_err(at("read", raw))?;
    Ok(vec![data])
}

pub(crate) fn write_all(path: &Path, raw: String, flags: Flags, mode: u32, data: &[u8]) -> Outcome {
    let mut file = flags.open(path, mode).map_err(at("open", raw.clone()))?;
    file.write_all(data).map_err(at("write", raw))?;
    Ok(vec![])
}

/// Creates `path`; with `recursive`, every missing ancestor too, returning the first one created.
pub(crate) fn make_dir(path: &Path, raw: String, recursive: bool, mode: u32) -> Outcome {
    let fail = at("mkdir", raw);
    let first_created = path.ancestors().take_while(|dir| !dir.exists()).last();
    let first_created = first_created.map(Path::to_path_buf);
    let mut builder = fs::DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, mode);
    #[cfg(not(unix))]
    let _ = mode;
    builder.create(path).map_err(&fail)?;
    match (recursive, first_created) {
        (true, Some(first)) => Ok(vec![first.to_string_lossy().into_owned().into_bytes()]),
        _ => Ok(vec![]),
    }
}

/// `fs.rm`: a directory needs `recursive`; `force` swallows a missing path.
pub(crate) fn remove(path: &Path, raw: String, recursive: bool, force: bool) -> Outcome {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if force && err.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(err) => return Err(at("stat", raw)(err)),
    };
    if meta.is_dir() {
        if !recursive {
            let mut error = FsError::new(
                "ERR_FS_EISDIR",
                21,
                &format!("Path is a directory: rm returned EISDIR (is a directory) {raw}"),
                "rm",
                Some(&raw),
            );
            error.cause = None;
            return Err(error);
        }
        fs::remove_dir_all(path).map_err(at("rm", raw))?;
    } else {
        fs::remove_file(path).map_err(at("rm", raw))?;
    }
    Ok(vec![])
}

/// Approximate `access(2)` from the permission bits (no libc: the crate forbids unsafe).
/// `mode` is the Node bit set: `R_OK` 4, `W_OK` 2, `X_OK` 1.
pub(crate) fn check_access(path: &Path, raw: String, mode: u32) -> Outcome {
    let meta = fs::metadata(path).map_err(at("access", raw.clone()))?;
    let denied = || {
        Err(FsError::from_io(
            &io::ErrorKind::PermissionDenied.into(),
            "access",
            Some(&raw),
        ))
    };
    if mode & 2 != 0 && meta.permissions().readonly() {
        return denied();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bits = meta.permissions().mode();
        if (mode & 4 != 0 && bits & 0o444 == 0) || (mode & 1 != 0 && bits & 0o111 == 0) {
            return denied();
        }
    }
    Ok(vec![])
}

#[cfg(unix)]
pub(crate) fn chmod(path: &Path, raw: String, mode: u32) -> Outcome {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(at("chmod", raw))?;
    Ok(vec![])
}

#[cfg(not(unix))]
pub(crate) fn chmod(_path: &Path, _raw: String, _mode: u32) -> Outcome {
    Ok(vec![])
}

fn system_time(ms: f64) -> SystemTime {
    let span = Duration::from_secs_f64(ms.abs() / 1000.0);
    if ms >= 0.0 {
        UNIX_EPOCH + span
    } else {
        UNIX_EPOCH - span
    }
}

pub(crate) fn set_times(path: &Path, raw: String, atime_ms: f64, mtime_ms: f64) -> Outcome {
    let fail = at("utime", raw);
    let file = open_for_times(path).map_err(&fail)?;
    let times = fs::FileTimes::new()
        .set_accessed(system_time(atime_ms))
        .set_modified(system_time(mtime_ms));
    file.set_times(times).map_err(&fail)?;
    Ok(vec![])
}

#[cfg(not(windows))]
fn open_for_times(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// SetFileTime needs FILE_WRITE_ATTRIBUTES, which a read-only open lacks; backup semantics
/// lets the same call open directories.
#[cfg(windows)]
fn open_for_times(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// `prefix` plus six random characters, created as a directory (retried on collision).
pub(crate) fn make_temp_dir(parent: &Path, name: &str, raw: &str) -> Outcome {
    use std::hash::{BuildHasher, RandomState};
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let fail = at("mkdtemp", format!("{raw}XXXXXX"));
    for _ in 0..16 {
        let mut seed = RandomState::new().hash_one(std::time::Instant::now());
        let suffix: String = (0..6)
            .map(|_| {
                let pick = ALPHABET[(seed % ALPHABET.len() as u64) as usize] as char;
                seed /= ALPHABET.len() as u64;
                pick
            })
            .collect();
        let target: PathBuf = parent.join(format!("{name}{suffix}"));
        match fs::create_dir(&target) {
            Ok(()) => return Ok(vec![format!("{raw}{suffix}").into_bytes()]),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(fail(err)),
        }
    }
    Err(fail(io::ErrorKind::AlreadyExists.into()))
}

/// Reads at `position` (or at the current offset when `None`); returns the bytes read.
pub(crate) fn read_fd(file: &File, length: usize, position: Option<u64>) -> Outcome {
    let mut buffer = vec![0u8; length];
    let count = match position {
        Some(at) => read_at(file, &mut buffer, at),
        None => (&*file).read(&mut buffer),
    }
    .map_err(|err| FsError::from_io(&err, "read", None))?;
    buffer.truncate(count);
    Ok(vec![buffer])
}

pub(crate) fn write_fd(file: &File, data: &[u8], position: Option<u64>) -> Outcome {
    let result = match position {
        Some(at) => write_at(file, data, at),
        None => (&*file).write_all(data),
    };
    result.map_err(|err| FsError::from_io(&err, "write", None))?;
    Ok(vec![text(data.len())])
}

#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], position: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buffer, position)
}

#[cfg(unix)]
fn write_at(file: &File, data: &[u8], position: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, data, position)
}

#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], position: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buffer, position)
}

#[cfg(windows)]
fn write_at(file: &File, data: &[u8], position: u64) -> io::Result<()> {
    let mut done = 0;
    while done < data.len() {
        done +=
            std::os::windows::fs::FileExt::seek_write(file, &data[done..], position + done as u64)?;
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn read_at(_file: &File, _buffer: &mut [u8], _position: u64) -> io::Result<usize> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(not(any(unix, windows)))]
fn write_at(_file: &File, _data: &[u8], _position: u64) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}
