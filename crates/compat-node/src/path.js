'use strict';
// node:path (posix and win32), written for this runtime.
// Known limits: `path.matchesGlob` is not implemented; without `process.cwd`/`Velyr.cwd`,
// relative paths resolve against "/".
const { validateString, validateObject } = require('internal/errors');

function cwd() {
  const proc = globalThis.process;
  if (proc && typeof proc.cwd === 'function') return proc.cwd();
  const velyr = globalThis.Velyr;
  if (velyr && typeof velyr.cwd === 'function') return velyr.cwd();
  return '/';
}

// Segments of `text` split on `separators`, with "." and empty parts removed and ".." applied.
function segments(text, separators, allowAboveRoot) {
  const out = [];
  for (const part of text.split(separators)) {
    if (part === '' || part === '.') continue;
    if (part !== '..') out.push(part);
    else if (out.length && out[out.length - 1] !== '..') out.pop();
    else if (allowAboveRoot) out.push('..');
  }
  return out;
}

function extOf(base) {
  const dot = base.lastIndexOf('.');
  return dot <= 0 || base === '..' ? '' : base.slice(dot);
}

function normalizeExt(ext) {
  return ext && ext[0] !== '.' ? `.${ext}` : ext || '';
}

function formatPath(sep, object) {
  validateObject(object, 'pathObject');
  const dir = object.dir || object.root;
  const base = object.base || `${object.name || ''}${normalizeExt(object.ext)}`;
  if (!dir) return base;
  return dir === object.root ? `${dir}${base}` : `${dir}${sep}${base}`;
}

function stripSuffix(base, suffix) {
  return suffix.length > 0 && suffix.length < base.length && base.endsWith(suffix)
    ? base.slice(0, base.length - suffix.length)
    : base;
}

const posix = {
  sep: '/',
  delimiter: ':',
  resolve(...args) {
    let resolved = '';
    let absolute = false;
    for (let i = args.length - 1; i >= -1 && !absolute; i--) {
      const path = i >= 0 ? args[i] : cwd();
      if (i >= 0) validateString(path, `paths[${i}]`);
      if (path.length === 0) continue;
      resolved = `${path}/${resolved}`;
      absolute = path[0] === '/';
    }
    const tail = segments(resolved, '/', !absolute).join('/');
    return absolute ? `/${tail}` : tail || '.';
  },
  normalize(path) {
    validateString(path, 'path');
    if (path.length === 0) return '.';
    const absolute = path[0] === '/';
    let tail = segments(path, '/', !absolute).join('/');
    if (!tail && !absolute) tail = '.';
    if (tail && path.endsWith('/')) tail += '/';
    return absolute ? `/${tail}` : tail;
  },
  isAbsolute(path) {
    validateString(path, 'path');
    return path.length > 0 && path[0] === '/';
  },
  join(...args) {
    args.forEach((arg, i) => validateString(arg, 'path'));
    const joined = args.filter((arg) => arg.length > 0).join('/');
    return joined ? posix.normalize(joined) : '.';
  },
  relative(from, to) {
    validateString(from, 'from');
    validateString(to, 'to');
    if (from === to) return '';
    const a = segments(posix.resolve(from), '/', false);
    const b = segments(posix.resolve(to), '/', false);
    let common = 0;
    while (common < a.length && common < b.length && a[common] === b[common]) common++;
    return [...Array(a.length - common).fill('..'), ...b.slice(common)].join('/');
  },
  toNamespacedPath(path) {
    return path;
  },
  dirname(path) {
    validateString(path, 'path');
    if (path.length === 0) return '.';
    let end = path.length;
    while (end > 1 && path[end - 1] === '/') end--;
    const slash = path.lastIndexOf('/', end - 1);
    if (slash === -1) return '.';
    if (slash === 0) return '/';
    return path.slice(0, slash);
  },
  basename(path, suffix) {
    validateString(path, 'path');
    if (suffix !== undefined) validateString(suffix, 'suffix');
    if (suffix !== undefined && suffix === path) return '';
    let end = path.length;
    while (end > 0 && path[end - 1] === '/') end--;
    const base = path.slice(path.lastIndexOf('/', end - 1) + 1, end);
    return suffix === undefined ? base : stripSuffix(base, suffix);
  },
  extname(path) {
    validateString(path, 'path');
    return extOf(posix.basename(path));
  },
  format: (object) => formatPath('/', object),
  parse(path) {
    validateString(path, 'path');
    const parsed = { root: '', dir: '', base: '', ext: '', name: '' };
    if (path.length === 0) return parsed;
    parsed.root = path[0] === '/' ? '/' : '';
    let end = path.length;
    while (end > 1 && path[end - 1] === '/') end--;
    const trimmed = path.slice(0, end);
    const slash = trimmed.lastIndexOf('/');
    if (trimmed === '/') {
      parsed.dir = '/';
      return parsed;
    }
    parsed.base = trimmed.slice(slash + 1);
    parsed.ext = extOf(parsed.base);
    parsed.name = parsed.ext ? parsed.base.slice(0, -parsed.ext.length) : parsed.base;
    parsed.dir = slash === -1 ? '' : slash === 0 ? '/' : trimmed.slice(0, slash);
    return parsed;
  },
};

