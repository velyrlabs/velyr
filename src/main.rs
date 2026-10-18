use std::fs;
use std::path::Path;

fn step<T: std::fmt::Debug>(what: &str, r: std::io::Result<T>) -> Option<T> {
    match r {
        Ok(v) => { println!("OK   {what}: {v:?}"); Some(v) }
        Err(e) => { println!("ERR  {what}: {e} (raw {:?})", e.raw_os_error()); None }
    }
}

fn main() {
    let base = std::env::temp_dir().join(format!("probe-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    let dir = step("canonicalize", fs::canonicalize(&base)).unwrap();
    step("create_dir_all(verbatim)", fs::create_dir_all(&dir));
    step("canonicalize(verbatim)", fs::canonicalize(&dir));
    let target = dir.join("path__posix.mjs");
    let staging = dir.join(".path__posix.mjs.1");
    step("write staging", fs::write(&staging, "x"));
    step("rename staging->target (new)", fs::rename(&staging, &target));
    step("write staging 2", fs::write(&staging, "y"));
    step("rename staging->target (replace)", fs::rename(&staging, &target));
    step("read target", fs::read(&target).map(|b| b.len()));
    step("create_dir w", fs::create_dir_all(dir.join("w")));
    let mixed = format!("{}\\w/p.txt", dir.display());
    step("write verbatim+slash", fs::write(Path::new(&mixed), "z"));
    let plain = format!("{}\\w/p.txt", base.display());
    step("write plain+slash", fs::write(Path::new(&plain), "z"));
}
