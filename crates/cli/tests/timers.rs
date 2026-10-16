use assert_cmd::Command;

fn run(name: &str) -> assert_cmd::assert::Assert {
    let path = format!(
        "{}/../../tests/fixtures/m2-timers/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    Command::cargo_bin("velyr")
        .unwrap()
        .args(["run", &path])
        .assert()
}

#[test]
fn sync_then_microtask_then_timer() {
    run("order.js").success().stdout("0\n1\n2\n");
}

#[test]
fn cleared_timeout_never_fires_and_process_exits() {
    run("cleared.js").success().stdout("").stderr("");
}

#[test]
fn interval_cleared_on_third_run_prints_three_lines() {
    run("interval.js")
        .success()
        .stdout("tick 1\ntick 2\ntick 3\n");
}

#[test]
fn timeout_passes_extra_args_and_orders_by_delay() {
    run("args.js")
        .success()
        .stdout("number\nt0\nx y\nt1\nmacro\nmicro\n");
}

#[test]
fn non_function_callbacks_throw_type_error() {
    run("invalid.js").success().stdout("true\ntrue\ntrue\n");
}

#[test]
fn timer_ops_are_not_visible_to_user_code() {
    run("no_binding.js")
        .success()
        .stdout("undefined undefined undefined\n");
}
