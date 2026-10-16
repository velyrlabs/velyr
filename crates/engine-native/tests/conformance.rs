use velyr_engine_api_conformance::{Outcome, run_all};
use velyr_engine_native::NativeEngine;

#[test]
fn native_passes_its_feature_set() {
    let report = run_all(&NativeEngine);
    println!("{report}");
    assert_eq!(report.failed(), 0, "{report}");
    assert_eq!(report.passed(), 8, "{report}");
    assert!(
        matches!(report.outcome("module_import"), Some(Outcome::Pass)),
        "{report}"
    );
}
