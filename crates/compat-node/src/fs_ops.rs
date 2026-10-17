//! Host ops behind `node:fs`. Two entry points share one dispatcher: `op_node_fs` (sync) and
//! `op_node_fs_async` (a promise; the I/O runs on the blocking pool). Inputs are an op name plus
//! typed arrays (numbers as decimal text); the result is one ArrayBuffer: a status byte (0 ok,
//! 1 error) and then parts, each a big-endian `u32` length plus bytes. An error result is the
//! seven parts of `FsError::to_parts`: code, errno, syscall, path, dest, message, cause.
//!
//! Every op is split in two: `prepare` runs on the isolate thread, resolves each path to its
//! canonical form and authorizes that (never the raw string) and touches the resource table;
//! the returned job only does `std::fs` work on what was authorized, so it is safe to move to
//! another thread. `open` and `close` do their work inside `prepare` for the same reason.

// `FsError` is large but only exists on the failure path.
#![allow(clippy::result_large_err)]

use std::fs;
use std::path::PathBuf;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};
use velyr_io_fs::sync_ops::{
    Access, FsError, PathMode, authorize_path, close_fd, deny_nested_in_tree, file_for_fd,
    insert_file, verify_unchanged,
};
use velyr_runtime_errors::VelyrError;
use velyr_runtime_event_loop::CompletionValue;
use velyr_runtime_ops::op_async;

use crate::fs_copy as copy;
use crate::fs_jobs::{self as jobs, Flags, Outcome, at};

type Job = Box<dyn FnOnce() -> Outcome + Send>;

pub(crate) fn fs_host_functions() -> Vec<HostFunctionDef> {
    vec![
        HostFunctionDef {
            name: "op_node_fs",
            arity: 1,
            callback: op_sync,
        },
        HostFunctionDef {
            name: "op_node_fs_async",
            arity: 1,
            callback: op_async_call,
        },
    ]
}

fn job(work: impl FnOnce() -> Outcome + Send + 'static) -> Job {
    Box::new(work)
}

fn arg(args: &[Vec<u8>], index: usize) -> Result<String, FsError> {
    args.get(index)
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .ok_or_else(|| FsError::invalid("missing argument", "fs", None))
}

fn num(args: &[Vec<u8>], index: usize) -> Result<f64, FsError> {
    arg(args, index)?
        .parse()
        .map_err(|_| FsError::invalid("invalid numeric argument", "fs", None))
}

fn flag(args: &[Vec<u8>], index: usize) -> Result<bool, FsError> {
    Ok(arg(args, index)? == "1")
}

/// A position argument: negative means "the current offset".
fn position(args: &[Vec<u8>], index: usize) -> Result<Option<u64>, FsError> {
    let value = num(args, index)?;
    Ok((value >= 0.0).then_some(value as u64))
}

fn done() -> Result<Job, FsError> {
    Ok(job(|| Ok(vec![])))
}

/// A path `prepare` authorized. Async jobs run after the JS turn that issued them, so they
/// re-resolve every guard before their I/O (`verify_unchanged`).
struct Guard {
    path: PathBuf,
    mode: PathMode,
    syscall: String,
    raw: String,
}

fn authorize(
    ctx: &mut dyn CallContext,
    guards: &mut Vec<Guard>,
    syscall: &str,
    raw: &str,
    mode: PathMode,
    access: Access,
) -> Result<PathBuf, FsError> {
    let path = authorize_path(ctx, syscall, raw, mode, access)?;
    guards.push(Guard {
        path: path.clone(),
        mode,
        syscall: syscall.into(),
        raw: raw.into(),
    });
    Ok(path)
}

/// Authorizes the path an `open`-style call touches, from the access its flags imply.
fn authorize_flags(
    ctx: &mut dyn CallContext,
    guards: &mut Vec<Guard>,
    syscall: &str,
    raw: &str,
    flags: &Flags,
) -> Result<PathBuf, FsError> {
    let mode = if flags.create {
        PathMode::Create
    } else {
        PathMode::Follow
    };
    if !flags.writes() {
        return authorize(ctx, guards, syscall, raw, mode, Access::Read);
    }
    let path = authorize(ctx, guards, syscall, raw, mode, Access::Write)?;
    if flags.read {
        authorize(ctx, guards, syscall, raw, mode, Access::Read)?;
    }
    Ok(path)
}

