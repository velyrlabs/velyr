// Lazy registry for the Tier 1 `node:` builtins. Written for this runtime; no Node source is copied.
(function (global) {
  "use strict";

  const factories = {
/*FACTORIES*/
  };
  const publicNames = new Set([/*PUBLIC*/]);
  const cache = Object.create(null);

  // Byte ops for Buffer/string_decoder: captured, then removed from the global (ADR-010). Hosts
  // that skip compat_node_host_functions() get a clear error on first use, not at install.
  const missing = (what) => () => { throw new Error(`velyr compat-node: register the host functions for ${what}`); };
  const ops = {
    encode: global.op_buffer_encode || missing("Buffer (compat_node_host_functions)"),
    decode: global.op_buffer_decode || missing("Buffer (compat_node_host_functions)"),
    byteLength: global.op_buffer_byte_length || missing("Buffer (compat_node_host_functions)"),
  };
  const opNames = ["op_buffer_encode", "op_buffer_decode", "op_buffer_byte_length"];
  // process / env / setImmediate ops: same capture-and-delete, same clear error when absent.
  for (const [key, name] of Object.entries({
    processInfo: "op_process_info", envGet: "op_process_env_get", envHas: "op_process_env_has",
    envSet: "op_process_env_set", envDelete: "op_process_env_delete", envKeys: "op_process_env_keys",
    exit: "op_process_exit", cwd: "op_process_cwd", chdir: "op_process_chdir",
    hrtime: "op_process_hrtime", uptime: "op_process_uptime", memory: "op_process_memory",
    write: "op_process_write", immediateSet: "op_immediate_set", immediateClear: "op_immediate_clear",
    immediateRef: "op_immediate_ref", tickFlags: "op_process_tick_flags",
    ctxSet: "op_async_context_set", ctxGet: "op_async_context_get",
    cryptoSync: "op_node_crypto", cryptoAsync: "op_node_crypto_async",
    fsSync: "op_node_fs", fsAsync: "op_node_fs_async",
    os: "op_node_os", zlibSync: "op_node_zlib", zlibAsync: "op_node_zlib_async",
    dnsLookup: "op_node_dns_lookup", dnsQuery: "op_node_dns_query", childRunSync: "op_node_child_run_sync",
    httpListen: "op_node_http_listen", httpAccept: "op_node_http_accept", httpTake: "op_node_http_take",
    httpRead: "op_node_http_read", httpRespond: "op_node_http_respond", httpWrite: "op_node_http_write",
    httpEnd: "op_node_http_end", httpFail: "op_node_http_fail", httpShutdown: "op_node_http_shutdown",
    httpClose: "op_node_http_close", httpDisconnected: "op_node_http_disconnected",
    httpUpgrade: "op_node_http_upgrade", httpUpReady: "op_node_http_up_ready",
    httpUpRead: "op_node_http_up_read", httpUpWrite: "op_node_http_up_write",
    httpUpShutdown: "op_node_http_up_shutdown",
    httpClientNew: "op_node_http_client_new", httpClientClose: "op_node_http_client_close",
    httpReqBegin: "op_node_http_creq_begin", httpReqWrite: "op_node_http_creq_write",
    httpReqEnd: "op_node_http_creq_end", httpReqResponse: "op_node_http_creq_response",
    httpReqRead: "op_node_http_creq_read", httpReqClose: "op_node_http_creq_close",
    worker: "op_node_worker", v8: "op_node_v8",
  })) {
    ops[key] = global[name] || missing("process (compat_node_host_functions)");
    opNames.push(name);
  }
  for (const name of opNames) delete global[name];
  cache["internal/ops"] = { exports: ops };

  function load(name) {
    const cached = cache[name];
    if (cached) return cached.exports;
    const module = { exports: {} };
    cache[name] = module;
    factories[name].call(module.exports, module, module.exports, load);
    return module.exports;
  }

  const api = Object.freeze({
    has: (name) => publicNames.has(name),
    names: () => [...publicNames],
    require(name) {
      if (!publicNames.has(name)) throw new Error(`No such builtin module: ${name}`);
      return load(name);
    },
  });
  Object.defineProperty(global, Symbol.for("velyr.node"), { value: api });

  // `require('node:x')`: compat-commonjs strips the prefix and looks the name up here.
  const commonjs = global[Symbol.for("velyr.commonjs")];
  if (commonjs) {
    for (const name of publicNames) commonjs.builtins.set(name, load(name));
    // Same rule as the ESM resolver (`builtin_name`): a bare `test` is an npm package.
    commonjs.prefixOnly.add("test");
  }
})(globalThis);
