//! `velyr run --watch[=paths]`: the script restarts when a module of its import graph, or one of
//! the given paths, changes. Every wait has a deadline and the supervisor is killed at the end.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(20);

struct Supervisor {
    child: Child,
    lines: Receiver<String>,
}

impl Supervisor {
    fn start(dir: &Path, watch: Option<&str>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_velyr"));
        command.current_dir(dir).arg("run");
        match watch {
            Some(paths) => command.arg(format!("--watch={paths}")),
            None => command.arg("--watch"),
        };
        let mut child = command
            .arg("main.mjs")
            .env("HOME", dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { child, lines }
    }

    fn wait_for(&self, want: &str) {
        let end = Instant::now() + DEADLINE;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line == want => return,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    panic!("no `{want}` line within {DEADLINE:?}")
                }
            }
        }
    }

    /// Lines that arrive within `window`.
    fn quiet_for(&self, window: Duration) -> Vec<String> {
        let end = Instant::now() + window;
        let mut seen = Vec::new();
        while let Ok(line) = self
            .lines
            .recv_timeout(end.saturating_duration_since(Instant::now()))
        {
            seen.push(line);
        }
        seen
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(
        root.join("main.mjs"),
        "import { value } from './lib/dep.mjs';\nconsole.log('value=' + value);\n",
    )
    .unwrap();
    std::fs::create_dir(root.join("lib")).unwrap();
    std::fs::write(root.join("lib/dep.mjs"), "export const value = 'v1';\n").unwrap();
    (dir, root)
}

#[test]
fn editing_an_imported_dependency_restarts_the_script() {
    let (_guard, root) = project();
    let supervisor = Supervisor::start(&root, None);
    supervisor.wait_for("value=v1");
    // Past the graph report poll, so the dependency is known when it changes.
    std::thread::sleep(Duration::from_millis(600));
    std::fs::write(root.join("lib/dep.mjs"), "export const value = 'v2';\n").unwrap();
    supervisor.wait_for("value=v2");
}

#[test]
fn editing_the_entry_restarts_the_script() {
    let (_guard, root) = project();
    let supervisor = Supervisor::start(&root, None);
    supervisor.wait_for("value=v1");
    std::thread::sleep(Duration::from_millis(600));
    std::fs::write(
        root.join("main.mjs"),
        "import { value } from './lib/dep.mjs';\nconsole.log('again=' + value);\n",
    )
    .unwrap();
    supervisor.wait_for("again=v1");
}

#[test]
fn a_file_outside_the_graph_does_not_restart_it() {
    let (_guard, root) = project();
    let supervisor = Supervisor::start(&root, None);
    supervisor.wait_for("value=v1");
    std::thread::sleep(Duration::from_millis(600));
    std::fs::write(root.join("lib/notes.txt"), "not a module").unwrap();
    let seen = supervisor.quiet_for(Duration::from_millis(1500));
    assert!(seen.is_empty(), "{seen:?}");
}

#[test]
fn a_given_path_restarts_the_script_when_it_changes() {
    let (_guard, root) = project();
    std::fs::create_dir(root.join("data")).unwrap();
    let supervisor = Supervisor::start(&root, Some("data"));
    supervisor.wait_for("value=v1");
    std::thread::sleep(Duration::from_millis(600));
    std::fs::write(root.join("data/config.json"), "{}").unwrap();
    supervisor.wait_for("value=v1");
}
