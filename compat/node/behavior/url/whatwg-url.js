// Adapted from nodejs/node test/parallel/test-whatwg-url-constructor.js. Copyright Node.js contributors. MIT license.
'use strict';
const assert = require('node:assert');

const url = new URL('https://u:p@Example.com:8080/a/./b/../c?x=1#frag');
assert.strictEqual(url.protocol, 'https:');
assert.strictEqual(url.username, 'u');
assert.strictEqual(url.password, 'p');
assert.strictEqual(url.hostname, 'example.com');
assert.strictEqual(url.port, '8080');
assert.strictEqual(url.pathname, '/a/c');
assert.strictEqual(url.hash, '#frag');
assert.strictEqual(url.origin, 'https://example.com:8080');
assert.strictEqual(new URL('../x', 'http://h/a/b/c').href, 'http://h/a/x');
assert.strictEqual(new URL('http://h:80/').port, '');
assert.throws(() => new URL('not a url'), { code: 'ERR_INVALID_URL' });
assert.strictEqual(URL.canParse('http://h'), true);
assert.strictEqual(URL.canParse('nope'), false);
