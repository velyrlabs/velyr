//! `node:fs` and `node:fs/promises` over io-fs: every API in its sync, callback and promise form,
//! fd handling, streams, and authorization. Scripts run through `common::script`; each one gets
//! `DIR`, a canonical temp directory the grants cover.

mod common;

use common::script::{CHECKS, assert_ok, run_with};
use std::sync::Arc;
use velyr_runtime_permissions::{Capability, CapabilitySet, PathPattern};

const PRELUDE: &str = r#"
const { Buffer } = globalThis[Symbol.for('velyr.node')].require('buffer');
const node = globalThis[Symbol.for('velyr.node')];
const fs = node.require('fs');
const fsp = node.require('fs/promises');
const path = node.require('path');
const cb = (fn, ...args) => new Promise((resolve) => fn(...args, (error, value, extra) => resolve({ error, value, extra })));
const fails = (fn, code, label) => { try { fn(); } catch (e) { eq(e.code, code, label || 'code'); return e; } throw new Error('did not throw ' + code + ' ' + (label || '')); };
const rejects = async (promise, code, label) => { try { await promise; } catch (e) { eq(e.code, code, label || 'code'); return e; } throw new Error('did not reject ' + code + ' ' + (label || '')); };
// The runtime's default `path` is posix, while fs reports native paths, as Node's win32 `path` does.
const j = (name) => (process.platform === 'win32' ? path.win32 : path).join(DIR, name);
"#;

fn grants(dir: &str, read: bool, write: bool) -> Arc<CapabilitySet> {
    let mut builder = CapabilitySet::builder();
    if read {
        builder = builder.allow(Capability::FsRead(PathPattern::new(dir)));
    }
    if write {
        builder = builder.allow(Capability::FsWrite(PathPattern::new(dir)));
    }
    builder.build()
}

fn run_fs(body: &str, read: bool, write: bool) -> String {
    let dir = tempfile::tempdir().unwrap();
    let canonical = dunce::canonicalize(dir.path()).unwrap();
    let dir_text = canonical.to_str().unwrap();
    let script = format!("{CHECKS}const DIR = {dir_text:?};{PRELUDE}{body}");
    run_with(&script, grants(dir_text, read, write), "")
}

fn full(body: &str) -> String {
    run_fs(body, true, true)
}

