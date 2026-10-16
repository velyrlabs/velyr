//! VELYR-179: abuse cases for the symlink authorization of `node:fs` (VELYR-77). Each script
//! gets `DIR` (the grant) and `OUT` (a canonical directory outside it, holding `secret.txt` and
//! `victim/keep`); the Rust side checks afterwards that nothing under `OUT` changed.
#![cfg(unix)]

mod common;

use common::script::{CHECKS, assert_ok, run_with};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

const PRELUDE: &str = r#"
const node = globalThis[Symbol.for('velyr.node')];
const fs = node.require('fs');
const fsp = node.require('fs/promises');
const path = node.require('path');
const j = (name) => path.join(DIR, name);
const o = (name) => path.join(OUT, name);
const denied = (error, label) => {
  eq(error && error.code, 'ERR_ACCESS_DENIED', label);
  eq(error.cause && error.cause.code, 'VELYR_PERMISSION_DENIED', label + ' cause');
};
const deniedSync = (fn, label) => { try { fn(); } catch (e) { denied(e, label); return; } throw new Error('allowed: ' + label); };
const deniedAsync = async (promise, label) => { try { await promise; } catch (e) { denied(e, label); return; } throw new Error('allowed: ' + label); };
const settle = (promise) => promise.then(() => null, (e) => e);
// Swaps directory `d` for a link to OUT; `d` keeps a decoy so the first check passes.
const swapForOut = (name) => { fs.renameSync(j(name), j(name + '.old')); fs.symlinkSync(OUT, j(name)); };
"#;

struct Sandbox {
    dir: TempDir,
    out: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let sandbox = Self {
            dir: tempfile::tempdir().unwrap(),
            out: tempfile::tempdir().unwrap(),
        };
        std::fs::write(sandbox.out_path().join("secret.txt"), "secret").unwrap();
        std::fs::create_dir(sandbox.out_path().join("victim")).unwrap();
        std::fs::write(sandbox.out_path().join("victim/keep"), "keep").unwrap();
        sandbox
    }

    fn dir_path(&self) -> PathBuf {
        self.dir.path().canonicalize().unwrap()
    }

    fn out_path(&self) -> PathBuf {
        self.out.path().canonicalize().unwrap()
    }

    /// Runs `body` with read and write on `DIR`, minus `denied` (paths relative to `DIR`).
    fn run(&self, body: &str, denied: &[&str]) -> String {
        let dir = self.dir_path();
        let mut builder = CapabilitySet::builder()
            .allow(Capability::FsRead(pattern(&dir)))
            .allow(Capability::FsWrite(pattern(&dir)));
        for name in denied {
            builder = builder
                .deny(Capability::FsRead(pattern(&dir.join(name))))
                .deny(Capability::FsWrite(pattern(&dir.join(name))));
        }
        let script = format!(
            "{CHECKS}const DIR = {:?}; const OUT = {:?};{PRELUDE}{body}",
            dir.to_str().unwrap(),
            self.out_path().to_str().unwrap()
        );
        run_with(&script, builder.build(), "")
    }

    /// `OUT` holds exactly what `new` put there, unchanged.
    fn assert_out_untouched(&self) {
        let out = self.out_path();
        let mut names: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["secret.txt", "victim"],
            "OUT gained or lost entries"
        );
        assert_eq!(read(&out.join("secret.txt")), "secret");
        assert_eq!(read(&out.join("victim/keep")), "keep");
        assert!(
            !out.join("secret.txt")
                .symlink_metadata()
                .unwrap()
                .is_symlink()
        );
    }
}

