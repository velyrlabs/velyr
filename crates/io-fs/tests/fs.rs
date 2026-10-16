//! `Velyr.*` file APIs on V8, with capability sets built directly.

use std::path::Path;

use velyr_engine_v8::V8Engine;
use velyr_io_fs::{fs_host_functions, install_fs};
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{install_url, url_host_functions};

/// Runs `script` as an ES module with the given fs grants (the entry import is always granted);
/// the script reports by throwing, which comes back as the message.
fn run(script: &str, read: &[&Path], write: &[&Path]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let entry = dir.path().join("main.mjs");
    std::fs::write(&entry, script).unwrap();
    let mut builder = CapabilitySet::builder().allow(Capability::ImportLocal(PathPattern::new(
        dir.path().to_str().unwrap(),
    )));
    for path in read {
        builder = builder.allow(Capability::FsRead(PathPattern::new(path.to_str().unwrap())));
    }
    for path in write {
        builder = builder.allow(Capability::FsWrite(PathPattern::new(
            path.to_str().unwrap(),
        )));
    }
    let permissions = builder.build();
    let granted = permissions.clone();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(fs_host_functions());
    functions.extend(url_host_functions());
    let result = run_module_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            capabilities: Some(granted),
            ..RuntimeConfig::default()
        },
        functions,
        BOOTSTRAP_JS,
        move |isolate, sender, _| {
            install_op_context(isolate, 0, sender, permissions, QuotaConfig::default());
            install_url(isolate).unwrap();
            install_fs(isolate).unwrap();
        },
        entry,
    );
    match result {
        Err(err) => err.to_string(),
        Ok(()) => "ok".into(),
    }
}

fn report(expr: &str) -> String {
    format!(r#"const v = await ({expr}); throw new Error("got:" + v);"#)
}

fn denied(expr: &str) -> String {
    format!(
        r#"try {{ await ({expr}); }} catch (e) {{ throw new Error("code:" + e.code); }}
        throw new Error("not-denied");"#
    )
}

fn canonical_tmp() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}

#[test]
fn read_without_grant_rejects_permission_denied() {
    let (_guard, dir) = canonical_tmp();
    std::fs::write(dir.join("fixture.txt"), "hello").unwrap();
    let script = denied(&format!(
        "Velyr.readTextFile({:?})",
        dir.join("fixture.txt")
    ));
    let message = run(&script, &[], &[]);
    assert!(
        message.contains("code:VELYR_PERMISSION_DENIED"),
        "{message}"
    );
}

#[test]
fn granted_read_returns_text_and_bytes() {
    let (_guard, dir) = canonical_tmp();
    let file = dir.join("fixture.txt");
    std::fs::write(&file, "héllo").unwrap();
    let text = run(
        &report(&format!("Velyr.readTextFile({file:?})")),
        &[&dir],
        &[],
    );
    assert_eq!(text, "Uncaught got:héllo");
    let bytes = run(
        &report(&format!(
            "Velyr.readFile({file:?}).then((b) => b.constructor.name + [...b])"
        )),
        &[&dir],
        &[],
    );
    assert_eq!(bytes, "Uncaught got:Uint8Array104,195,169,108,108,111");
}

#[test]
fn relative_path_resolves_against_cwd() {
    let cwd = std::env::current_dir().unwrap();
    let message = run(
        &report("Velyr.readTextFile('Cargo.toml').then((t) => t.slice(0, 9))"),
        &[&cwd],
        &[],
    );
    assert_eq!(message, "Uncaught got:[package]");
}

#[test]
fn file_url_is_accepted_and_other_schemes_are_rejected() {
    let (_guard, dir) = canonical_tmp();
    std::fs::write(dir.join("a b.txt"), "url").unwrap();
    let url = format!("new URL('file://{}/a%20b.txt')", dir.display());
    let message = run(&report(&format!("Velyr.readTextFile({url})")), &[&dir], &[]);
    assert_eq!(message, "Uncaught got:url");
    let message = run(
        r#"try { await Velyr.readTextFile(new URL('http://x.com/a')); } catch (e) { throw new Error(e.name); }"#,
        &[&dir],
        &[],
    );
    assert_eq!(message, "Uncaught TypeError");
}

#[test]
fn missing_file_rejects_not_found() {
    let (_guard, dir) = canonical_tmp();
    let script = denied(&format!("Velyr.readFile({:?})", dir.join("nope.txt")));
    assert!(run(&script, &[&dir], &[]).contains("code:VELYR_NOT_FOUND"));
}

#[test]
fn write_roundtrips_text_and_every_byte_value() {
    let (_guard, dir) = canonical_tmp();
    let script = format!(
        r#"await Velyr.writeTextFile({text:?}, "olá");
        await Velyr.writeFile({bin:?}, Uint8Array.from({{ length: 256 }}, (_, i) => i));
        const t = await Velyr.readTextFile({text:?});
        const b = await Velyr.readFile({bin:?});
        throw new Error([t, b.length, b.every((x, i) => x === i)].join("|"));"#,
        text = dir.join("t.txt"),
        bin = dir.join("b.bin"),
    );
    assert_eq!(run(&script, &[&dir], &[&dir]), "Uncaught olá|256|true");
    assert_eq!(std::fs::read(dir.join("t.txt")).unwrap(), "olá".as_bytes());
}