#[test]
fn sync_api_covers_files_directories_and_links() {
    assert_ok(full(
        r#"fs.writeFileSync(j('a.txt'), 'hello');
        eq(fs.readFileSync(j('a.txt'), 'utf8'), 'hello');
        eq(Buffer.isBuffer(fs.readFileSync(j('a.txt'))), true);
        eq(fs.readFileSync(j('a.txt'), { encoding: 'hex' }), '68656c6c6f');
        fs.appendFileSync(j('a.txt'), ' world');
        eq(fs.readFileSync(j('a.txt')).toString(), 'hello world');
        fs.writeFileSync(j('b.bin'), new Uint8Array([1, 2, 3]));
        eq(fs.readFileSync(j('b.bin')).length, 3);
        eq(fs.existsSync(j('a.txt')), true);
        eq(fs.existsSync(j('missing')), false);

        const st = fs.statSync(j('a.txt'));
        eq(st.isFile(), true); eq(st.isDirectory(), false); eq(st.size, 11);
        eq(st.mtime instanceof Date, true); eq(typeof st.mtimeMs, 'number'); eq(st.mode & 0o170000, 0o100000);
        eq(fs.statSync(DIR).isDirectory(), true);

        eq(fs.mkdirSync(j('x/y/z'), { recursive: true }), j('x'));
        eq(fs.mkdirSync(j('x/y/z'), { recursive: true }), undefined);
        fails(() => fs.mkdirSync(j('x')), 'EEXIST');
        fs.mkdirSync(j('d'));
        fs.writeFileSync(j('d/one'), '1');
        fs.mkdirSync(j('d/sub'));
        same(fs.readdirSync(j('d')).sort(), ['one', 'sub']);
        const ents = fs.readdirSync(j('d'), { withFileTypes: true }).sort((p, q) => p.name < q.name ? -1 : 1);
        same(ents.map((e) => [e.name, e.isFile(), e.isDirectory()]), [['one', true, false], ['sub', false, true]]);
        eq(ents[0].parentPath, j('d'));
        same(fs.readdirSync(j('d'), { recursive: true }).sort(), ['one', 'sub']);

        fs.symlinkSync(j('a.txt'), j('link'));
        eq(fs.readlinkSync(j('link')), j('a.txt'));
        eq(fs.lstatSync(j('link')).isSymbolicLink(), true);
        eq(fs.statSync(j('link')).isSymbolicLink(), false);
        eq(fs.realpathSync(j('link')), fs.realpathSync(j('a.txt')));
        eq(fs.realpathSync.native(j('link')), fs.realpathSync(j('a.txt')));

        fs.renameSync(j('a.txt'), j('renamed.txt'));
        eq(fs.existsSync(j('a.txt')), false);
        fs.copyFileSync(j('renamed.txt'), j('copy.txt'));
        eq(fs.readFileSync(j('copy.txt'), 'utf8'), 'hello world');
        fails(() => fs.copyFileSync(j('renamed.txt'), j('copy.txt'), fs.constants.COPYFILE_EXCL), 'EEXIST');
        fs.cpSync(j('d'), j('d2'), { recursive: true });
        eq(fs.readFileSync(j('d2/one'), 'utf8'), '1');
        fails(() => fs.cpSync(j('d'), j('d3')), 'ERR_FS_EISDIR');

        fs.accessSync(j('copy.txt'));
        fs.accessSync(j('copy.txt'), fs.constants.R_OK | fs.constants.W_OK);
        fails(() => fs.accessSync(j('nope')), 'ENOENT');
        fs.chmodSync(j('copy.txt'), 0o600);
        eq(fs.statSync(j('copy.txt')).mode & 0o777, 0o600);
        fs.utimesSync(j('copy.txt'), 1000, 2000);
        eq(fs.statSync(j('copy.txt')).mtimeMs, 2000000);
        eq(fs.statSync(j('copy.txt')).atimeMs, 1000000);
        const tmp = fs.mkdtempSync(j('tmp-'));
        eq(tmp.startsWith(j('tmp-')) && tmp.length === j('tmp-').length + 6, true);
        eq(fs.statSync(tmp).isDirectory(), true);
        const dir = fs.opendirSync(j('d'));
        const names = []; let entry;
        while ((entry = dir.readSync()) !== null) names.push(entry.name);
        dir.closeSync();
        same(names.sort(), ['one', 'sub']);

        fs.unlinkSync(j('b.bin'));
        fails(() => fs.unlinkSync(j('b.bin')), 'ENOENT');
        fails(() => fs.rmdirSync(j('d')), 'ENOTEMPTY');
        fs.rmdirSync(j('x/y/z'));
        fails(() => fs.rmSync(j('d')), 'ERR_FS_EISDIR');
        fs.rmSync(j('d'), { recursive: true });
        fs.rmSync(j('d'), { force: true });
        fails(() => fs.rmSync(j('d')), 'ENOENT');
        eq(fs.existsSync(j('d')), false);"#,
    ));
}

#[test]
fn sync_errors_carry_node_properties() {
    assert_ok(full(
        r#"const e = fails(() => fs.readFileSync(j('nope')), 'ENOENT');
        eq(e instanceof Error, true);
        eq(e.syscall, 'open'); eq(e.path, j('nope')); eq(e.errno < 0, true);
        eq(e.message, `ENOENT: no such file or directory, open '${j('nope')}'`);
        eq(fails(() => fs.statSync(j('nope')), 'ENOENT').syscall, 'stat');
        eq(fails(() => fs.readdirSync(j('nope')), 'ENOENT').syscall, 'scandir');
        fs.writeFileSync(j('f'), '');
        const r = fails(() => fs.renameSync(j('nope'), j('other')), 'ENOENT');
        eq(r.dest, j('other'));
        eq(r.message, `ENOENT: no such file or directory, rename '${j('nope')}' -> '${j('other')}'`);
        fails(() => fs.readdirSync(j('f')), 'ENOTDIR');
        fails(() => fs.readFileSync(DIR), 'EISDIR');
        fails(() => fs.readFileSync(123.5), 'ERR_INVALID_ARG_TYPE');
        fails(() => fs.readFileSync(j('a\0b')), 'ERR_INVALID_ARG_VALUE');
        fails(() => fs.writeFileSync(j('f'), 5), 'ERR_INVALID_ARG_TYPE');"#,
    ));
}

