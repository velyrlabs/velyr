const u = new URL("../a?b=1#c", "http://x.com/p/q");
u.searchParams.append("k", "v w");
console.log(u.href);
console.log(String(URL.canParse("nope")));
