use velyr_engine_api_conformance::run_all;
use velyr_engine_v8::V8Engine;

#[test]
fn v8_passes_every_conformance_case() {
    let report = run_all(&V8Engine::new());
    println!("{report}");
    assert_eq!(report.failed(), 0, "{report}");
    assert_eq!(report.skipped(), 0, "{report}");
    assert_eq!(report.passed(), 12, "{report}");
}