#[test]
fn fd_api_maps_descriptors_to_resources() {
    assert_ok(full(
        r#"const fd = fs.openSync(j('fd.txt'), 'w+');
        eq(Number.isInteger(fd) && fd >= 3, true, 'fd is >= 3');
        eq(fs.writeSync(fd, 'hello world'), 11);
        eq(fs.writeSync(fd, Buffer.from('HE'), 0, 2, 0), 2);
        const buf = Buffer.alloc(5);
        eq(fs.readSync(fd, buf, 0, 5, 0), 5);
        eq(buf.toString(), 'HEllo');
        eq(fs.fstatSync(fd).size, 11);
        const second = fs.openSync(j('fd.txt'), 'r');
        eq(second !== fd, true);
        eq(fs.readSync(second, buf, 0, 5, null), 5);
        eq(fs.readSync(second, buf, 0, 5, null), 5);
        eq(buf.toString(), ' worl');
        eq(fs.readSync(second, buf, 0, 5, 11), 0, 'eof');
        fs.closeSync(fd); fs.closeSync(second);
        fs.appendFileSync(j('fd.txt'), '!');
        eq(fs.readFileSync(j('fd.txt'), 'utf8'), 'HEllo world!');
        const ap = fs.openSync(j('fd.txt'), 'a');
        fs.writeSync(ap, '?'); fs.closeSync(ap);
        eq(fs.readFileSync(j('fd.txt'), 'utf8'), 'HEllo world!?');
        fails(() => fs.openSync(j('fd.txt'), 'wx'), 'EEXIST');
        fails(() => fs.openSync(j('nope'), 'r'), 'ENOENT');
        fs.writeFileSync(fs.openSync(j('viafd'), 'w'), 'by fd');
        eq(fs.readFileSync(j('viafd'), 'utf8'), 'by fd');
        // a closed, never-issued or malformed descriptor is EBADF; closing twice too
        fails(() => fs.closeSync(fd), 'EBADF', 'double close');
        fails(() => fs.readSync(fd, buf, 0, 5, 0), 'EBADF', 'read closed');
        fails(() => fs.writeSync(fd, 'x'), 'EBADF', 'write closed');
        fails(() => fs.fstatSync(fd), 'EBADF', 'fstat closed');
        fails(() => fs.fstatSync(999999), 'EBADF', 'never issued');
        fails(() => fs.closeSync(0), 'EBADF', 'stdio is not a resource');
        fails(() => fs.closeSync(-1), 'EBADF', 'negative');
        fails(() => fs.fstatSync('x'), 'ERR_INVALID_ARG_TYPE');"#,
    ));
}