fn parse_flags(value: &str) -> Result<Flags, FsError> {
    Flags::parse(value).ok_or_else(|| {
        let message = format!("The argument 'flags' is invalid. Received '{value}'");
        FsError::new("ERR_INVALID_ARG_VALUE", 22, &message, "open", None)
    })
}

fn prepare(
    ctx: &mut dyn CallContext,
    guards: &mut Vec<Guard>,
    name: &str,
    a: &[Vec<u8>],
) -> Result<Job, FsError> {
    use PathMode::{Create, Deep, Entry, Follow};
    let read = Access::Read;
    let write = Access::Write;
    match name {
        "stat" | "lstat" => {
            let (follow, syscall) = if name == "stat" {
                (true, "stat")
            } else {
                (false, "lstat")
            };
            let raw = arg(a, 0)?;
            let mode = if follow { Follow } else { Entry };
            let path = authorize(ctx, guards, syscall, &raw, mode, read)?;
            Ok(job(move || {
                let meta = if follow {
                    fs::metadata(&path)
                } else {
                    fs::symlink_metadata(&path)
                };
                Ok(jobs::stat_parts(&meta.map_err(at(syscall, raw))?))
            }))
        }
        "fstat" => {
            let file = file_for_fd(ctx, num(a, 0)?, "fstat")?;
            Ok(job(move || {
                let meta = file
                    .metadata()
                    .map_err(|err| FsError::from_io(&err, "fstat", None))?;
                Ok(jobs::stat_parts(&meta))
            }))
        }
        "readFile" => {
            let raw = arg(a, 0)?;
            let authorized = authorize(ctx, guards, "open", &raw, Follow, read);
            // A compiled executable's embedded files are not on disk: `authorize` checks the raw
            // path first and fails with ENOENT only once the grant allowed it.
            if let Some(vfs) = velyr_runtime_vfs::covering(std::path::Path::new(&raw)) {
                if let Err(err) = authorized
                    && err.code != "ENOENT"
                {
                    return Err(err);
                }
                let embedded = vfs.read(&raw).map(<[u8]>::to_vec);
                return Ok(job(move || {
                    let missing = std::io::ErrorKind::NotFound.into();
                    let data =
                        embedded.ok_or_else(|| FsError::from_io(&missing, "open", Some(&raw)))?;
                    Ok(vec![data])
                }));
            }
            let path = authorized?;
            Ok(job(move || jobs::read_all(&path, raw)))
        }
        "writeFile" => {
            let (raw, data) = (arg(a, 0)?, a.get(1).cloned().unwrap_or_default());
            let flags = parse_flags(&arg(a, 2)?)?;
            let mode = num(a, 3)? as u32;
            let path = authorize_flags(ctx, guards, "open", &raw, &flags)?;
            Ok(job(move || jobs::write_all(&path, raw, flags, mode, &data)))
        }
        "readdir" => {
            let raw = arg(a, 0)?;
            let path = authorize(ctx, guards, "scandir", &raw, Follow, read)?;
            Ok(job(move || jobs::read_dir(&path, raw)))
        }
        "mkdir" => {
            let (raw, recursive, mode) = (arg(a, 0)?, flag(a, 1)?, num(a, 2)? as u32);
            let path_mode = if recursive { Deep } else { Create };
            let path = authorize(ctx, guards, "mkdir", &raw, path_mode, write)?;
            Ok(job(move || jobs::make_dir(&path, raw, recursive, mode)))
        }
        "rm" => {
            let (raw, recursive, force) = (arg(a, 0)?, flag(a, 1)?, flag(a, 2)?);
            let authorized = authorize(ctx, guards, "rm", &raw, Entry, write).and_then(|path| {
                if recursive {
                    deny_nested_in_tree(ctx, "rm", &raw, &path, write)?;
                }
                Ok(path)
            });
            match authorized {
                Ok(path) => Ok(job(move || jobs::remove(&path, raw, recursive, force))),
                Err(err) if force && err.code == "ENOENT" => done(),
                Err(err) => Err(err),
            }
        }
        "rmdir" | "unlink" => {
            let (syscall, raw) = (if name == "rmdir" { "rmdir" } else { "unlink" }, arg(a, 0)?);
            let path = authorize(ctx, guards, syscall, &raw, Entry, write)?;
            Ok(job(move || {
                let result = if syscall == "rmdir" {
                    fs::remove_dir(&path)
                } else {
                    fs::remove_file(&path)
                };
                result.map_err(at(syscall, raw))?;
                Ok(vec![])
            }))
        }
        "rename" => {
            let (from_raw, to_raw) = (arg(a, 0)?, arg(a, 1)?);
            let both = |err: FsError| {
                FsError {
                    path: Some(from_raw.clone()),
                    ..err
                }
                .with_dest(&to_raw)
            };
            let from = authorize(ctx, guards, "rename", &from_raw, Entry, write).map_err(both)?;
            deny_nested_in_tree(ctx, "rename", &from_raw, &from, write).map_err(both)?;
            let to = authorize(ctx, guards, "rename", &to_raw, Entry, write).map_err(both)?;
            Ok(job(move || {
                let fail = |err: std::io::Error| {
                    FsError::from_io(&err, "rename", Some(&from_raw)).with_dest(&to_raw)
                };
                fs::rename(&from, &to).map_err(fail)?;
                Ok(vec![])
            }))
        }
        "copyFile" | "cp" => {
            let (from_raw, to_raw) = (arg(a, 0)?, arg(a, 1)?);
            let syscall = if name == "cp" { "cp" } else { "copyfile" };
            let both = |err: FsError| {
                FsError {
                    path: Some(from_raw.clone()),
                    ..err
                }
                .with_dest(&to_raw)
            };
            let source_mode = if name == "cp" { Entry } else { Follow };
            let from =
                authorize(ctx, guards, syscall, &from_raw, source_mode, read).map_err(both)?;
            let to = authorize(ctx, guards, syscall, &to_raw, Create, write).map_err(both)?;
            if name == "cp" && flag(a, 2)? {
                // The copy duplicates whatever is below the root, so a nested deny of either kind blocks it.
                for access in [read, write] {
                    deny_nested_in_tree(ctx, syscall, &from_raw, &from, access).map_err(both)?;
                }
            }
            let raw = (from_raw, to_raw);
            if name == "cp" {
                let (recursive, force, error_on_exist) = (flag(a, 2)?, flag(a, 3)?, flag(a, 4)?);
                return Ok(job(move || {
                    copy::copy_tree(&from, &to, raw, recursive, force, error_on_exist)
                }));
            }
            let exclusive = num(a, 2)? as u32 & 1 != 0;
            Ok(job(move || copy::copy_file(&from, &to, raw, exclusive)))
        }
        "access" => {
            let (raw, mode) = (arg(a, 0)?, num(a, 1)? as u32);
            let path = authorize(ctx, guards, "access", &raw, Follow, read)?;
            if mode & 2 != 0 {
                authorize(ctx, guards, "access", &raw, Follow, write)?;
            }
            Ok(job(move || jobs::check_access(&path, raw, mode)))
        }
        "realpath" => {
            let raw = arg(a, 0)?;
            let path = authorize(ctx, guards, "lstat", &raw, Follow, read)?;
            Ok(job(move || {
                Ok(vec![path.to_string_lossy().into_owned().into_bytes()])
            }))
        }
        "symlink" => {
            // The target is only text until something follows the link, and following it is
            // authorized then, so only the link's own location is checked here.
            let (target, raw) = (arg(a, 0)?, arg(a, 1)?);
            let link = authorize(ctx, guards, "symlink", &raw, Entry, write).map_err(|err| {
                FsError {
                    path: Some(target.clone()),
                    ..err
                }
                .with_dest(&raw)
            })?;
            Ok(job(move || {
                let fail = |err: std::io::Error| {
                    FsError::from_io(&err, "symlink", Some(&target)).with_dest(&raw)
                };
                copy::symlink(std::path::Path::new(&target), &link).map_err(fail)?;
                Ok(vec![])
            }))
        }
        "readlink" => {
            let raw = arg(a, 0)?;
            let path = authorize(ctx, guards, "readlink", &raw, Entry, read)?;
            Ok(job(move || {
                let target = fs::read_link(&path).map_err(at("readlink", raw))?;
                Ok(vec![target.to_string_lossy().into_owned().into_bytes()])
            }))
        }
        "chmod" => {
            let (raw, mode) = (arg(a, 0)?, num(a, 1)? as u32);
            let path = authorize(ctx, guards, "chmod", &raw, Follow, write)?;
            Ok(job(move || jobs::chmod(&path, raw, mode)))
        }
        "utimes" => {
            let (raw, atime, mtime) = (arg(a, 0)?, num(a, 1)?, num(a, 2)?);
            let path = authorize(ctx, guards, "utime", &raw, Follow, write)?;
            Ok(job(move || jobs::set_times(&path, raw, atime, mtime)))
        }
        "mkdtemp" => {
            // Authorized as `<prefix>XXXXXX`: a grant must cover the directory that will hold it.
            let prefix = arg(a, 0)?;
            let placeholder = format!("{prefix}XXXXXX");
            let path = authorize(ctx, guards, "mkdtemp", &placeholder, Create, write)?;
            let (parent, stem) = (path.parent().map(PathBuf::from), path.file_name());
            let stem = stem.map(|n| n.to_string_lossy().trim_end_matches("XXXXXX").to_string());
            let (Some(parent), Some(stem)) = (parent, stem) else {
                return Err(FsError::invalid(
                    "invalid path",
                    "mkdtemp",
                    Some(&placeholder),
                ));
            };
            Ok(job(move || jobs::make_temp_dir(&parent, &stem, &prefix)))
        }
        "open" => {
            let (raw, flags, mode) = (arg(a, 0)?, parse_flags(&arg(a, 1)?)?, num(a, 2)? as u32);
            // Opened right here: nothing to re-check, and no fd to leak on a failed check.
            let path = authorize_flags(ctx, &mut Vec::new(), "open", &raw, &flags)?;
            let file = flags.open(&path, mode).map_err(at("open", raw))?;
            let fd = insert_file(ctx, file, "open")?;
            Ok(job(move || Ok(vec![fd.to_string().into_bytes()])))
        }
        "close" => {
            close_fd(ctx, num(a, 0)?, "close")?;
            done()
        }
        "read" => {
            let file = file_for_fd(ctx, num(a, 0)?, "read")?;
            let (length, at) = (num(a, 1)? as usize, position(a, 2)?);
            Ok(job(move || jobs::read_fd(&file, length, at)))
        }
        "write" => {
            let file = file_for_fd(ctx, num(a, 0)?, "write")?;
            let (data, at) = (a.get(1).cloned().unwrap_or_default(), position(a, 2)?);
            Ok(job(move || jobs::write_fd(&file, &data, at)))
        }
        _ => Err(FsError::invalid(
            &format!("unknown fs operation {name}"),
            "fs",
            None,
        )),
    }
}

