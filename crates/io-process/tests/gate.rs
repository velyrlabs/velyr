#![cfg(unix)] // velyr-io-process spawns children only on unix; Windows returns Unsupported.
//! The permission matrix of `authorize_spawn`: what a grant by name or by path covers, deny rules,
//! relative PATH entries, and that a denial never depends on whether the program exists.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use velyr_io_process::{GateError, authorize_spawn};
use velyr_runtime_permissions::{Capability, CapabilitySet, ProgramPattern};

const PATH: &str = "/bin:/usr/bin";

fn allow(patterns: &[&str]) -> Arc<CapabilitySet> {
    patterns
        .iter()
        .fold(CapabilitySet::builder(), |builder, pattern| {
            builder.allow(Capability::ProcessSpawn(ProgramPattern::new(*pattern)))
        })
        .build()
}

fn gate(set: &CapabilitySet, program: &str) -> Result<String, GateError> {
    gate_in(set, program, "/", PATH)
}

fn gate_in(set: &CapabilitySet, program: &str, cwd: &str, path: &str) -> Result<String, GateError> {
    authorize_spawn(set, program, Path::new(cwd), Some(OsStr::new(path)))
        .map(|resolved| resolved.path().to_str().unwrap().to_owned())
}

fn canon(path: &str) -> String {
    dunce::canonicalize(path)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned()
}

#[test]
fn no_grant_denies_everything() {
    let none = CapabilitySet::builder().build();
    assert_eq!(gate(&none, "echo"), Err(GateError::Denied));
    assert_eq!(gate(&none, "/bin/echo"), Err(GateError::Denied));
}

#[test]
fn a_name_grant_covers_that_name_wherever_it_resolves() {
    let set = allow(&["echo"]);
    assert!(gate(&set, "echo").unwrap().ends_with("/echo"));
    assert_eq!(gate(&set, "ls"), Err(GateError::Denied));
}

#[test]
fn a_path_grant_covers_the_resolved_path() {
    let by_path = allow(&["/bin/echo"]);
    let echo = canon("/bin/echo");
    assert_eq!(gate(&by_path, "echo").as_deref(), Ok(echo.as_str()));
    assert_eq!(gate(&by_path, "/bin/echo").as_deref(), Ok(echo.as_str()));
    assert_eq!(gate(&by_path, "ls"), Err(GateError::Denied));
}

#[test]
fn a_name_with_a_slash_needs_the_path_not_the_name() {
    // `echo` the name does not make `/bin/echo` (or `./echo`) runnable.
    let set = allow(&["echo"]);
    assert_eq!(gate(&set, "/bin/echo"), Err(GateError::Denied));
    assert_eq!(
        gate_in(&set, "./echo", "/bin", PATH),
        Err(GateError::Denied)
    );
    // Dot segments are resolved before matching: no escaping a directory grant.
    let dir = allow(&["/bin/*"]);
    assert!(gate(&dir, "/bin/echo").is_ok());
    assert_eq!(gate(&dir, "/bin/../usr/bin/env"), Err(GateError::Denied));
    assert_eq!(gate(&dir, "/bin/./../usr/bin/env"), Err(GateError::Denied));
}

#[test]
fn a_relative_path_is_anchored_at_the_given_cwd() {
    let set = allow(&["/bin/echo"]);
    let echo = canon("/bin/echo");
    assert_eq!(
        gate_in(&set, "./echo", "/bin", PATH).as_deref(),
        Ok(echo.as_str())
    );
    assert_eq!(
        gate_in(&set, "../bin/echo", "/tmp", PATH).as_deref(),
        Ok(echo.as_str())
    );
    assert_eq!(
        gate_in(&set, "./echo", "/tmp", PATH),
        Err(GateError::Denied)
    );
}

#[test]
fn deny_on_the_name_or_the_path_wins() {
    let set = CapabilitySet::builder()
        .allow(Capability::ProcessSpawn(ProgramPattern::new("*")))
        .deny(Capability::ProcessSpawn(ProgramPattern::new("/bin/echo")))
        .deny(Capability::ProcessSpawn(ProgramPattern::new("ls")))
        .build();
    assert_eq!(gate(&set, "echo"), Err(GateError::Denied));
    assert_eq!(gate(&set, "/bin/echo"), Err(GateError::Denied));
    assert_eq!(gate(&set, "ls"), Err(GateError::Denied));
    assert!(gate(&set, "cat").is_ok());
}

