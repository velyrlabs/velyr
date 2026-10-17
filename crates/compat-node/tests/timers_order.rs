//! Ordering between due timers, `nextTick` and promises; RT-10's batch dispatch must keep it.

mod common;

use common::script::{assert_ok, check};

#[test]
fn next_tick_and_promise_run_between_two_due_timers() {
    assert_ok(check(
        r#"const log = [];
        await new Promise((resolve) => {
          setTimeout(() => {
            log.push('a');
            process.nextTick(() => log.push('tick'));
            Promise.resolve().then(() => log.push('promise'));
          }, 1);
          setTimeout(() => { log.push('b'); resolve(); }, 1);
        });
        same(log, ['a', 'tick', 'promise', 'b']);"#,
    ));
}