#[test]
fn callback_api_matches_the_sync_results() {
    assert_ok(full(
        r#"const ok = (r, label) => { if (r.error) throw r.error; return r.value; };
        ok(await cb(fs.writeFile, j('c.txt'), 'callback'));
        eq(ok(await cb(fs.readFile, j('c.txt'), 'utf8')), 'callback');
        eq(Buffer.isBuffer(ok(await cb(fs.readFile, j('c.txt')))), true);
        ok(await cb(fs.appendFile, j('c.txt'), '!'));
        eq(ok(await cb(fs.readFile, j('c.txt'), { encoding: 'utf8' })), 'callback!');
        eq(ok(await cb(fs.stat, j('c.txt'))).size, 9);
        eq(ok(await cb(fs.lstat, j('c.txt'))).isFile(), true);
        eq(ok(await cb(fs.mkdir, j('m/n'), { recursive: true })), j('m'));
        ok(await cb(fs.writeFile, j('m/n/f'), 'x'));
        same(ok(await cb(fs.readdir, j('m/n'))), ['f']);
        eq(ok(await cb(fs.readdir, j('m/n'), { withFileTypes: true }))[0].isFile(), true);
        ok(await cb(fs.copyFile, j('c.txt'), j('c2.txt')));
        ok(await cb(fs.rename, j('c2.txt'), j('c3.txt')));
        ok(await cb(fs.access, j('c3.txt')));
        eq((await cb(fs.access, j('c2.txt'))).error.code, 'ENOENT');
        eq(await new Promise((r) => fs.exists(j('c3.txt'), r)), true);
        eq(await new Promise((r) => fs.exists(j('c2.txt'), r)), false);
        eq(ok(await cb(fs.realpath, j('c3.txt'))), fs.realpathSync(j('c3.txt')));
        ok(await cb(fs.symlink, j('c3.txt'), j('ln')));
        eq(ok(await cb(fs.readlink, j('ln'))), j('c3.txt'));
        ok(await cb(fs.chmod, j('c3.txt'), 0o640));
        ok(await cb(fs.utimes, j('c3.txt'), 5, 6));
        eq(fs.statSync(j('c3.txt')).mtimeMs, 6000);
        ok(await cb(fs.cp, j('m'), j('m2'), { recursive: true }));
        eq(fs.readFileSync(j('m2/n/f'), 'utf8'), 'x');
        eq(ok(await cb(fs.mkdtemp, j('t-'))).startsWith(j('t-')), true);

        const fd = ok(await cb(fs.open, j('c3.txt'), 'r+'));
        eq(fd >= 3, true);
        const buf = Buffer.alloc(4);
        const read = await cb(fs.read, fd, buf, 0, 4, 0);
        eq(read.value, 4); eq(read.extra, buf); eq(buf.toString(), 'call');
        eq((await cb(fs.write, fd, 'CALL', 0)).value, 4);
        eq(ok(await cb(fs.fstat, fd)).size, 9);
        ok(await cb(fs.close, fd));
        eq(fs.readFileSync(j('c3.txt'), 'utf8'), 'CALLback!');
        eq((await cb(fs.close, fd)).error.code, 'EBADF');
        eq((await cb(fs.read, fd, buf, 0, 4, 0)).error.code, 'EBADF');
        eq((await cb(fs.readFile, j('nope'))).error.code, 'ENOENT');
        ok(await cb(fs.unlink, j('ln')));
        ok(await cb(fs.rm, j('m'), { recursive: true }));
        ok(await cb(fs.mkdir, j('rd')));
        ok(await cb(fs.rmdir, j('rd')));
        eq(fs.existsSync(j('m')), false);
        const arg = (() => { try { fs.readFile(j('c.txt')); } catch (e) { return e.code; } })();
        eq(arg, 'ERR_INVALID_ARG_TYPE');"#,
    ));
}

#[test]
fn promises_api_and_file_handles() {
    assert_ok(full(
        r#"eq(fs.promises, fsp);
        await fsp.writeFile(j('p.txt'), 'promise');
        eq(await fsp.readFile(j('p.txt'), 'utf8'), 'promise');
        await fsp.appendFile(j('p.txt'), 's');
        eq((await fsp.readFile(j('p.txt'))).toString(), 'promises');
        eq((await fsp.stat(j('p.txt'))).size, 8);
        eq((await fsp.lstat(j('p.txt'))).isFile(), true);
        eq(await fsp.mkdir(j('q/r'), { recursive: true }), j('q'));
        await fsp.writeFile(j('q/r/f'), 'z');
        same(await fsp.readdir(j('q/r')), ['f']);
        eq((await fsp.readdir(j('q'), { withFileTypes: true }))[0].isDirectory(), true);
        await fsp.copyFile(j('p.txt'), j('p2.txt'));
        await fsp.rename(j('p2.txt'), j('p3.txt'));
        await fsp.access(j('p3.txt'));
        await rejects(fsp.access(j('p2.txt')), 'ENOENT');
        eq(await fsp.realpath(j('p3.txt')), fs.realpathSync(j('p3.txt')));
        await fsp.symlink(j('p3.txt'), j('pl'));
        eq(await fsp.readlink(j('pl')), j('p3.txt'));
        await fsp.chmod(j('p3.txt'), 0o600);
        await fsp.utimes(j('p3.txt'), 7, 8);
        eq((await fsp.stat(j('p3.txt'))).mtimeMs, 8000);
        await fsp.cp(j('q'), j('q2'), { recursive: true });
        eq(await fsp.readFile(j('q2/r/f'), 'utf8'), 'z');
        eq((await fsp.mkdtemp(j('pt-'))).startsWith(j('pt-')), true);
        const names = [];
        for await (const entry of await fsp.opendir(j('q'))) names.push(entry.name);
        same(names, ['r']);
        await fsp.unlink(j('pl'));
        await fsp.rm(j('q'), { recursive: true });
        await fsp.rmdir(j('q2/r')).catch((e) => eq(e.code, 'ENOTEMPTY'));
        await rejects(fsp.readFile(j('nope')), 'ENOENT');
        await rejects(fsp.rm(j('nope')), 'ENOENT');
        await fsp.rm(j('nope'), { force: true });

        const handle = await fsp.open(j('h.txt'), 'w+');
        eq(handle.fd >= 3, true);
        eq((await handle.write('handle data')).bytesWritten, 11);
        eq((await handle.write(Buffer.from('HA'), 0, 2, 0)).bytesWritten, 2);
        const buf = Buffer.alloc(6);
        const { bytesRead, buffer } = await handle.read(buf, 0, 6, 0);
        eq(bytesRead, 6); eq(buffer, buf); eq(buf.toString(), 'HAndle');
        eq((await handle.stat()).size, 11);
        // readFile starts at the cursor, which the unpositioned write left at the end (like Node)
        eq(await handle.readFile({ encoding: 'utf8' }), '');
        await handle.appendFile('!');
        await handle.close();
        eq(handle.fd, -1);
        eq(await fsp.readFile(j('h.txt'), 'utf8'), 'HAndle data!');
        await rejects(handle.read(buf, 0, 1, 0), 'EBADF');
        await rejects(fsp.open(j('nope'), 'r'), 'ENOENT');
        const viaHandle = await fsp.open(j('h2.txt'), 'w');
        await viaHandle.writeFile('whole');
        await viaHandle.close();
        eq(await fsp.readFile(j('h2.txt'), 'utf8'), 'whole');"#,
    ));
}

