//! `Velyr.watchFs` on V8: events arrive quickly, the paths are authorized, closing releases the
//! watcher. Every test has a watchdog, so a missing event fails instead of hanging.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use velyr_engine_v8::V8Engine;
use velyr_io_fs::{fs_host_functions, install_fs};
use velyr_runtime_core::{RuntimeConfig, run_module_file};
use velyr_runtime_ops::{BOOTSTRAP_JS, OpRegistry, install_op_context};
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};
use velyr_runtime_resources::QuotaConfig;
use velyr_web_core::{install_url, url_host_functions};

const WATCHDOG: Duration = Duration::from_secs(20);
/// Lets the events of a test's setup pass before it starts watching.
const SETTLE: Duration = Duration::from_millis(200);

/// Runs `script` as an ES module with FsRead and FsWrite on `grant` (none when empty); the script
/// reports by throwing. `outside` runs on another thread once `<dir>/ready` exists.
fn run(script: &str, dir: &Path, grant: bool, outside: impl FnOnce() + Send + 'static) -> String {
    let entry_dir = tempfile::tempdir().unwrap();
    let entry = entry_dir.path().join("main.mjs");
    std::fs::write(&entry, script).unwrap();
    let mut builder = CapabilitySet::builder().allow(Capability::ImportLocal(PathPattern::new(
        entry_dir.path().to_str().unwrap(),
    )));
    if grant {
        let pattern = || PathPattern::new(dir.to_str().unwrap());
        builder = builder
            .allow(Capability::FsRead(pattern()))
            .allow(Capability::FsWrite(pattern()));
    }
    let permissions = builder.build();
    let granted = permissions.clone();
    let mut functions = OpRegistry::builtin().host_functions().to_vec();
    functions.extend(fs_host_functions());
    functions.extend(url_host_functions());
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    std::thread::spawn(move || {
        std::thread::sleep(WATCHDOG);
        stop.cancel();
    });
    let ready = dir.join("ready");
    std::thread::spawn(move || {
        for _ in 0..400 {
            if ready.exists() {
                outside();
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    });
    let result = run_module_file(
        V8Engine::new(),
        RuntimeConfig {
            platform_init: Some(V8Engine::init_platform),
            capabilities: Some(granted),
            leak_check: true,
            shutdown: Some(shutdown),
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

fn canonical_tmp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}

/// Watches `dir`, runs `trigger`, waits for an event of kind `want` on `target` (FSEvents may
/// report other kinds for the same record first) and reports `got:<kind>:<elapsed ms>`.
fn wait_for(dir: &Path, target: &str, want: &str, trigger: &str) -> String {
    format!(
        r#"const dir = {dir:?};
        const watcher = Velyr.watchFs([dir]);
        {trigger}
        const started = Date.now();
        for await (const event of watcher) {{
          if (event.kind === "{want}" && event.paths.some((p) => p.endsWith("/{target}"))) {{
            throw new Error("got:" + event.kind + ":" + (Date.now() - started));
          }}
        }}"#
    )
}

fn parse(message: &str) -> (String, u64) {
    let text = message.strip_prefix("Uncaught got:").expect(message);
    let (kind, ms) = text.split_once(':').expect(message);
    (kind.to_owned(), ms.parse().expect(message))
}

#[test]
fn a_new_file_is_reported_as_create_within_500ms() {
    let (_guard, dir) = canonical_tmp();
    let script = wait_for(
        &dir,
        "created.txt",
        "create",
        r#"await Velyr.writeTextFile(dir + "/created.txt", "x");"#,
    );
    let (kind, ms) = parse(&run(&script, &dir, true, || {}));
    assert_eq!(kind, "create");
    assert!(ms < 500, "{ms} ms");
}

#[test]
fn an_edited_file_is_reported_as_modify_within_500ms() {
    let (_guard, dir) = canonical_tmp();
    let file = dir.join("edited.txt");
    std::fs::write(&file, "one").unwrap();
    // The events of the setup above must not be taken for the edit.
    std::thread::sleep(SETTLE);
    let script = wait_for(
        &dir,
        "edited.txt",
        "modify",
        r#"await Velyr.writeTextFile(dir + "/edited.txt", "two");"#,
    );
    let (kind, ms) = parse(&run(&script, &dir, true, || {}));
    assert_eq!(kind, "modify");
    assert!(ms < 500, "{ms} ms");
}

#[test]
fn a_deleted_file_is_reported_as_remove_within_500ms() {
    let (_guard, dir) = canonical_tmp();
    let file = dir.join("doomed.txt");
    std::fs::write(&file, "bye").unwrap();
    std::thread::sleep(SETTLE);
    // `ready` tells the helper thread the watcher is open; it then deletes the file.
    let script = wait_for(
        &dir,
        "doomed.txt",
        "remove",
        r#"await Velyr.writeTextFile(dir + "/ready", "1");"#,
    );
    let (kind, ms) = parse(&run(&script, &dir, true, move || {
        std::fs::remove_file(file).unwrap();
    }));
    assert_eq!(kind, "remove");
    assert!(ms < 500, "{ms} ms");
}

#[test]
fn the_rust_watcher_classifies_by_what_the_file_system_says() {
    let (_guard, dir) = canonical_tmp();
    let file = dir.join("e.txt");
    std::fs::write(&file, "one").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let watcher = velyr_io_fs::FsWatcher::open(std::slice::from_ref(&dir), false).unwrap();
    let kinds = |watcher: &velyr_io_fs::FsWatcher| {
        runtime.block_on(async {
            let mut kinds = Vec::new();
            while let Ok(Some(event)) =
                tokio::time::timeout(Duration::from_millis(800), watcher.next()).await
            {
                kinds.push(event.kind);
            }
            kinds
        })
    };
    std::fs::write(&file, "two").unwrap();
    let edit = kinds(&watcher);
    assert!(
        !edit.is_empty() && edit.iter().all(|k| *k == "modify"),
        "{edit:?}"
    );
    std::fs::remove_file(&file).unwrap();
    let delete = kinds(&watcher);
    assert!(
        !delete.is_empty() && delete.iter().all(|k| *k == "remove"),
        "{delete:?}"
    );
}

#[test]
fn watching_without_fs_read_is_denied() {
    let (_guard, dir) = canonical_tmp();
    let script = format!(
        r#"try {{ Velyr.watchFs([{dir:?}]); }} catch (e) {{ throw new Error("code:" + e.code); }}
        throw new Error("not-denied");"#
    );
    let message = run(&script, &dir, false, || {});
    assert!(
        message.contains("code:VELYR_PERMISSION_DENIED"),
        "{message}"
    );
}

#[test]
fn one_denied_path_denies_the_whole_watch() {
    let (_guard, dir) = canonical_tmp();
    let other = tempfile::tempdir().unwrap();
    let script = format!(
        r#"try {{ Velyr.watchFs([{dir:?}, {:?}]); }} catch (e) {{ throw new Error("code:" + e.code); }}
        throw new Error("not-denied");"#,
        other.path().canonicalize().unwrap()
    );
    let message = run(&script, &dir, true, || {});
    assert!(
        message.contains("code:VELYR_PERMISSION_DENIED"),
        "{message}"
    );
}

#[test]
fn leaving_the_loop_closes_the_watcher() {
    let (_guard, dir) = canonical_tmp();
    // `leak_check` fails the run when the resource is still open at the end.
    let script = format!(
        r#"const watcher = Velyr.watchFs([{dir:?}]);
        await Velyr.writeTextFile({dir:?} + "/a.txt", "x");
        for await (const event of watcher) {{ break; }}"#
    );
    assert_eq!(run(&script, &dir, true, || {}), "ok");
}
