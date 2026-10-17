//! `node:readline`, `node:readline/promises` and `node:vm`, through `common::script` on V8.

mod common;

use common::script::{CHECKS, assert_ok, run};
use velyr_compat_node::BUILTINS;

const PRELUDE: &str = r#"
const req = (name) => globalThis[Symbol.for('velyr.node')].require(name);
const code = (fn, expected) => { try { fn(); } catch (e) { eq(e.code, expected, 'error code'); return e; } throw new Error('did not throw ' + expected); };
const { Readable, Writable } = req('stream');
const sink = () => { const out = []; const w = new Writable({ write(c, e, cb) { out.push(String(c)); cb(); } }); w.out = out; return w; };
"#;

fn script(body: &str) {
    assert_ok(run(&format!("{CHECKS}{PRELUDE}{body}")));
}

#[test]
fn readline_emits_line_events_and_close_over_a_readable() {
    script(
        r#"
        const readline = req('readline');
        const input = Readable.from(['one\ntw', 'o\r\nthree\r', '\nlast']);
        const rl = readline.createInterface({ input });
        const lines = [];
        rl.on('line', (l) => lines.push(l));
        await new Promise((r) => rl.on('close', r));
        same(lines, ['one', 'two', 'three', 'last']);
    "#,
    );
}

#[test]
fn readline_async_iterator_yields_each_line() {
    script(
        r#"
        const { createInterface } = req('readline');
        const rl = createInterface({ input: Readable.from(['a\nb\n', 'c\n']), crlfDelay: Infinity });
        const lines = [];
        for await (const line of rl) lines.push(line);
        same(lines, ['a', 'b', 'c']);
    "#,
    );
}

#[test]
fn readline_question_writes_the_query_and_answers_once() {
    script(
        r#"
        const { createInterface } = req('readline');
        const output = sink();
        const rl = createInterface({ input: Readable.from(['yes\nignored\n']), output });
        const answer = await new Promise((r) => rl.question('Continue? ', r));
        eq(answer, 'yes');
        same(output.out, ['Continue? ']);
        rl.close();
    "#,
    );
}

#[test]
fn readline_promises_question_resolves_with_the_answer() {
    script(
        r#"
        const promises = req('readline/promises');
        eq(req('readline').promises, promises);
        const output = sink();
        const rl = promises.createInterface({ input: Readable.from(['42\n']), output });
        eq(await rl.question('n? '), '42');
        rl.close();
    "#,
    );
}

#[test]
fn readline_close_is_idempotent_and_blocks_question() {
    script(
        r#"
        const { createInterface } = req('readline');
        const rl = createInterface({ input: new Readable({ read() {} }) });
        let closes = 0;
        rl.on('close', () => closes++);
        rl.close();
        rl.close();
        eq(closes, 1);
        code(() => rl.question('x', () => {}), 'ERR_USE_AFTER_CLOSE');
    "#,
    );
}

#[test]
fn readline_cursor_helpers_are_no_ops_without_a_tty() {
    script(
        r#"
        const readline = req('readline');
        const out = sink();
        eq(readline.clearLine(out, 0), true);
        eq(readline.cursorTo(out, 0), true);
        eq(readline.moveCursor(out, 1, 1), true);
        eq(readline.clearScreenDown(out), true);
        same(out.out, []);
        let done = 0;
        readline.cursorTo(out, 0, 0, () => done++);
        await sleep(0);
        eq(done, 1);
        readline.emitKeypressEvents(new Readable({ read() {} }));
    "#,
    );
}

#[test]
fn vm_run_in_this_context_returns_the_completion_value() {
    script(
        r#"
        const vm = req('vm');
        eq(vm.runInThisContext('1 + 2'), 3);
        vm.runInThisContext('var velyrVmGlobal = 7');
        eq(globalThis.velyrVmGlobal, 7);
        const err = (() => { try { vm.runInThisContext('(', { filename: 'boom.js' }); } catch (e) { return e; } })();
        eq(err instanceof SyntaxError, true);
        code(() => vm.runInThisContext(1), 'ERR_INVALID_ARG_TYPE');
    "#,
    );
}

#[test]
fn vm_script_runs_in_this_context_more_than_once() {
    script(
        r#"
        const { Script } = req('vm');
        globalThis.velyrCounter = 0;
        const script = new Script('++velyrCounter');
        eq(script.runInThisContext(), 1);
        eq(script.runInThisContext(), 2);
        code(() => new Script(5), 'ERR_INVALID_ARG_TYPE');
    "#,
    );
}

#[test]
fn vm_contexts_are_unavailable_with_a_clear_error() {
    script(
        r#"
        const vm = req('vm');
        eq(vm.isContext({}), false);
        code(() => vm.isContext(1), 'ERR_INVALID_ARG_TYPE');
        for (const fn of [
          () => vm.createContext({}),
          () => vm.runInNewContext('1'),
          () => vm.runInContext('1', {}),
          () => new vm.Script('1').runInNewContext(),
          () => new vm.Script('1').runInContext({}),
        ]) {
          const e = code(fn, 'ERR_FEATURE_UNAVAILABLE_ON_PLATFORM');
          eq(/separate contexts/.test(e.message), true, e.message);
        }
    "#,
    );
}

#[test]
fn readline_and_vm_are_registered_builtins() {
    script(
        r#"
        const node = globalThis[Symbol.for('velyr.node')];
        for (const name of ['readline', 'readline/promises', 'vm']) eq(node.has(name), true, name);
    "#,
    );
}

#[test]
fn readline_and_vm_export_lists_match_the_modules() {
    for name in ["readline", "readline/promises", "vm"] {
        let mut listed: Vec<&str> = BUILTINS
            .iter()
            .find(|b| b.name == name)
            .unwrap()
            .exports
            .to_vec();
        listed.sort_unstable();
        script(&format!(
            "eq(Object.keys(req({name:?})).sort().join(','), {:?}, {name:?});",
            listed.join(",")
        ));
    }
}
