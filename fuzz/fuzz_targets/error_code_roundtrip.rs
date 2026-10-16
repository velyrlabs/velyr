#![no_main]

use std::str::FromStr;

use libfuzzer_sys::fuzz_target;
use velyr_runtime_errors::ErrorCode;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(code) = ErrorCode::from_str(text) {
        let reparsed = ErrorCode::from_str(code.as_str())
            .expect("as_str() output must parse back into an ErrorCode");
        assert_eq!(reparsed, code);
    }
});
