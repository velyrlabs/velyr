//! `process.nextTick` ordering with the flag-gated tick hook (RT-09), through `common::script`.

mod common;

use common::script::{assert_ok, check};

#[test]
fn next_tick_runs_before_promise_reactions() {
    assert_ok(check(
        r#"const log = [];
        Promise.resolve().then(() => log.push('p'));
        process.nextTick(() => log.push('t'));
        await sleep(5);
        same(log, ['t', 'p']);"#,
    ));
}

#[test]
fn next_tick_queued_from_a_microtask_runs() {
    assert_ok(check(
        r#"const log = [];
        Promise.resolve().then(() => process.nextTick(() => log.push('t')));
        await sleep(5);
        same(log, ['t']);"#,
    ));
}

#[test]
fn nested_next_tick_drains_in_one_pass() {
    assert_ok(check(
        r#"const log = [];
        process.nextTick(() => {
          log.push('a');
          process.nextTick(() => { log.push('b'); process.nextTick(() => log.push('c')); });
        });
        Promise.resolve().then(() => log.push('p'));
        await sleep(5);
        same(log, ['a', 'b', 'c', 'p']);"#,
    ));
}

#[test]
fn next_tick_from_a_timer_callback_runs_before_the_next_timer() {
    assert_ok(check(
        r#"const log = [];
        await new Promise((resolve) => {
          setTimeout(() => { log.push('t1'); process.nextTick(() => log.push('tick')); }, 1);
          setTimeout(() => { log.push('t2'); resolve(); }, 1);
        });
        same(log, ['t1', 'tick', 't2']);"#,
    ));
}

#[test]
fn next_tick_after_a_drained_queue_runs_again() {
    assert_ok(check(
        r#"const log = [];
        process.nextTick(() => log.push(1));
        await sleep(5);
        process.nextTick(() => log.push(2));
        await sleep(5);
        same(log, [1, 2]);"#,
    ));
}