#[test]
fn streams_read_and_write_files() {
    assert_ok(full(
        r#"const stream = node.require('stream');
        const text = 'abcdefghij'.repeat(20000);
        const out = fs.createWriteStream(j('s.txt'));
        const opened = new Promise((r) => out.on('open', r));
        out.write(text.slice(0, 100000));
        out.write(text.slice(100000));
        await new Promise((r, x) => out.end(r));
        eq(out.bytesWritten, 200000);
        eq(out.path, j('s.txt'));
        eq(typeof (await opened), 'number');
        eq(fs.statSync(j('s.txt')).size, 200000);

        let got = '';
        const input = fs.createReadStream(j('s.txt'), { encoding: 'utf8', highWaterMark: 4096 });
        for await (const chunk of input) got += chunk;
        eq(got, text);
        const ranged = [];
        for await (const chunk of fs.createReadStream(j('s.txt'), { start: 5, end: 14 })) ranged.push(chunk);
        eq(Buffer.concat(ranged).toString(), 'fghijabcde');

        await node.require('stream/promises').pipeline(fs.createReadStream(j('s.txt')), fs.createWriteStream(j('s2.txt')));
        eq(fs.readFileSync(j('s2.txt'), 'utf8'), text);
        const appended = fs.createWriteStream(j('s2.txt'), { flags: 'a' });
        await new Promise((r) => appended.end('tail', r));
        eq(fs.readFileSync(j('s2.txt'), 'utf8'), text + 'tail');

        const missing = fs.createReadStream(j('nope'));
        const error = await new Promise((r) => missing.on('error', r));
        eq(error.code, 'ENOENT');
        const closed = fs.createReadStream(j('s.txt'));
        await new Promise((r) => { closed.on('close', r); closed.destroy(); });
        eq(closed.destroyed, true);"#,
    ));
}

const DENIED_CALLS: &str = r#"
const calls = {
  readFile: [j('f'), 'utf8'], writeFile: [j('f'), 'x'], appendFile: [j('f'), 'x'], stat: [j('f')], lstat: [j('f')],
  readdir: [DIR], mkdir: [j('dd')], rm: [j('f')], rmdir: [j('dd')], unlink: [j('f')], rename: [j('f'), j('g')],
  copyFile: [j('f'), j('g')], cp: [j('f'), j('g')], access: [j('f')], realpath: [j('f')], symlink: [j('f'), j('l')],
  readlink: [j('f')], open: [j('f'), 'r'], mkdtemp: [j('t-')], utimes: [j('f'), 1, 1], chmod: [j('f'), 0o600],
  opendir: [DIR],
};
const check = (error, name, variant) => {
  eq(error && error.code, 'ERR_ACCESS_DENIED', name + ' ' + variant);
  eq(error.cause && error.cause.code, 'VELYR_PERMISSION_DENIED', name + ' cause');
  eq(error instanceof Error, true);
};
"#;

