async function code(fn) {
  try {
    await fn();
    return "allowed";
  } catch (error) {
    return error.code ?? `no-code:${error.message}`;
  }
}

const results = {
  readFile: await code(() => Velyr.readTextFile("fixture.txt")),
  stat: await code(() => Velyr.stat("fixture.txt")),
  writeFile: await code(() => Velyr.writeTextFile("written.txt", "x")),
  envGet: await code(() => Velyr.env.get("HOME")),
  envHas: await code(() => Velyr.env.has("HOME")),
  envToObject: await code(() => Velyr.env.toObject()),
  envSet: await code(() => Velyr.env.set("X", "1")),
  importOutside: await code(() => import("../outside.js")),
  importAbsolute: await code(() => import("/etc/hosts")),
  outsideLoaded: globalThis.outsideLoaded === true,
};

const absent = [
  "XMLHttpRequest", "EventSource", "Deno", "Bun", "WebAssembly.instantiateStreaming",
];
for (const name of absent) {
  results[`typeof ${name}`] = typeof name.split(".").reduce((o, k) => o?.[k], globalThis);
}

const velyrApis = [
  "spawn", "run", "dlopen", "openKv", "resolveDns", "Worker", "dlopenSync", "ffi", "net", "http",
];
for (const name of velyrApis) {
  results[`typeof Velyr.${name}`] = typeof Velyr[name];
}

// APIs that ship: without flags every call that reaches the network must be denied.
// `Velyr.startTls` needs a connection, which cannot be had without `connect`, so it has no call here.
// A throwaway self-signed pair, so the call reaches the permission check instead of failing on parsing.
const CERT = "-----BEGIN CERTIFICATE-----\nMIIBXjCCAQSgAwIBAgIUBtJOoBGMmh+X9rKMb86R6gbXnbMwCgYIKoZIzj0EAwIw\nFDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MTAxNjA2MzEyN1oYDzIxMjYwOTIy\nMDYzMTI3WjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjO\nPQMBBwNCAAQylWILGesmC0xoc8/zcFj6fF0yITELtyGbEbYmXomc+oVol0UpWrVX\nZwK9JXpa2ecEzzDQSCe5aauOacAhWFUSozIwMDAdBgNVHQ4EFgQUn3KhaFTZNfPm\nRC61rQCsBGNWoNQwDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNIADBFAiBU\n68I4Xi1oro19PpaW37BVvwGl64o1AR3I1fSEO5Y4KAIhAJ7CnSMkkNxmmcGSroYB\n7r6q053DoD+qOU6icfvw2J7/\n-----END CERTIFICATE-----\n";
const KEY = "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgxt08SoWWn81tZNwo\nMhPpU5XTiiJIcQ633E238mWU9dShRANCAAQylWILGesmC0xoc8/zcFj6fF0yITEL\ntyGbEbYmXomc+oVol0UpWrVXZwK9JXpa2ecEzzDQSCe5aauOacAhWFUS\n-----END PRIVATE KEY-----\n";
const calls = {
  // `process` ships in the node and full profiles; `process.env` still needs `--allow-env`.
  processEnv: () => process.env.HOME,
  fetch: () => fetch("http://127.0.0.1:9/"),
  // A denied dial fails like any network failure: `error`, then `close` 1006, never `open`.
  WebSocket: () => new Promise((resolve, reject) => {
    const ws = new WebSocket("ws://127.0.0.1:9/");
    ws.onopen = () => resolve("allowed");
    ws.onclose = (event) => (event.code === 1006 ? reject({ code: "closed-without-open" }) : resolve("closed"));
  }),
  connect: () => Velyr.connect({ hostname: "127.0.0.1", port: 9 }),
  listen: () => Velyr.listen({ hostname: "127.0.0.1", port: 0 }),
  serve: () => Velyr.serve({ hostname: "127.0.0.1", port: 0 }, () => new Response("x")),
  connectTls: () => Velyr.connectTls({ hostname: "127.0.0.1", port: 9 }),
  // Ships, but spawning needs `--allow-run`.
  Command: () => new Velyr.Command("true").output(),
  listenTls: () => Velyr.listenTls({ hostname: "127.0.0.1", port: 0, cert: CERT, key: KEY }),
};
for (const [name, call] of Object.entries(calls)) {
  results[`call ${name}`] = await code(call);
}

console.log(JSON.stringify(results));
