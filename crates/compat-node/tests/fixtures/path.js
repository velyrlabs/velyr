const path = node('path');
const posix = path.posix;
const win32 = path.win32;

test('identity and constants', () => {
  eq(path, posix); eq(posix.posix, posix); eq(win32.win32, win32);
  eq(posix.sep, '/'); eq(posix.delimiter, ':'); eq(win32.sep, '\\'); eq(win32.delimiter, ';');
  eq(node('path/posix'), posix); eq(node('path/win32'), win32);
});
test('posix.join', () => {
  eq(posix.join('/foo', 'bar', 'baz/asdf', 'quux', '..'), '/foo/bar/baz/asdf');
  eq(posix.join(''), '.'); eq(posix.join('a', '', 'b'), 'a/b'); eq(posix.join('..', 'x'), '../x');
  eq(posix.join('a/', '/b'), 'a/b'); eq(posix.join('/'), '/');
});
test('posix.normalize', () => {
  eq(posix.normalize('/foo/bar//baz/asdf/quux/..'), '/foo/bar/baz/asdf');
  eq(posix.normalize(''), '.'); eq(posix.normalize('./a/../..'), '..');
  eq(posix.normalize('a/b/'), 'a/b/'); eq(posix.normalize('/..'), '/'); eq(posix.normalize('../../a'), '../../a');
});
test('posix.resolve', () => {
  eq(posix.resolve('/foo/bar', './baz'), '/foo/bar/baz');
  eq(posix.resolve('/foo/bar', '/tmp/file/'), '/tmp/file');
  eq(posix.resolve('/a', '..', '..'), '/');
  eq(posix.resolve('/a/b', '../c', 'd'), '/a/c/d');
});
test('posix.relative', () => {
  eq(posix.relative('/data/orandea/test/aaa', '/data/orandea/impl/bbb'), '../../impl/bbb');
  eq(posix.relative('/a', '/a'), ''); eq(posix.relative('/a/b', '/a/b/c/d'), 'c/d'); eq(posix.relative('/a/b/c', '/a'), '../..');
});
test('posix.dirname/basename/extname', () => {
  eq(posix.dirname('/a/b/c'), '/a/b'); eq(posix.dirname('/a'), '/'); eq(posix.dirname('a'), '.');
  eq(posix.dirname(''), '.'); eq(posix.dirname('/a/b/'), '/a');
  eq(posix.basename('/foo/bar/baz/asdf/quux.html'), 'quux.html');
  eq(posix.basename('/foo/bar/baz/asdf/quux.html', '.html'), 'quux');
  eq(posix.basename('/a/b/'), 'b'); eq(posix.basename('aaa/bbb', 'bbb'), 'bbb'); eq(posix.basename(''), '');
  eq(posix.extname('index.html'), '.html'); eq(posix.extname('index.coffee.md'), '.md');
  eq(posix.extname('index.'), '.'); eq(posix.extname('index'), ''); eq(posix.extname('.index'), '');
  eq(posix.extname('.index.md'), '.md'); eq(posix.extname('a/.b'), '');
});
test('posix.isAbsolute/parse/format', () => {
  eq(posix.isAbsolute('/a'), true); eq(posix.isAbsolute('a'), false); eq(posix.isAbsolute(''), false);
  same(posix.parse('/home/user/dir/file.txt'), { root: '/', dir: '/home/user/dir', base: 'file.txt', ext: '.txt', name: 'file' });
  same(posix.parse('file'), { root: '', dir: '', base: 'file', ext: '', name: 'file' });
  eq(posix.format({ dir: '/home/user/dir', base: 'file.txt' }), '/home/user/dir/file.txt');
  eq(posix.format({ root: '/', base: 'f.txt' }), '/f.txt');
  eq(posix.format({ name: 'f', ext: 'txt' }), 'f.txt');
  eq(posix.toNamespacedPath('/a'), '/a');
});
test('invalid arguments carry Node codes', () => {
  const e = code(() => posix.join(1), 'ERR_INVALID_ARG_TYPE', TypeError);
  eq(e.message, 'The "path" argument must be of type string. Received type number (1)');
  code(() => posix.basename('a', 1), 'ERR_INVALID_ARG_TYPE', TypeError);
  code(() => posix.format(null), 'ERR_INVALID_ARG_TYPE', TypeError);
});
test('win32.resolve', () => {
  eq(win32.resolve('C:\\foo', 'bar'), 'C:\\foo\\bar');
  eq(win32.resolve('C:\\a', 'D:\\b'), 'D:\\b');
  eq(win32.resolve('C:\\a', '\\b'), 'C:\\b');
  eq(win32.resolve('C:/a/b', '../c'), 'C:\\a\\c');
  eq(win32.resolve('C:\\a\\b', '..', '..', '..'), 'C:\\');
  eq(win32.resolve('\\\\server\\share\\x', '..', 'y'), '\\\\server\\share\\y');
});
test('win32.join/normalize', () => {
  eq(win32.join('C:\\a', '..', 'b'), 'C:\\b'); eq(win32.join('a', 'b'), 'a\\b');
  eq(win32.join('//server', 'share', 'x'), '\\\\server\\share\\x');
  eq(win32.normalize('C:/a//b/../c'), 'C:\\a\\c'); eq(win32.normalize('C:'), 'C:.');
  eq(win32.normalize('C:..\\x'), 'C:..\\x'); eq(win32.normalize('a/b/'), 'a\\b\\');
});
test('win32.isAbsolute/basename/dirname/extname', () => {
  eq(win32.isAbsolute('C:\\a'), true); eq(win32.isAbsolute('C:a'), false); eq(win32.isAbsolute('\\a'), true);
  eq(win32.isAbsolute('//s/x'), true); eq(win32.isAbsolute('C:'), false);
  eq(win32.basename('C:\\foo\\bar.txt'), 'bar.txt'); eq(win32.basename('C:\\foo\\bar.txt', '.txt'), 'bar');
  eq(win32.basename('C:'), ''); eq(win32.dirname('C:\\a\\b'), 'C:\\a'); eq(win32.dirname('C:\\'), 'C:\\');
  eq(win32.dirname('C:'), 'C:'); eq(win32.dirname('\\\\server\\share\\x'), '\\\\server\\share\\');
  eq(win32.extname('a\\b.c'), '.c');
});
test('win32.relative/parse/format/namespaced', () => {
  eq(win32.relative('C:\\orandea\\test\\aaa', 'C:\\orandea\\impl\\bbb'), '..\\..\\impl\\bbb');
  eq(win32.relative('C:\\a', 'D:\\b'), 'D:\\b'); eq(win32.relative('C:\\A\\b', 'c:\\a\\B\\c'), 'c');
  same(win32.parse('C:\\path\\dir\\file.txt'), { root: 'C:\\', dir: 'C:\\path\\dir', base: 'file.txt', ext: '.txt', name: 'file' });
  eq(win32.format({ root: 'C:\\', dir: 'C:\\path\\dir', base: 'file.txt' }), 'C:\\path\\dir\\file.txt');
  eq(win32.toNamespacedPath('C:\\a'), '\\\\?\\C:\\a');
  eq(win32.toNamespacedPath('\\\\server\\share\\x'), '\\\\?\\UNC\\server\\share\\x');
});
finish();
