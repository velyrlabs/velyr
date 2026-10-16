'use strict';
// node:os. Written for this runtime; no Node source is copied. Answers come from the OS through
// one host op (sysinfo and std), never from process.env, so no env permission is involved.
//
// Known limits: cpus().times are exact on Linux (/proc/stat) and zero elsewhere (reading them
// needs unsafe code, and sampling would block); networkInterfaces() is always empty (no interface enumeration
// without unsafe code or an extra sysinfo feature, and no net capability to gate it on);
// userInfo().shell is null; homedir/tmpdir follow std (HOME/TMPDIR, then the
// passwd entry); getPriority/setPriority are not implemented; the errno table is a subset.
const errors = require('internal/errors');
const ops = require('internal/ops');
const process = require('process');

const query = (name) => ops.os(name);
const lines = (text) => (text === '' ? [] : text.split('\n'));

const ARCH_MACHINE = { x64: 'x86_64', ia32: 'i686', arm64: process.platform === 'linux' ? 'aarch64' : 'arm64', arm: 'armv7l' };
const TYPES = { darwin: 'Darwin', linux: 'Linux', win32: 'Windows_NT' };

const platform = () => process.platform;
const arch = () => process.arch;
const type = () => TYPES[process.platform] || process.platform;
const machine = () => ARCH_MACHINE[process.arch] || process.arch;
const endianness = () => 'LE';
const hostname = () => query('hostname');
const release = () => query('release');
const version = () => query('version');
const uptime = () => query('uptime');
const totalmem = () => query('totalmem');
const freemem = () => query('freemem');
const homedir = () => query('homedir');
const tmpdir = () => {
  const dir = query('tmpdir');
  return dir.length > 1 && dir.endsWith('/') ? dir.slice(0, -1) : dir;
};

function cpus() {
  return lines(query('cpus')).map((line) => {
    const [model, speed, user, nice, sys, idle, irq] = line.split('\t');
    return { model, speed: Number(speed), times: { user: +user, nice: +nice, sys: +sys, idle: +idle, irq: +irq } };
  });
}

const availableParallelism = () => Math.max(1, ops.os('cpucount'));

function loadavg() {
  return query('loadavg').split('\t').map(Number);
}

const networkInterfaces = () => ({ __proto__: null });

function userInfo(options) {
  if (options !== undefined && options !== null) errors.validateObject(options, 'options');
  const [username = '', uid = '-1', gid = '-1'] = query('user').split('\t');
  const info = { uid: Number(uid), gid: Number(gid), username, homedir: homedir(), shell: null };
  if (options && options.encoding === 'buffer') {
    const { Buffer } = require('buffer');
    info.username = Buffer.from(info.username);
    info.homedir = Buffer.from(info.homedir);
  }
  return info;
}

const linux = process.platform === 'linux';
const signals = {
  SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGILL: 4, SIGTRAP: 5, SIGABRT: 6, SIGBUS: linux ? 7 : 10, SIGFPE: 8,
  SIGKILL: 9, SIGUSR1: linux ? 10 : 30, SIGSEGV: 11, SIGUSR2: linux ? 12 : 31, SIGPIPE: 13, SIGALRM: 14,
  SIGTERM: 15, SIGCHLD: linux ? 17 : 20, SIGCONT: linux ? 18 : 19, SIGSTOP: linux ? 19 : 17, SIGTSTP: linux ? 20 : 18,
  SIGTTIN: 21, SIGTTOU: 22, SIGURG: linux ? 23 : 16, SIGXCPU: 24, SIGXFSZ: 25, SIGVTALRM: 26, SIGPROF: 27,
  SIGWINCH: 28, SIGIO: linux ? 29 : 23, SIGSYS: linux ? 31 : 12,
};
const errno = {
  E2BIG: 7, EACCES: 13, EADDRINUSE: linux ? 98 : 48, EADDRNOTAVAIL: linux ? 99 : 49, EAGAIN: linux ? 11 : 35,
  EBADF: 9, EBUSY: 16, ECONNABORTED: linux ? 103 : 53, ECONNREFUSED: linux ? 111 : 61, ECONNRESET: linux ? 104 : 54,
  EEXIST: 17, EINTR: 4, EINVAL: 22, EIO: 5, EISDIR: 21, EMFILE: 24, ENOENT: 2, ENOMEM: 12,
  ENOSPC: 28, ENOTDIR: 20, ENOTEMPTY: linux ? 39 : 66, EPERM: 1, EPIPE: 32, ETIMEDOUT: linux ? 110 : 60, EXDEV: 18,
};
const priority = {
  PRIORITY_LOW: 19, PRIORITY_BELOW_NORMAL: 10, PRIORITY_NORMAL: 0, PRIORITY_ABOVE_NORMAL: -7,
  PRIORITY_HIGH: -14, PRIORITY_HIGHEST: -20,
};
const constants = Object.freeze({
  UV_UDP_REUSEADDR: 4,
  dlopen: Object.freeze({ RTLD_LAZY: 1, RTLD_NOW: 2, RTLD_GLOBAL: linux ? 256 : 8, RTLD_LOCAL: linux ? 0 : 4 }),
  errno: Object.freeze(errno),
  signals: Object.freeze(signals),
  priority: Object.freeze(priority),
});

module.exports = {
  EOL: process.platform === 'win32' ? '\r\n' : '\n',
  devNull: process.platform === 'win32' ? '\\\\.\\nul' : '/dev/null',
  arch, availableParallelism, constants, cpus, endianness, freemem, homedir, hostname, loadavg, machine,
  networkInterfaces, platform, release, tmpdir, totalmem, type, uptime, userInfo, version,
};
