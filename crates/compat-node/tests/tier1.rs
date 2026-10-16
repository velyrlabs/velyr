//! Node Tier 1 behavior, as JS fixtures with asserts running in a real V8 isolate. Each fixture
//! registers cases with `test`/`atest` (see `fixtures/_harness.js`) and ends with `finish()`.

mod common;

macro_rules! fixture {
    ($test:ident, $file:literal) => {
        #[test]
        fn $test() {
            common::run_fixture($file, include_str!(concat!("fixtures/", $file)), false);
        }
    };
}

fixture!(path_posix_and_win32, "path.js");
fixture!(url_module, "url.js");
fixture!(events_module, "events.js");
fixture!(assert_module, "assert.js");
fixture!(util_inspect, "util_inspect.js");
fixture!(util_rest, "util_rest.js");
