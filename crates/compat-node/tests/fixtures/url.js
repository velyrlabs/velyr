const url = node("url");
test("URL globals are re-exported", () => {
  eq(url.URL, globalThis.URL);
  eq(url.URLSearchParams, globalThis.URLSearchParams);
});
test("fileURLToPath posix", () => {
  eq(url.fileURLToPath("file:///a/b%20c.txt", { windows: false }), "/a/b c.txt");
  eq(url.fileURLToPath(new URL("file:///x/y"), { windows: false }), "/x/y");
  code(() => url.fileURLToPath("http://a/b", { windows: false }), "ERR_INVALID_URL_SCHEME", TypeError);
  code(() => url.fileURLToPath("file://host/a", { windows: false }), "ERR_INVALID_FILE_URL_HOST", TypeError);
  code(() => url.fileURLToPath("file:///a%2Fb", { windows: false }), "ERR_INVALID_FILE_URL_PATH", TypeError);
  code(() => url.fileURLToPath(1), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("fileURLToPath windows", () => {
  eq(url.fileURLToPath("file:///C:/dir/f%20x.txt", { windows: true }), "C:\\dir\\f x.txt");
  eq(url.fileURLToPath("file://server/share/f.txt", { windows: true }), "\\\\server\\share\\f.txt");
  code(() => url.fileURLToPath("file:///nodrive/f", { windows: true }), "ERR_INVALID_FILE_URL_PATH", TypeError);
});
test("pathToFileURL", () => {
  eq(url.pathToFileURL("/a/b c#d?e%.txt", { windows: false }).href, "file:///a/b%20c%23d%3Fe%25.txt");
  eq(url.pathToFileURL("C:\\dir\\f x.txt", { windows: true }).href, "file:///C:/dir/f%20x.txt");
  eq(url.pathToFileURL("\\\\server\\share\\f.txt", { windows: true }).href, "file://server/share/f.txt");
  eq(url.pathToFileURL("/tmp/dir/", { windows: false }).href, "file:///tmp/dir/");
  code(() => url.pathToFileURL(1), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("legacy parse", () => {
  const u = url.parse("http://user:pa%20ss@Host.com:8080/p/a/t/h?query=string#hash");
  eq(u.protocol, "http:");
  eq(u.slashes, true);
  eq(u.auth, "user:pa ss");
  eq(u.host, "host.com:8080");
  eq(u.port, "8080");
  eq(u.hostname, "host.com");
  eq(u.hash, "#hash");
  eq(u.search, "?query=string");
  eq(u.query, "query=string");
  eq(u.pathname, "/p/a/t/h");
  eq(u.path, "/p/a/t/h?query=string");
  const q = url.parse("/a?x=1&x=2&y=3", true);
  same(q.query, { x: ["1", "2"], y: "3" });
  eq(q.pathname, "/a");
  eq(q.host, null);
  eq(url.parse("http://a.com").pathname, "/");
  code(() => url.parse(1), "ERR_INVALID_ARG_TYPE", TypeError);
});
test("legacy format", () => {
  eq(url.format({ protocol: "https", hostname: "x.com", pathname: "/p", query: { a: "1", b: ["2", "3"] } }), "https://x.com/p?a=1&b=2&b=3");
  eq(url.format({ protocol: "http", host: "x.com:81", pathname: "a", hash: "h" }), "http://x.com:81/a#h");
  eq(url.format("http://a.com/b?c=d"), "http://a.com/b?c=d");
  eq(url.format(new URL("http://u:p@a.com/b?c#d"), { fragment: false, auth: false }), "http://a.com/b?c");
  eq(url.format(url.parse("http://a.com:80/x?y#z")), "http://a.com:80/x?y#z");
});
test("legacy resolve", () => {
  eq(url.resolve("/one/two/three", "four"), "/one/two/four");
  eq(url.resolve("http://example.com/one", "/two"), "http://example.com/two");
  eq(url.resolve("http://example.com/a/b", "../c"), "http://example.com/c");
});
test("urlToHttpOptions", () => {
  const o = url.urlToHttpOptions(new URL("https://u:p%40@[::1]:8443/a?b#c"));
  eq(o.hostname, "::1");
  eq(o.port, 8443);
  eq(o.auth, "u:p@");
  eq(o.path, "/a?b");
  eq(o.protocol, "https:");
});
finish();
