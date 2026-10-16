//! `node:worker_threads` over the web `Worker` of runtime-workers, on V8 under the real event
//! loop. A worker entry can be ESM or CommonJS. Every run is bounded by the harness watchdog.

mod worker_support;

use worker_support::Fixture;

const PRELUDE: &str = "
import * as wt from 'node:worker_threads';
const { Worker, MessageChannel, BroadcastChannel } = wt;
const log = (value) => record(typeof value === 'string' ? value : JSON.stringify(value));
const once = (emitter, name) => new Promise((resolve) => emitter.once(name, (...args) => resolve(args)));
const codeOf = (fn) => { try { fn(); return 'no error'; } catch (e) { return e.code ?? String(e); } };
";

/// Runs `body` as an async function in `main.mjs`, after the prelude, and returns its log.
fn run(fx: &Fixture, body: &str) -> Vec<String> {
    let script = format!(
        "{PRELUDE}\ntry {{ await (async () => {{\n{body}\n}})(); }} catch (e) {{ log('FAIL ' + (e && e.stack || e)); }}\n"
    );
    fx.write("main.mjs", &script);
    let log = fx.run().unwrap_or_else(|e| panic!("run failed: {e}"));
    assert!(
        !log.iter().any(|line| line.starts_with("FAIL")),
        "script failed: {log:?}"
    );
    log
}

#[test]
fn raw_web_worker_round_trips_in_the_harness() {
    let fx = Fixture::new();
    fx.write("echo.mjs", "self.onmessage = (e) => self.postMessage('esm ' + e.data);");
    fx.write("echo.cjs", "self.onmessage = (e) => self.postMessage('cjs ' + e.data);");
    // No node:worker_threads here: this proves the harness before the module exists.
    fx.write(
        "main.mjs",
        &format!(
            "for (const file of ['echo.mjs', 'echo.cjs']) {{
               const w = new Worker('file://{}/' + file, {{ type: 'module' }});
               const reply = new Promise((resolve) => {{ w.onmessage = (e) => resolve(e.data); }});
               w.postMessage('hi');
               record(await reply);
               w.terminate();
             }}",
            fx.dir.display()
        ),
    );
    assert_eq!(fx.run().unwrap(), ["esm hi", "cjs hi"]);
}

const ESM_WORKER: &str = "
import { parentPort, workerData, isMainThread, threadId } from 'node:worker_threads';
parentPort.on('message', (m) => {
  workerData.list.push(99);
  parentPort.postMessage({ echo: m, data: workerData, isMainThread, threadId: threadId > 0 });
});
";

const CJS_WORKER: &str = "
const { parentPort, workerData, isMainThread, threadId } = require('node:worker_threads');
parentPort.on('message', (m) => {
  workerData.list.push(99);
  parentPort.postMessage({ echo: m, data: workerData, isMainThread, threadId: threadId > 0 });
});
";

fn exchange(file: &str, source: &str) {
    let fx = Fixture::new();
    fx.write(file, source);
    let log = run(
        &fx,
        &format!(
            "const data = {{ n: 1, list: [1, 2], when: new Date(0) }};
             const w = new Worker('{}/{file}', {{ workerData: data }});
             data.n = 2; data.list.push(3);
             w.postMessage('ping');
             const [reply] = await once(w, 'message');
             log({{ echo: reply.echo, n: reply.data.n, list: reply.data.list,
                    date: reply.data.when instanceof Date, main: reply.isMainThread, id: reply.threadId }});
             log({{ parentList: data.list }});
             const [code] = await Promise.all([once(w, 'exit'), w.terminate()]);
             log('exit ' + typeof code[0]);",
            fx.dir.display()
        ),
    );
    assert_eq!(
        log,
        [
            "{\"echo\":\"ping\",\"n\":1,\"list\":[1,2,99],\"date\":true,\"main\":false,\"id\":true}",
            "{\"parentList\":[1,2,3]}",
            "exit number",
        ]
    );
}

#[test]
fn esm_worker_exchanges_messages_and_clones_worker_data() {
    exchange("worker.mjs", ESM_WORKER);
}

#[test]
fn cjs_worker_exchanges_messages_and_clones_worker_data() {
    exchange("worker.cjs", CJS_WORKER);
}

#[test]
fn main_thread_facts() {
    let fx = Fixture::new();
    let log = run(
        &fx,
        "log({ main: wt.isMainThread, id: wt.threadId, port: wt.parentPort, data: wt.workerData ?? null,
               limits: wt.resourceLimits });",
    );
    assert_eq!(
        log,
        ["{\"main\":true,\"id\":0,\"port\":null,\"data\":null,\"limits\":{}}"]
    );
}

#[test]
fn terminate_stops_a_busy_worker_and_resolves() {
    let fx = Fixture::new();
    fx.write(
        "busy.mjs",
        "import { parentPort } from 'node:worker_threads'; parentPort.postMessage('ready'); for (;;) {}",
    );
    let log = run(
        &fx,
        &format!(
            "const w = new Worker('{}/busy.mjs');
             await once(w, 'message');
             const exit = once(w, 'exit');
             const code = await w.terminate();
             log('terminate ' + typeof code);
             log('exit ' + typeof (await exit)[0]);",
            fx.dir.display()
        ),
    );
    assert_eq!(log, ["terminate number", "exit number"]);
}

#[test]
fn a_worker_that_finishes_emits_exit_zero() {
    let fx = Fixture::new();
    fx.write("quick.mjs", "import 'node:worker_threads'; record('ran');");
    let log = run(
        &fx,
        &format!(
            "const w = new Worker('{}/quick.mjs');
             log('exit ' + (await once(w, 'exit'))[0]);",
            fx.dir.display()
        ),
    );
    assert_eq!(log, ["ran", "exit 0"]);
}