#[test]
fn every_call_is_denied_without_a_grant() {
    let body = format!(
        "{DENIED_CALLS}{}",
        r#"for (const [name, args] of Object.entries(calls)) {
          try { fs[name + 'Sync'](...args); throw new Error('sync allowed ' + name); } catch (e) { check(e, name, 'sync'); }
          const result = await cb(fs[name], ...args);
          check(result.error, name, 'callback');
          try { await fsp[name](...args); throw new Error('promise allowed ' + name); } catch (e) { check(e, name, 'promise'); }
        }
        eq(await new Promise((r) => fs.exists(j('f'), r)), false, 'exists swallows the denial');
        eq(fs.existsSync(j('f')), false);
        for (const make of [() => fs.createReadStream(j('f')), () => fs.createWriteStream(j('f'))]) {
          const stream = make();
          check(await new Promise((r) => stream.on('error', r)), 'stream', 'stream');
        }
        check((() => { try { fs.openSync(j('f'), 'w'); } catch (e) { return e; } })(), 'open w', 'sync');
        // existence is never revealed to a caller without the grant
        check((() => { try { fs.statSync(j('does-not-exist')); } catch (e) { return e; } })(), 'stat missing', 'sync');"#
    );
    assert_ok(run_fs(&body, false, false));
}

#[test]
fn read_and_write_grants_are_separate() {
    // read-only: reads work, anything that changes the tree is denied
    let read_only = format!(
        "{DENIED_CALLS}{}",
        r#"eq(typeof fs.statSync(DIR).size, 'number');
        eq(fs.readdirSync(DIR).length, 0);
        for (const name of ['writeFile', 'appendFile', 'mkdir', 'rmdir', 'unlink', 'mkdtemp', 'chmod', 'utimes', 'symlink']) {
          try { fs[name + 'Sync'](...calls[name]); throw new Error('write allowed ' + name); } catch (e) { check(e, name, 'ro'); }
        }
        try { fs.openSync(j('f'), 'r+'); throw new Error('r+ allowed'); } catch (e) { check(e, 'open r+', 'ro'); }
        try { fs.openSync(j('f'), 'w'); throw new Error('w allowed'); } catch (e) { check(e, 'open w', 'ro'); }"#
    );
    assert_ok(run_fs(&read_only, true, false));
}

#[test]
fn write_only_grant_cannot_read() {
    let body = format!(
        "{DENIED_CALLS}{}",
        r#"fs.writeFileSync(j('f'), 'data');
        for (const name of ['readFile', 'stat', 'readdir', 'realpath', 'readlink', 'access']) {
          try { fs[name + 'Sync'](...calls[name]); throw new Error('read allowed ' + name); } catch (e) { check(e, name, 'wo'); }
        }"#
    );
    assert_ok(run_fs(&body, false, true));
}

// `fs.symlink` returns Unsupported on Windows, where creating links needs a privilege.
#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_the_grant() {
    let outside = tempfile::tempdir().unwrap();
    let secret = dunce::canonicalize(outside.path())
        .unwrap()
        .join("secret.txt");
    std::fs::write(&secret, "secret").unwrap();
    let body = format!(
        "{DENIED_CALLS}const secret = {:?};{}",
        secret.to_str().unwrap(),
        r#"fs.symlinkSync(secret, j('escape'));
        try { fs.readFileSync(j('escape')); throw new Error('followed the link out'); } catch (e) { check(e, 'read via link', 'sync'); }
        try { fs.writeFileSync(j('escape'), 'x'); throw new Error('wrote through the link'); } catch (e) { check(e, 'write via link', 'sync'); }
        // the link itself is inside the grant
        eq(fs.readlinkSync(j('escape')), secret);
        eq(fs.lstatSync(j('escape')).isSymbolicLink(), true);
        fs.unlinkSync(j('escape'));
        eq(fs.existsSync(j('escape')), false);"#
    );
    assert_ok(run_fs(&body, true, true));
    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "secret");
}
