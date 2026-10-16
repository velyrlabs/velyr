// Velyr.Command on top of the private process ops. Evaluated once per isolate, after the runtime-ops
// bootstrap created `Velyr` and after web-streams installed its internal native hooks.
//
//   const command = new Velyr.Command("git", { args: ["status"], cwd: "/repo", env: { LANG: "C" } });
//   const { success, code, stdout } = await command.output();   // stdout/stderr piped, bounded by maxBuffer
//   const child = command.spawn();                              // streams, status, kill()
//
// The child's environment is exactly `env` (default: empty); nothing is inherited. Spawning needs the
// `process-spawn` permission for the program, by name or by the path it resolves to. Piped streams
// are read on demand (the child blocks on a full pipe), and a child's resource is released when it
// has exited and its piped output has been read to the end (or cancelled): consume or cancel what
// you pipe.
(function (global) {
  "use strict";

  const OPS = ["spawn", "read", "write", "close_stdin", "wait", "kill", "collect", "close"];
  const op = {};
  for (const name of OPS) {
    op[name] = global[`op_command_${name}`];
    delete global[`op_command_${name}`];
  }
  const { createNativeReadable, createNativeWritable } = global[Symbol.for("velyr.internal.streams")];

  const READ_CHUNK = 16 * 1024;
  const DEFAULT_MAX_BUFFER = 1024 * 1024;
  const MAX_BUFFER_LIMIT = 64 * 1024 * 1024; // keep in step with io-process' MAX_BUFFER_LIMIT
  const MODES = new Set(["piped", "inherit", "null"]);
  const SIGNALS = { 1: "SIGHUP", 2: "SIGINT", 3: "SIGQUIT", 6: "SIGABRT", 9: "SIGKILL", 13: "SIGPIPE", 14: "SIGALRM", 15: "SIGTERM" };
  const collect = Symbol("collect");
  const STANDARD = { spawn: ["inherit", "inherit", "inherit"], output: ["null", "piped", "piped"] };

  function unsupported(message) {
    return Object.assign(new Error(message), { code: "VELYR_UNSUPPORTED" });
  }

  function validate(program, options) {
    if (typeof program !== "string" || program === "") throw new TypeError("Velyr.Command: program must be a non-empty string");
    if (options === null || typeof options !== "object") throw new TypeError("Velyr.Command: options must be an object");
    const { args = [], cwd, env = {}, stdin, stdout, stderr, maxBuffer = DEFAULT_MAX_BUFFER } = options;
    if (!Array.isArray(args) || args.some((arg) => typeof arg !== "string")) throw new TypeError("Velyr.Command: args must be an array of strings");
    if (cwd !== undefined && typeof cwd !== "string") throw new TypeError("Velyr.Command: cwd must be a string");
    if (env === null || typeof env !== "object") throw new TypeError("Velyr.Command: env must be an object");
    const pairs = Object.entries(env).map(([name, value]) => {
      if (typeof value !== "string") throw new TypeError(`Velyr.Command: env.${name} must be a string`);
      return [name, value];
    });
    for (const [key, mode] of [["stdin", stdin], ["stdout", stdout], ["stderr", stderr]]) {
      if (mode !== undefined && !MODES.has(mode)) throw new TypeError(`Velyr.Command: ${key} must be "piped", "inherit" or "null"`);
    }
    if (!Number.isInteger(maxBuffer) || maxBuffer < 0) throw new TypeError("Velyr.Command: maxBuffer must be a non-negative integer");
    if (maxBuffer > MAX_BUFFER_LIMIT) {
      throw Object.assign(new RangeError(`Velyr.Command: maxBuffer must be at most ${MAX_BUFFER_LIMIT} bytes (64 MiB)`), { code: "ERR_OUT_OF_RANGE" });
    }
    return { program, args: [...args], cwd, env: pairs, stdin, stdout, stderr, maxBuffer };
  }

  const describe = (exit) => Object.freeze({
    success: exit.code === 0,
    code: exit.code,
    signal: exit.signal === null ? null : (SIGNALS[exit.signal] ?? String(exit.signal)),
  });

  class ChildProcess {
    #id;
    #live;
    #modes;
    #status;
    #stdin;
    #stdout;
    #stderr;

    constructor({ id, pid }, modes) {
      this.#id = id;
      this.#modes = modes;
      this.#live = 1 + (modes[1] === "piped") + (modes[2] === "piped");
      Object.defineProperty(this, "pid", { value: pid, enumerable: true });
      this.#status = op.wait(id).then((json) => {
        this.#release();
        return describe(JSON.parse(json));
      });
    }

    #release() {
      if (--this.#live === 0) {
        try {
          op.close(this.#id);
        } catch {
          // Already closed.
        }
      }
    }

    #output(index) {
      const which = index;
      let finished = false;
      const done = () => {
        if (!finished) {
          finished = true;
          this.#release();
        }
      };
      return createNativeReadable(
        this.#id,
        async (id) => {
          const chunk = await op.read(id, which, READ_CHUNK);
          if (chunk === null) done();
          return chunk;
        },
        // Cancelling stops reading; a child that keeps writing will block on the full pipe.
        () => done(),
      );
    }

    get stdin() {
      if (this.#modes[0] !== "piped") return null;
      return (this.#stdin ??= createNativeWritable(
        this.#id,
        (id, chunk) => op.write(id, chunk),
        (id) => {
          try {
            op.close_stdin(id);
          } catch {
            // The child is gone.
          }
        },
      ));
    }

    get stdout() {
      if (this.#modes[1] !== "piped") return null;
      return (this.#stdout ??= this.#output(1));
    }

    get stderr() {
      if (this.#modes[2] !== "piped") return null;
      return (this.#stderr ??= this.#output(2));
    }

    get status() {
      return this.#status;
    }

    // Collects a piped stream (bounded) for Command.output(); empty when it was not piped.
    async [collect](index, maxBuffer) {
      if (this.#modes[index] !== "piped") return new Uint8Array(0);
      try {
        return new Uint8Array(await op.collect(this.#id, index, maxBuffer));
      } finally {
        this.#release();
      }
    }

    // Only SIGKILL exists: the host cannot deliver other signals.
    kill(signal = "SIGKILL") {
      if (signal !== "SIGKILL") throw unsupported(`Velyr child.kill: only SIGKILL is supported, got ${String(signal)}`);
      try {
        op.kill(this.#id);
      } catch {
        // Already released: the child ended.
      }
    }
  }

  class Command {
    #options;

    constructor(program, options = {}) {
      this.#options = Object.freeze(validate(program, options));
    }

    #launch(kind) {
      const { program, args, cwd, env, maxBuffer: _, ...stdio } = this.#options;
      const [defIn, defOut, defErr] = STANDARD[kind];
      const modes = [stdio.stdin ?? defIn, stdio.stdout ?? defOut, stdio.stderr ?? defErr];
      const info = JSON.parse(op.spawn(JSON.stringify({
        program, args, cwd: cwd ?? null, env, stdin: modes[0], stdout: modes[1], stderr: modes[2],
      })));
      return new ChildProcess(info, modes);
    }

    spawn() {
      return this.#launch("spawn");
    }

    async output() {
      const child = this.#launch("output");
      const { maxBuffer } = this.#options;
      const [stdout, stderr] = await Promise.all([child[collect](1, maxBuffer), child[collect](2, maxBuffer)]);
      return Object.freeze({ ...(await child.status), stdout, stderr });
    }
  }

  Object.defineProperty(global.Velyr, "Command", {
    value: Command,
    writable: false,
    configurable: true,
    enumerable: true,
  });
})(globalThis);