#[test]
fn an_uncaught_worker_error_reaches_the_parent_then_exit_one() {
    let fx = Fixture::new();
    fx.write(
        "boom.mjs",
        "import 'node:worker_threads'; throw new Error('boom');",
    );
    let log = run(
        &fx,
        &format!(
            "const w = new Worker('{}/boom.mjs');
             const [error] = await once(w, 'error');
             log('error ' + error.message);
             log('exit ' + (await once(w, 'exit'))[0]);",
            fx.dir.display()
        ),
    );
    assert_eq!(log, ["error boom", "exit 1"]);
}

#[test]
fn process_exit_code_reaches_the_parent() {
    let fx = Fixture::new();
    fx.write("exit7.mjs", "process.exit(7); record('unreachable');");
    let log = run(
        &fx,
        &format!(
            "const w = new Worker('{}/exit7.mjs');
             log('exit ' + (await once(w, 'exit'))[0]);",
            fx.dir.display()
        ),
    );
    assert_eq!(log, ["exit 7"]);
}

#[test]
fn a_heap_limit_ends_the_worker_with_out_of_memory_and_the_parent_goes_on() {
    let fx = Fixture::new();
    fx.write(
        "hog.mjs",
        "import { resourceLimits } from 'node:worker_threads';
         record('limit ' + resourceLimits.maxOldGenerationSizeMb);
         const keep = []; for (;;) keep.push(new Array(100000).fill(1));",
    );
    let log = run(
        &fx,
        &format!(
            "const w = new Worker('{}/hog.mjs', {{ resourceLimits: {{ maxOldGenerationSizeMb: 16 }} }});
             const [error] = await once(w, 'error');
             log('error ' + error.code);
             log('exit ' + (await once(w, 'exit'))[0]);
             log('alive');",
            fx.dir.display()
        ),
    );
    assert_eq!(
        log,
        ["limit 16", "error ERR_WORKER_OUT_OF_MEMORY", "exit 1", "alive"]
    );
}

#[test]
fn unsupported_options_fail_loudly_and_eval_is_off() {
    let fx = Fixture::new();
    fx.write("noop.mjs", "");
    let log = run(
        &fx,
        &format!(
            "const file = '{}/noop.mjs';
             log(codeOf(() => new Worker(file, {{ env: wt.SHARE_ENV }})));
             log(codeOf(() => new Worker('1 + 1', {{ eval: true }})));
             log(codeOf(() => new Worker(file, {{ resourceLimits: {{ maxYoungGenerationSizeMb: 4 }} }})));
             log(codeOf(() => new Worker(file, {{ transferList: [] , stdout: true }})));
             log(codeOf(() => new Worker('http://example.com/x.mjs')));
             try {{ new Worker(file, {{ workerData: () => 1 }}); log('no error'); }} catch (e) {{ log(e.name); }}
             log(codeOf(() => wt.receiveMessageOnPort(new MessageChannel().port1)));",
            fx.dir.display()
        ),
    );
    assert_eq!(
        log,
        [
            "ERR_VELYR_UNSUPPORTED",
            "ERR_VELYR_UNSUPPORTED",
            "ERR_VELYR_UNSUPPORTED",
            "ERR_VELYR_UNSUPPORTED",
            "ERR_INVALID_ARG_VALUE",
            "DataCloneError",
            "ERR_VELYR_UNSUPPORTED",
        ]
    );
}

#[test]
fn message_ports_speak_node_events_and_cross_into_a_worker() {
    let fx = Fixture::new();
    fx.write(
        "port.mjs",
        "import { parentPort } from 'node:worker_threads';
         parentPort.once('message', ({ port }) => { port.on('message', (m) => { port.postMessage(m * 2); port.close(); }); });",
    );
    let log = run(
        &fx,
        &format!(
            "const local = new MessageChannel();
             local.port2.on('message', (m) => log('local ' + m));
             local.port1.postMessage(1);
             await new Promise((resolve) => setTimeout(resolve, 20));
             local.port1.close();

             const {{ port1, port2 }} = new MessageChannel();
             const w = new Worker('{}/port.mjs');
             w.postMessage({{ port: port2 }}, [port2]);
             port1.postMessage(21);
             const [answer] = await once(port1, 'message');
             log('worker ' + answer);
             await once(port1, 'close');
             await w.terminate();",
            fx.dir.display()
        ),
    );
    assert_eq!(log, ["local 1", "worker 42"]);
}

#[test]
fn broadcast_channel_reaches_other_channels_of_the_same_name() {
    let fx = Fixture::new();
    let log = run(
        &fx,
        "const a = new BroadcastChannel('wt');
         const b = new BroadcastChannel('wt');
         const got = new Promise((resolve) => { b.onmessage = (e) => resolve(e.data); });
         a.postMessage({ hello: 1 });
         log(await got);
         a.close(); b.close();",
    );
    assert_eq!(log, ["{\"hello\":1}"]);
}

#[test]
fn name_and_environment_data_reach_the_worker() {
    let fx = Fixture::new();
    fx.write(
        "env.mjs",
        "import { parentPort, getEnvironmentData } from 'node:worker_threads';
         parentPort.postMessage({ name: self.name, shared: getEnvironmentData('key') });",
    );
    let log = run(
        &fx,
        &format!(
            "wt.setEnvironmentData('key', {{ deep: [1] }});
             const w = new Worker('{}/env.mjs', {{ name: 'mine' }});
             log((await once(w, 'message'))[0]);
             await w.terminate();",
            fx.dir.display()
        ),
    );
    assert_eq!(log, ["{\"name\":\"mine\",\"shared\":{\"deep\":[1]}}"]);
}