fn pattern(path: &Path) -> PathPattern {
    PathPattern::new(path.to_str().unwrap())
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Attack: an async call is authorized when it is issued, but its job runs after the current
/// JS turn. Swapping a parent directory for a link in between must not redirect the job.
#[test]
fn toctou_parent_swap_cannot_redirect_async_jobs() {
    let sandbox = Sandbox::new();
    assert_ok(sandbox.run(
        r#"const dirs = ['w', 'r', 'rm', 't', 'm', 'c', 'u', 'un', 'ch', 'rd', 'st', 'rn'];
        for (const d of dirs) fs.mkdirSync(j(d));
        for (const d of ['r', 'u', 'un', 'ch', 'st', 'rn']) fs.writeFileSync(j(d + '/secret.txt'), 'decoy');
        fs.mkdirSync(j('rm/victim'));
        fs.writeFileSync(j('src'), 'payload');
        const pending = {
          unlink: fsp.unlink(j('un/secret.txt')),
          chmod: fsp.chmod(j('ch/secret.txt'), 0o777),
          readdir: fsp.readdir(j('rd')).then((names) => { if (names.includes('secret.txt')) throw new Error('listed OUT'); throw new Error('decoy'); }).catch((e) => { if (e.message === 'listed OUT') return; throw e; }),
          stat: fsp.stat(j('st/secret.txt')).then((st) => { if (st.size !== 6) throw new Error('decoy'); }),
          rename: fsp.rename(j('rn/secret.txt'), j('stolen')),
          writeFile: fsp.writeFile(j('w/planted'), 'pwned'),
          readFile: fsp.readFile(j('r/secret.txt'), 'utf8'),
          rm: fsp.rm(j('rm/victim'), { recursive: true }),
          mkdtemp: fsp.mkdtemp(j('t/tmp-')),
          mkdir: fsp.mkdir(j('m/made')),
          copyFile: fsp.copyFile(j('src'), j('c/copied')),
          utimes: fsp.utimes(j('u/secret.txt'), 1, 1),
        };
        for (const d of dirs) swapForOut(d);
        const escaped = [];
        for (const [name, promise] of Object.entries(pending)) {
          if ((await settle(promise)) === null) escaped.push(name);
        }
        same(escaped, [], 'ran through the swapped parent');"#,
        &[],
    ));
    sandbox.assert_out_untouched();
}

/// Attack: an async write to a new name, then a link planted at that name before the job opens it.
#[test]
fn toctou_link_planted_at_a_new_name_is_not_followed() {
    let sandbox = Sandbox::new();
    assert_ok(sandbox.run(
        r#"const write = fsp.writeFile(j('fresh'), 'pwned');
        const append = fsp.appendFile(j('fresh2'), 'pwned');
        fs.symlinkSync(o('planted'), j('fresh'));
        fs.symlinkSync(o('secret.txt'), j('fresh2'));
        eq((await settle(write)) === null, false, 'writeFile followed the planted link');
        eq((await settle(append)) === null, false, 'appendFile followed the planted link');"#,
        &[],
    ));
    sandbox.assert_out_untouched();
}

/// Attack: an entry authorized through the parent-directory fallback (it was a link) is swapped
/// before the job runs. Entry operations never follow the final component, so the job still
/// only touches the name inside the grant.
#[test]
fn toctou_swapping_the_fallback_link_stays_inside_the_grant() {
    let sandbox = Sandbox::new();
    assert_ok(sandbox.run(
        r#"fs.symlinkSync(o('secret.txt'), j('l1'));
        fs.symlinkSync(o('secret.txt'), j('l2'));
        fs.symlinkSync(o('secret.txt'), j('l3'));
        const rm = fsp.rm(j('l1'), { recursive: true, force: true });
        const unlink = fsp.unlink(j('l2'));
        const rename = fsp.rename(j('l3'), j('moved'));
        fs.unlinkSync(j('l1')); fs.symlinkSync(o('victim'), j('l1'));
        fs.unlinkSync(j('l2')); fs.symlinkSync(o('victim'), j('l2'));
        fs.unlinkSync(j('l3')); fs.mkdirSync(j('l3'));
        eq(await settle(rm), null, 'rm');
        eq(await settle(unlink), null, 'unlink');
        eq(await settle(rename), null, 'rename');
        eq(fs.existsSync(j('l1')) || fs.existsSync(j('l2')), false);
        eq(fs.statSync(j('moved')).isDirectory(), true);"#,
        &[],
    ));
    sandbox.assert_out_untouched();
}

/// Attack: move a link (or anything) out of the grant with `rename`, or rename over a link.
#[test]
fn rename_cannot_move_a_link_out_of_the_grant() {
    let sandbox = Sandbox::new();
    assert_ok(sandbox.run(
        r#"fs.symlinkSync(o('secret.txt'), j('l'));
        deniedSync(() => fs.renameSync(j('l'), o('l')), 'rename link out');
        await deniedAsync(fsp.rename(j('l'), o('l')), 'rename link out async');
        deniedSync(() => fs.renameSync(o('secret.txt'), j('stolen')), 'rename from outside');
        fs.mkdirSync(j('sub'));
        fs.renameSync(j('l'), j('sub/l'));
        deniedSync(() => fs.readFileSync(j('sub/l')), 'read moved link');
        fs.writeFileSync(j('plain'), 'replacement');
        fs.renameSync(j('plain'), j('sub/l'));
        eq(fs.readFileSync(j('sub/l'), 'utf8'), 'replacement');
        // a link to a directory inside the grant: renaming into it must not go through it
        fs.symlinkSync(OUT, j('outdir'));
        fs.writeFileSync(j('f'), 'x');
        deniedSync(() => fs.renameSync(j('f'), j('outdir/f')), 'rename into a linked dir');"#,
        &[],
    ));
    sandbox.assert_out_untouched();
}

/// Attack: plant a link pointing out, then use it from APIs that follow paths.
#[test]
fn a_planted_link_cannot_be_used_by_other_apis() {
    let sandbox = Sandbox::new();
    assert_ok(sandbox.run(
        r#"fs.symlinkSync(o('secret.txt'), j('file-link'));
        fs.symlinkSync(OUT, j('dir-link'));
        const reads = {
          readFile: [j('file-link')], stat: [j('file-link')], access: [j('file-link')],
          realpath: [j('file-link')], readdir: [j('dir-link')], open: [j('file-link'), 'r'],
          copyFile: [j('file-link'), j('copy')], opendir: [j('dir-link')],
        };
        const writes = {
          writeFile: [j('file-link'), 'x'], appendFile: [j('file-link'), 'x'], chmod: [j('file-link'), 0o777],
          utimes: [j('file-link'), 1, 1], mkdir: [j('dir-link/new')], mkdtemp: [j('dir-link/t-')],
          rm: [j('dir-link/victim'), { recursive: true }], unlink: [j('dir-link/secret.txt')],
          copyFile: [j('copy-src'), j('file-link')], cp: [j('copy-src'), j('dir-link/victim/keep'), { force: true }],
          symlink: [j('x'), j('dir-link/new-link')], open: [j('file-link'), 'w'],
        };
        fs.writeFileSync(j('copy-src'), 'pwned');
        for (const [name, args] of [...Object.entries(reads), ...Object.entries(writes)]) {
          deniedSync(() => fs[name + 'Sync'](...args), name + ' sync');
          await deniedAsync(fsp[name](...args), name + ' promise');
        }
        for (const make of [() => fs.createReadStream(j('file-link')), () => fs.createWriteStream(j('file-link'))]) {
          const stream = make();
          denied(await new Promise((r) => stream.on('error', r)), 'stream');
        }
        // a dangling link counts as missing for `mkdir -p`, which must still not create its target
        fs.symlinkSync(o('created'), j('dangling'));
        for (const make of [() => fs.mkdirSync(j('dangling/sub'), { recursive: true }), () => fs.writeFileSync(j('dangling'), 'x')]) {
          let error = (() => { try { make(); } catch (e) { return e; } })();
          eq(error === undefined, false, 'went through the dangling link');
        }
        eq((await settle(fsp.mkdir(j('dangling/sub'), { recursive: true }))) === null, false, 'mkdir -p async');
        // async open still works: it is opened at call time, so there is nothing to re-check
        const handle = await fsp.open(j('copy-src'), 'r');
        await handle.close();"#,
        &[],
    ));
    sandbox.assert_out_untouched();
}

/// Attack: `cp` and recursive `rm` over trees that contain links pointing out.
#[test]
fn cp_and_recursive_rm_never_follow_links_in_a_tree() {
    let sandbox = Sandbox::new();
    assert_ok(sandbox.run(
        r#"fs.mkdirSync(j('tree'));
        fs.symlinkSync(OUT, j('tree/out'));
        fs.symlinkSync(o('secret.txt'), j('tree/secret'));
        // copying the tree copies the links as links, not what they point at
        fs.cpSync(j('tree'), j('copy'), { recursive: true });
        eq(fs.lstatSync(j('copy/out')).isSymbolicLink(), true);
        eq(fs.readlinkSync(j('copy/secret')), o('secret.txt'));
        deniedSync(() => fs.readFileSync(j('copy/secret')), 'read copied link');
        await fsp.cp(j('tree/secret'), j('single'));
        eq(fs.lstatSync(j('single')).isSymbolicLink(), true);
        // a destination that already holds a link to a directory is not merged into
        fs.mkdirSync(j('src')); fs.mkdirSync(j('src/victim')); fs.writeFileSync(j('src/victim/keep'), 'pwned');
        fs.mkdirSync(j('dest')); fs.symlinkSync(o('victim'), j('dest/victim'));
        let error = (() => { try { fs.cpSync(j('src'), j('dest'), { recursive: true, force: true }); } catch (e) { return e; } })();
        eq(error && error.code, 'EEXIST', 'cp into a linked dir');
        // a top-level destination link is resolved and denied; inside a tree, force replaces
        // the link and never writes its target
        fs.writeFileSync(j('src2'), 'pwned');
        fs.symlinkSync(o('secret.txt'), j('dest2'));
        deniedSync(() => fs.cpSync(j('src2'), j('dest2'), { force: true }), 'cp onto a link');
        fs.mkdirSync(j('d3')); fs.writeFileSync(j('d3/secret.txt'), 'pwned');
        fs.mkdirSync(j('t3')); fs.symlinkSync(o('secret.txt'), j('t3/secret.txt'));
        fs.cpSync(j('d3'), j('t3'), { recursive: true, force: true });
        eq(fs.lstatSync(j('t3/secret.txt')).isSymbolicLink(), false);
        // recursive rm removes links, not what they point at
        fs.rmSync(j('tree'), { recursive: true });
        await fsp.rm(j('copy'), { recursive: true });
        eq(fs.existsSync(j('tree')) || fs.existsSync(j('copy')), false);
        fs.symlinkSync(o('victim'), j('dir-link-root'));
        fs.rmSync(j('dir-link-root'), { recursive: true, force: true });
        eq(fs.existsSync(j('dir-link-root')), false);"#,
        &[],
    ));
    sandbox.assert_out_untouched();
}

/// Attack: `mkdtemp` with a prefix that climbs out, goes through a link, or whose placeholder
/// name already exists as a link.
#[test]
fn mkdtemp_prefix_cannot_leave_the_grant() {
    let sandbox = Sandbox::new();
    let out_name = sandbox
        .out_path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let body = format!(
        "const OUT_NAME = {out_name:?};{}",
        r#"fs.symlinkSync(OUT, j('outdir'));
        fs.symlinkSync(OUT, j('pXXXXXX'));
        const prefixes = [o('t-'), j('../t-'), j('../' + OUT_NAME + '/t-'), j('outdir/t-'), j('outdir/'), j('p'), j('missing/../../t-')];
        for (const prefix of prefixes) {
          let error = (() => { try { fs.mkdtempSync(prefix); } catch (e) { return e; } })();
          eq(error === undefined, false, 'mkdtemp allowed ' + prefix);
          eq(['ERR_ACCESS_DENIED', 'ENOENT'].includes(error.code), true, prefix + ' ' + error.code);
          error = await settle(fsp.mkdtemp(prefix));
          eq(error === null, false, 'mkdtemp async allowed ' + prefix);
        }
        const made = fs.mkdtempSync(j('ok-'));
        eq(path.dirname(made), DIR);
        eq(fs.lstatSync(made).isDirectory(), true);"#
    );
    assert_ok(sandbox.run(&body, &[]));
    sandbox.assert_out_untouched();
}

/// With a deny inside the grant, a link into the denied path still cannot be followed, and the
/// parent fallback only ever acts on the link.
#[test]
fn links_into_a_denied_subtree_cannot_be_followed() {
    let sandbox = Sandbox::new();
    let protected = sandbox.dir_path().join("protected");
    std::fs::create_dir(&protected).unwrap();
    std::fs::write(protected.join("secret"), "inner").unwrap();
    assert_ok(sandbox.run(
        r#"deniedSync(() => fs.readFileSync(j('protected/secret')), 'direct read');
        fs.symlinkSync(j('protected/secret'), j('to-secret'));
        fs.symlinkSync(j('protected'), j('to-dir'));
        deniedSync(() => fs.readFileSync(j('to-secret')), 'read via link');
        deniedSync(() => fs.writeFileSync(j('to-secret'), 'x'), 'write via link');
        deniedSync(() => fs.readdirSync(j('to-dir')), 'readdir via link');
        deniedSync(() => fs.unlinkSync(j('to-dir/secret')), 'unlink via linked dir');
        deniedSync(() => fs.renameSync(j('protected/secret'), j('stolen')), 'rename out of deny');
        deniedSync(() => fs.symlinkSync(j('x'), j('protected/planted')), 'plant into deny');
        eq(fs.lstatSync(j('to-secret')).isSymbolicLink(), true);
        fs.rmSync(j('to-dir'), { recursive: true });
        fs.unlinkSync(j('to-secret'));"#,
        &["protected"],
    ));
    assert_eq!(read(&protected.join("secret")), "inner");
    sandbox.assert_out_untouched();
}

/// Open finding (no symlink involved): tree operations authorize only their root, so a deny
/// nested under the grant is read by `cp`, moved by `rename` and deleted by recursive `rm`.
#[test]
#[ignore = "VELYR-179 open finding: deny below a tree root, needs a human decision"]
fn tree_operations_respect_a_nested_deny() {
    let sandbox = Sandbox::new();
    let protected = sandbox.dir_path().join("parent/protected");
    std::fs::create_dir_all(&protected).unwrap();
    std::fs::write(protected.join("secret"), "inner").unwrap();
    assert_ok(sandbox.run(
        r#"deniedSync(() => fs.readFileSync(j('parent/protected/secret')), 'direct read');
        deniedSync(() => fs.cpSync(j('parent'), j('copy'), { recursive: true }), 'cp reads the deny');
        deniedSync(() => fs.renameSync(j('parent'), j('moved')), 'rename moves the deny');
        deniedSync(() => fs.rmSync(j('parent'), { recursive: true }), 'rm deletes the deny');"#,
        &["parent/protected"],
    ));
    assert_eq!(read(&protected.join("secret")), "inner");
}