#[test]
fn write_file_sends_bytes_without_building_a_binary_string() {
    let (_guard, dir) = canonical_tmp();
    let script = format!(
        r#"String.fromCharCode = () => {{ throw new Error("binary string built"); }};
        const all = Uint8Array.from({{ length: 256 }}, (_, i) => i);
        await Velyr.writeFile({full:?}, all);
        await Velyr.writeFile({win:?}, all.subarray(250));
        await Velyr.writeFile({buf:?}, all.buffer.slice(0, 3));
        await Velyr.writeFile({view:?}, new DataView(all.buffer, 1, 2));
        try {{ await Velyr.writeFile({full:?}, "text"); }} catch (e) {{ throw new Error(e.name); }}"#,
        full = dir.join("full.bin"),
        win = dir.join("win.bin"),
        buf = dir.join("buf.bin"),
        view = dir.join("view.bin"),
    );
    assert_eq!(run(&script, &[&dir], &[&dir]), "Uncaught TypeError");
    let all: Vec<u8> = (0..=255).collect();
    assert_eq!(std::fs::read(dir.join("full.bin")).unwrap(), all);
    assert_eq!(std::fs::read(dir.join("win.bin")).unwrap(), &all[250..]);
    assert_eq!(std::fs::read(dir.join("buf.bin")).unwrap(), [0, 1, 2]);
    assert_eq!(std::fs::read(dir.join("view.bin")).unwrap(), [1, 2]);
}

#[test]
fn write_outside_allow_write_is_denied_and_creates_nothing() {
    let (_guard, dir) = canonical_tmp();
    let target = dir.join("out.txt");
    let script = denied(&format!("Velyr.writeTextFile({target:?}, 'x')"));
    assert!(run(&script, &[&dir], &[]).contains("code:VELYR_PERMISSION_DENIED"));
    assert!(!target.exists());
}

#[test]
fn write_into_missing_parent_rejects_not_found() {
    let (_guard, dir) = canonical_tmp();
    let script = denied(&format!(
        "Velyr.writeTextFile({:?}, 'x')",
        dir.join("no/dir.txt")
    ));
    assert!(run(&script, &[], &[&dir]).contains("code:VELYR_NOT_FOUND"));
}

#[test]
fn stat_reports_size_kind_and_mtime() {
    let (_guard, dir) = canonical_tmp();
    let file = dir.join("s.txt");
    std::fs::write(&file, "12345").unwrap();
    let script = report(&format!(
        "Promise.all([Velyr.stat({file:?}), Velyr.stat({dir:?})]).then(([f, d]) => [f.size, f.isFile, f.isDirectory, d.isDirectory, f.mtime instanceof Date].join())"
    ));
    assert_eq!(
        run(&script, &[&dir], &[]),
        "Uncaught got:5,true,false,true,true"
    );
    let denied_stat = denied(&format!("Velyr.stat({file:?})"));
    assert!(run(&denied_stat, &[], &[]).contains("code:VELYR_PERMISSION_DENIED"));
}

#[cfg(unix)]
#[test]
fn symlink_out_of_the_granted_dir_is_denied() {
    let (_guard, allowed) = canonical_tmp();
    let (_other, outside) = canonical_tmp();
    std::fs::write(outside.join("secret.txt"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), allowed.join("link.txt")).unwrap();
    let script = denied(&format!(
        "Velyr.readTextFile({:?})",
        allowed.join("link.txt")
    ));
    assert!(run(&script, &[&allowed], &[]).contains("code:VELYR_PERMISSION_DENIED"));
}

#[cfg(unix)]
#[test]
fn write_through_dangling_symlink_is_refused() {
    let (_guard, allowed) = canonical_tmp();
    let (_other, outside) = canonical_tmp();
    let target = outside.join("created.txt");
    std::os::unix::fs::symlink(&target, allowed.join("dangling.txt")).unwrap();
    let script = denied(&format!(
        "Velyr.writeTextFile({:?}, 'x')",
        allowed.join("dangling.txt")
    ));
    let message = run(&script, &[], &[&allowed]);
    assert!(
        message.contains("code:VELYR_PERMISSION_DENIED"),
        "{message}"
    );
    assert!(!target.exists());
}

#[test]
fn read_file_returns_exact_bytes_for_binary_content() {
    let (_guard, dir) = canonical_tmp();
    let file = dir.join("blob.bin");
    std::fs::write(&file, [0u8, 0x80, 0xff, 0xc3, 0x28, 7]).unwrap();
    let message = run(
        &report(&format!(
            "Velyr.readFile({file:?}).then((b) => b.constructor.name + [...b])"
        )),
        &[&dir],
        &[],
    );
    assert_eq!(message, "Uncaught got:Uint8Array0,128,255,195,40,7");
}

#[test]
fn read_text_file_of_directory_rejects_with_code() {
    let (_guard, dir) = canonical_tmp();
    let script = format!(
        r#"try {{ await Velyr.readTextFile({dir:?}); }}
        catch (e) {{ throw new Error((e instanceof Error) + ":" + e.code); }}
        throw new Error("resolved");"#
    );
    let message = run(&script, &[&dir], &[]);
    assert_eq!(message, "Uncaught true:VELYR_INVALID_DATA");
}
