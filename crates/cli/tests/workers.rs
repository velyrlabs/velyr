use std::time::Duration;

use assert_cmd::Command;

fn run(name: &str, flags: &[&str]) -> assert_cmd::assert::Assert {
    let path = format!(
        "{}/../../tests/fixtures/m5-workers/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    Command::cargo_bin("velyr")
        .unwrap()
        .env("VELYR_LEAK_CHECK", "1")
        .timeout(Duration::from_secs(30))
        .arg("run")
        .args(flags)
        .arg(&path)
        .assert()
}

#[test]
fn worker_squares_arrive_in_order() {
    run("squares.js", &[]).success().stdout("1,4,9,16,25\n");
}

#[test]
fn uncaught_error_in_worker_reaches_the_parent() {
    run("error_top.js", &[])
        .success()
        .stdout("error event: boom\n");
}

#[test]
fn error_thrown_by_a_message_handler_reaches_the_parent() {
    run("error_handler.js", &[])
        .success()
        .stdout("error event: in handler\n");
}

#[test]
fn terminate_stops_a_busy_worker() {
    run("terminate.js", &[])
        .success()
        .stdout("worker said ready\nterminated\n");
}

#[test]
fn classic_workers_are_rejected() {
    run("classic.js", &[]).success().stdout("true true\n");
}

#[test]
fn unref_lets_the_parent_exit_but_still_delivers_while_it_runs() {
    run("unref.js", &[])
        .success()
        .stdout("main done\nmessage hi\ntimer done\n");
}

#[test]
fn message_channel_within_one_isolate() {
    run("channel.js", &[])
        .success()
        .stdout("got {\"a\":1,\"list\":[1,2]} 0\n");
}

#[test]
fn transferred_port_talks_directly_to_the_worker() {
    run("transfer.js", &[])
        .success()
        .stdout("neutered: true\ndirect: hello from worker x true\n");
}

#[test]
fn broadcast_channel_reaches_across_isolates() {
    run("broadcast.js", &[])
        .success()
        .stdout("main got from worker\n");
}

#[test]
fn worker_authority_is_inherited_narrowed_and_widened_never() {
    run("permissions.js", &["--allow-env=HOME,USER"])
        .success()
        .stdout(
            "inherit HOME:granted USER:granted\n\
             narrow HOME:granted USER:denied\n\
             wider HOME:granted USER:granted\n",
        );
}

#[test]
fn worker_cannot_exceed_its_parent() {
    run("permissions.js", &["--allow-env=HOME"])
        .success()
        .stdout(
            "inherit HOME:granted USER:denied\n\
             narrow HOME:granted USER:denied\n\
             wider HOME:granted USER:denied\n",
        );
}

#[test]
fn relative_worker_url_resolves_against_the_calling_module() {
    run("nested.js", &[]).success().stdout("hello from sub\n");
}

#[test]
fn error_event_carries_the_source_location() {
    run("error_location.js", &[])
        .success()
        .stdout("boom true 1 true\n");
}

#[test]
fn full_queue_makes_post_message_fail_instead_of_growing() {
    run("queue_full.js", &[])
        .success()
        .stdout("VELYR_QUOTA_EXCEEDED 10000\n");
}