#[test]
fn denial_is_decided_before_existence() {
    let set = allow(&["echo"]);
    // A missing program that is not granted looks exactly like a forbidden one.
    assert_eq!(gate(&set, "no-such-program-zz"), Err(GateError::Denied));
    assert_eq!(gate(&set, "/no/such/path"), Err(GateError::Denied));
    // Once granted, absence is reported as such.
    let granted = allow(&["no-such-program-zz", "/no/such/path"]);
    assert_eq!(
        gate(&granted, "no-such-program-zz"),
        Err(GateError::NotFound)
    );
    assert_eq!(gate(&granted, "/no/such/path"), Err(GateError::NotFound));
}

#[test]
fn empty_and_relative_path_entries_are_not_searched() {
    let set = allow(&["*"]);
    // With cwd /bin, "." or "" on PATH would find `echo` there.
    assert_eq!(
        gate_in(&set, "echo", "/bin", ".:"),
        Err(GateError::NotFound)
    );
    assert_eq!(
        gate_in(&set, "echo", "/bin", "bin"),
        Err(GateError::NotFound)
    );
    assert!(gate_in(&set, "echo", "/bin", ".::/bin").is_ok());
}

#[test]
fn invalid_programs_are_rejected() {
    let set = allow(&["*"]);
    assert_eq!(gate(&set, ""), Err(GateError::Invalid));
    assert_eq!(gate(&set, "ec\0ho"), Err(GateError::Invalid));
}

#[test]
fn the_first_executable_on_path_is_the_one_authorized_and_run() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("echo");
    std::fs::write(&fake, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{PATH}", dir.path().display());
    // A name grant follows the name to whatever PATH finds first...
    let by_name = allow(&["echo"]);
    assert_eq!(
        gate_in(&by_name, "echo", "/", &path).as_deref(),
        Ok(canon(fake.to_str().unwrap()).as_str())
    );
    // ...a path grant does not: the shadowing binary is not the granted one.
    let by_path = allow(&["/bin/echo"]);
    assert_eq!(
        gate_in(&by_path, "echo", "/", &path),
        Err(GateError::Denied)
    );
    // A file that is not executable is skipped.
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        gate_in(&by_path, "echo", "/", &path).as_deref(),
        Ok(canon("/bin/echo").as_str())
    );
}

#[test]
fn the_canonical_path_is_what_gets_executed() {
    // A symlink granted by its own path, then re-pointed after the check: the program that was
    // authorized (the target at check time) is the one that runs.
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("tool");
    std::os::unix::fs::symlink("/bin/echo", &link).unwrap();
    let set = allow(&[link.to_str().unwrap()]);
    let resolved = authorize_spawn(&set, link.to_str().unwrap(), Path::new("/"), None).unwrap();
    assert_eq!(resolved.path(), Path::new(&canon("/bin/echo")));
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink("/bin/sh", &link).unwrap();
    assert_eq!(resolved.path(), Path::new(&canon("/bin/echo")));
}

#[test]
fn the_runtimes_own_executable_is_never_spawnable() {
    let me = std::env::current_exe().unwrap();
    let me = me.to_str().unwrap();
    // Granted by path, by pattern or by name: still refused, and never as plain `NotFound`.
    for grant in [me, "*"] {
        assert!(gate(&allow(&[grant]), me).is_err(), "granted by {grant}");
    }
    let name = Path::new(me).file_name().unwrap().to_str().unwrap();
    let dir = Path::new(me).parent().unwrap().to_str().unwrap();
    let by_name = gate_in(&allow(&[name]), name, "/", dir);
    assert!(by_name.is_err(), "granted by name");
    // A symlink to it is the same program.
    let tmp = tempfile::tempdir().unwrap();
    let link = tmp.path().join("innocent");
    std::os::unix::fs::symlink(me, &link).unwrap();
    assert!(gate(&allow(&["*"]), link.to_str().unwrap()).is_err());
}
