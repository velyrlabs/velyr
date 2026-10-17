//! Builds the list of exported Node-API symbols and the linker argument that exports them from
//! an executable, so `.node` addons loaded at run time resolve `napi_*` in the host binary.
//!
//! The list is every `fn napi_*` / `fn node_api_*` in `src/` (only exported FFI functions use
//! those prefixes). The argument is applied to this crate's tests and published to direct
//! dependents as `DEP_VELYR_NAPI_LINK_ARG` (`crates/cli/build.rs` applies it to `velyr`).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=src");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let mut names = Vec::new();
    collect_exports(Path::new("src"), &mut names);
    names.sort();
    names.dedup();

    if let Some(arg) = write_export_list(&out_dir, &target_os, &names) {
        println!("cargo:link_arg={arg}");
        println!("cargo:rustc-link-arg-tests={arg}");
    }
    let target = env::var("TARGET").expect("cargo sets TARGET");
    println!("cargo:rustc-env=VELYR_NAPI_TARGET={target}");
}

/// Writes the platform export list and returns the linker argument that applies it.
// shortcut: macOS is verified; the Linux dynamic list is untested (napi-scope.md section 5.3),
// Windows needs a `.def` file. Add both when CI covers those targets.
fn write_export_list(out_dir: &Path, target_os: &str, names: &[String]) -> Option<String> {
    let (file, body, flag) = match target_os {
        "macos" | "ios" => (
            "napi-exports.txt",
            names.iter().map(|n| format!("_{n}\n")).collect::<String>(),
            "-Wl,-exported_symbols_list,",
        ),
        "linux" | "android" | "freebsd" => (
            "napi-exports.list",
            format!(
                "{{\n{}}};\n",
                names
                    .iter()
                    .map(|n| format!("  {n};\n"))
                    .collect::<String>()
            ),
            "-Wl,--dynamic-list=",
        ),
        _ => return None,
    };
    let path = out_dir.join(file);
    fs::write(&path, body).expect("write the export list");
    Some(format!("{flag}{}", path.display()))
}

fn collect_exports(dir: &Path, names: &mut Vec<String>) {
    for entry in fs::read_dir(dir).expect("read src").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_exports(&path, names);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = fs::read_to_string(&path).expect("read source");
            names.extend(text.lines().filter_map(exported_name));
        }
    }
}

fn exported_name(line: &str) -> Option<String> {
    let rest = &line[line.find("fn ")? + 3..];
    if !(rest.starts_with("napi_") || rest.starts_with("node_api_")) {
        return None;
    }
    let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))?;
    Some(rest[..end].to_owned())
}