fn encode(result: Outcome) -> Vec<u8> {
    let (status, parts) = match result {
        Ok(parts) => (0u8, parts),
        Err(error) => (1u8, error.to_parts()),
    };
    let mut out = vec![status];
    for part in parts {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(&part);
    }
    out
}

fn read_call(
    ctx: &mut dyn CallContext,
    args: &[Value],
) -> Result<(String, Vec<Vec<u8>>), HostError> {
    let Some((Value::String(name), rest)) = args.split_first() else {
        return Err(HostError::TypeError(
            "fs op expects (name, ...bytes)".into(),
        ));
    };
    let parts = rest
        .iter()
        .map(|value| ctx.read_bytes(value))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| HostError::TypeError("fs op arguments must be typed arrays".into()))?;
    Ok((name.to_string_lossy(), parts))
}

fn op_sync(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (name, parts) = read_call(ctx, args)?;
    // The job runs right after its check, with no JS in between: nothing to re-check.
    let result = prepare(ctx, &mut Vec::new(), &name, &parts).and_then(|work| work());
    Ok(ctx.create_array_buffer(&encode(result)))
}

/// Wraps `work` so it first re-resolves every path `prepare` authorized.
fn guarded(guards: Vec<Guard>, work: Job) -> Job {
    job(move || {
        for guard in &guards {
            verify_unchanged(&guard.path, guard.mode, &guard.syscall, &guard.raw)?;
        }
        work()
    })
}

fn op_async_call(ctx: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let (name, parts) = read_call(ctx, args)?;
    let mut guards = Vec::new();
    let prepared = prepare(ctx, &mut guards, &name, &parts).map(|work| guarded(guards, work));
    op_async(ctx, |_token| async move {
        let result = match prepared {
            Ok(work) => tokio::task::spawn_blocking(work)
                .await
                .map_err(|err| VelyrError::internal(err.to_string()))?,
            Err(error) => Err(error),
        };
        Ok(CompletionValue::Bytes(encode(result)))
    })
}
