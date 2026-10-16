// Adapted from nodejs/node test/parallel/test-url-pathtofileurl.js and test-url-fileurltopath.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');
const url = require('node:url');

assert.strictEqual(url.pathToFileURL('/a b/c.js').href, 'file:///a%20b/c.js');
assert.strictEqual(url.fileURLToPath('file:///a%20b/c.js'), '/a b/c.js');
assert.strictEqual(url.fileURLToPath(new URL('file:///x/y')), '/x/y');
assert.throws(() => url.fileURLToPath('http://h/x'), { code: 'ERR_INVALID_URL_SCHEME' });
assert.strictEqual(url.URL, URL);
assert.strictEqual(url.URLSearchParams, URLSearchParams);
const legacy = url.parse('http://u@h.com:81/p?q=1#f', true);
assert.strictEqual(legacy.host, 'h.com:81');
assert.strictEqual(legacy.query.q, '1');
assert.strictEqual(url.format({ protocol: 'http', host: 'h.com', pathname: '/p' }), 'http://h.com/p');