const SEP = /[\\/]+/;
const isSep = (ch) => ch === '\\' || ch === '/';

// Splits a win32 path into its device ("C:" or "\\server\share"), whether a root separator
// follows, and the index where the rest starts.
function splitRoot(path) {
  const unc = /^[\\/]{2}([^\\/]+)[\\/]+([^\\/]+)/.exec(path);
  if (unc) {
    const next = unc[0].length;
    return { device: `\\\\${unc[1]}\\${unc[2]}`, absolute: true, end: isSep(path[next]) ? next + 1 : next };
  }
  if (isSep(path[0])) return { device: '', absolute: true, end: 1 };
  if (/^[a-zA-Z]:/.test(path)) {
    return isSep(path[2]) ? { device: path.slice(0, 2), absolute: true, end: 3 } : { device: path.slice(0, 2), absolute: false, end: 2 };
  }
  return { device: '', absolute: false, end: 0 };
}

const win32 = {
  sep: '\\',
  delimiter: ';',
  resolve(...args) {
    let device = '';
    let tail = '';
    let absolute = false;
    const fold = (text) => text.toLowerCase();
    const consider = (path) => {
      if (path.length === 0) return;
      const root = splitRoot(path);
      if (root.device && device && fold(root.device) !== fold(device)) return;
      if (!device && root.device) device = root.device;
      if (!absolute) {
        tail = `${path.slice(root.end)}\\${tail}`;
        absolute = root.absolute;
      }
    };
    for (let i = args.length - 1; i >= 0 && !(device && absolute); i--) {
      validateString(args[i], `paths[${i}]`);
      consider(args[i]);
    }
    if (!absolute) {
      const here = cwd();
      const root = splitRoot(here);
      if (!device || !root.device || fold(root.device) === fold(device)) consider(here);
      else consider(`${device}\\`);
    }
    const body = segments(tail, SEP, !absolute).join('\\');
    return `${device}${absolute ? '\\' : ''}${body}` || '.';
  },
  normalize(path) {
    validateString(path, 'path');
    if (path.length === 0) return '.';
    const root = splitRoot(path);
    let body = segments(path.slice(root.end), SEP, !root.absolute).join('\\');
    if (!body && !root.absolute) body = '.';
    if (body && isSep(path[path.length - 1])) body += '\\';
    return `${root.device}${root.absolute ? '\\' : ''}${body}`;
  },
  isAbsolute(path) {
    validateString(path, 'path');
    return path.length > 0 && splitRoot(path).absolute;
  },
  join(...args) {
    args.forEach((arg) => validateString(arg, 'path'));
    const parts = args.filter((arg) => arg.length > 0);
    if (parts.length === 0) return '.';
    let joined = parts.join('\\');
    // A leading "\\server" keeps its two slashes (UNC); any other run of leading slashes collapses.
    const first = parts[0];
    const uncStart = isSep(first[0]) && isSep(first[1]) && first.length > 2 && !isSep(first[2]);
    if (!uncStart) {
      let slashes = 0;
      while (slashes < joined.length && isSep(joined[slashes])) slashes++;
      if (slashes >= 2) joined = `\\${joined.slice(slashes)}`;
    }
    return win32.normalize(joined);
  },
  relative(from, to) {
    validateString(from, 'from');
    validateString(to, 'to');
    if (from === to) return '';
    const a = win32.resolve(from);
    const b = win32.resolve(to);
    if (a.toLowerCase() === b.toLowerCase()) return '';
    const ra = splitRoot(a);
    const rb = splitRoot(b);
    if (ra.device.toLowerCase() !== rb.device.toLowerCase()) return b;
    const sa = segments(a.slice(ra.end), SEP, false);
    const sb = segments(b.slice(rb.end), SEP, false);
    let common = 0;
    while (common < sa.length && common < sb.length && sa[common].toLowerCase() === sb[common].toLowerCase()) common++;
    return [...Array(sa.length - common).fill('..'), ...sb.slice(common)].join('\\');
  },
  toNamespacedPath(path) {
    if (typeof path !== 'string' || path.length === 0) return path;
    const resolved = win32.resolve(path);
    if (resolved.length <= 2) return path;
    if (resolved.startsWith('\\\\')) {
      return resolved[2] !== '?' && resolved[2] !== '.' ? `\\\\?\\UNC\\${resolved.slice(2)}` : path;
    }
    return /^[a-zA-Z]:\\/.test(resolved) ? `\\\\?\\${resolved}` : path;
  },
  dirname(path) {
    validateString(path, 'path');
    if (path.length === 0) return '.';
    const root = splitRoot(path);
    let end = path.length;
    while (end > root.end && isSep(path[end - 1])) end--;
    let slash = -1;
    for (let i = end - 1; i >= root.end; i--) if (isSep(path[i])) { slash = i; break; }
    if (slash === -1) return root.end > 0 ? path.slice(0, root.end) : '.';
    return path.slice(0, slash);
  },
  basename(path, suffix) {
    validateString(path, 'path');
    if (suffix !== undefined) validateString(suffix, 'suffix');
    if (suffix !== undefined && suffix === path) return '';
    const start = /^[a-zA-Z]:/.test(path) ? 2 : 0;
    let end = path.length;
    while (end > start && isSep(path[end - 1])) end--;
    let from = end;
    while (from > start && !isSep(path[from - 1])) from--;
    const base = path.slice(from, end);
    return suffix === undefined ? base : stripSuffix(base, suffix);
  },
  extname(path) {
    validateString(path, 'path');
    return extOf(win32.basename(path));
  },
  format: (object) => formatPath('\\', object),
  parse(path) {
    validateString(path, 'path');
    const parsed = { root: '', dir: '', base: '', ext: '', name: '' };
    if (path.length === 0) return parsed;
    const root = splitRoot(path);
    parsed.root = path.slice(0, root.end);
    let end = path.length;
    while (end > root.end && isSep(path[end - 1])) end--;
    const rest = path.slice(root.end, end);
    if (rest.length === 0) {
      parsed.dir = parsed.root;
      return parsed;
    }
    let slash = -1;
    for (let i = rest.length - 1; i >= 0; i--) if (isSep(rest[i])) { slash = i; break; }
    parsed.base = rest.slice(slash + 1);
    parsed.ext = extOf(parsed.base);
    parsed.name = parsed.ext ? parsed.base.slice(0, -parsed.ext.length) : parsed.base;
    parsed.dir = slash === -1 ? parsed.root : `${parsed.root}${rest.slice(0, slash)}`;
    return parsed;
  },
};

posix.posix = posix;
posix.win32 = win32;
win32.posix = posix;
win32.win32 = win32;
module.exports = posix;
