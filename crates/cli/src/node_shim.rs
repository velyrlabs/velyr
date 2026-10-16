//! The `node` shim (`velyr run --node-shim`) and what `velyr x` hands it.
//!
//! The shim never grants authority: it runs with the flags in `VELYR_NODE_SHIM_FLAGS` (the raw
//! permission arguments of the process that spawned it, joined by `\x1f`) and with none when the
//! variable is absent. Lifecycle scripts set it to `-A` themselves (compat-npm).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Parser;
use velyr_compat_npm::scripts::SHIM_FLAGS_ENV;

use crate::permissions::Permissions;
use crate::run::{self, EngineKind, Worker};

const SEPARATOR: char = '\x1f';

#[derive(Parser)]
struct Inherited {
    #[command(flatten)]
    permissions: Permissions,
}

/// The permissions the caller of the shim passed on, or none.
pub fn inherited(raw: Option<&str>) -> Result<Permissions, String> {
    let args = raw
        .unwrap_or_default()
        .split(SEPARATOR)
        .filter(|a| !a.is_empty());
    Inherited::try_parse_from(std::iter::once("velyr").chain(args))
        .map(|i| i.permissions)
        .map_err(|e| format!("error: invalid {SHIM_FLAGS_ENV}: {e}"))
}

/// The permission arguments written between `x` and the bin name on the command line.
pub fn raw_flags_of_x(argv: &[String], trailing: usize) -> Vec<String> {
    let after_x = argv.iter().skip(1).skip_while(|a| *a != "x").skip(1);
    let after_x: Vec<&String> = after_x.collect();
    let head = after_x.len().saturating_sub(trailing + 1);
    after_x[..head].iter().map(|a| (*a).clone()).collect()
}

fn js(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/// Appends `args` to `process.argv`, after the script the runtime put there.
pub fn argv_script(args: &[String]) -> String {
    let list: Vec<String> = args.iter().map(|a| js(a)).collect();
    format!("process.argv.push({});", list.join(","))
}

/// Puts `dirs` first on the script's `PATH`. The write lands in the isolate's env overlay and
/// needs EnvWrite; without it the child just does not find the shim by name.
pub fn path_script(dirs: &[PathBuf]) -> String {
    let current = std::env::var_os("PATH").unwrap_or_default();
    let all = dirs.iter().cloned().chain(std::env::split_paths(&current));
    let path = std::env::join_paths(all).unwrap_or(current);
    format!(
        "try {{ process.env.PATH = {}; }} catch (_) {{}}",
        js(&path.to_string_lossy())
    )
}

/// Records the flags this process runs with: io-process hands them to every child it spawns
/// (overwriting any `VELYR_NODE_SHIM_FLAGS` the script asked for), so a child never outranks us.
pub fn pin_flags(raw: &[String]) {
    velyr_io_process::set_shim_flags(raw.join(&SEPARATOR.to_string()));
}

/// `velyr run [--node-shim] <file> [-- args]`: runs `file` with `args` in `process.argv`.
pub fn run_with_args(
    file: &Path,
    args: &[String],
    flags: Permissions,
    engine: EngineKind,
) -> ExitCode {
    let result = run::prepare(file, flags).and_then(|prepared| {
        let script = (!args.is_empty()).then(|| argv_script(args));
        run::run_prepared(file, &prepared, script, Worker::default(), engine)
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// `velyr run --node-shim`: the flags come from the environment, never from the command line.
pub fn run_shim(file: &Path, args: &[String], engine: EngineKind) -> ExitCode {
    let raw = std::env::var(SHIM_FLAGS_ENV).ok();
    match inherited(raw.as_deref()) {
        Ok(flags) => {
            if let Some(raw) = &raw {
                velyr_io_process::set_shim_flags(raw.clone());
            }
            run_with_args(file, args, flags, engine)
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn flags_are_what_sits_between_x_and_the_bin() {
        let argv = v(&["velyr", "x", "--allow-read=.", "tool", "-A", "z"]);
        assert_eq!(raw_flags_of_x(&argv, 2), v(&["--allow-read=."]));
        let argv = v(&["velyr", "--log-format", "json", "x", "tool"]);
        assert!(raw_flags_of_x(&argv, 0).is_empty());
    }

    #[test]
    fn missing_or_empty_env_parses_to_the_default_flags() {
        let none = inherited(None).unwrap();
        assert!(!none.allow_all);
        assert!(inherited(Some("")).is_ok());
        assert!(inherited(Some("-A")).unwrap().allow_all);
        assert!(inherited(Some("--no-such-flag")).is_err());
    }

    #[test]
    fn both_crates_agree_on_the_variable_name() {
        assert_eq!(velyr_io_process::SHIM_FLAGS_ENV, SHIM_FLAGS_ENV);
    }

    #[test]
    fn argv_script_escapes_arguments() {
        assert_eq!(
            argv_script(&v(&["a\"b", "c"])),
            r#"process.argv.push("a\"b","c");"#
        );
    }
}
