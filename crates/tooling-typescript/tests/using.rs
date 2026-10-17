//! `using` declarations are lowered only when the engine lacks them.

use velyr_runtime_modules::{MediaType, SourceTransform};
use velyr_tooling_typescript::{Lowering, TypeScriptTransform};

const SOURCE: &str = "export function f() { using r = make(); return r.v; }\nexport async function g() { await using q = open(); }";

fn run(lowering: Lowering) -> String {
    TypeScriptTransform::new(None)
        .with_lowering(lowering)
        .transform("/m.ts", SOURCE, MediaType::TypeScript)
        .unwrap()
        .code
}

#[test]
fn lowers_using_through_the_helper() {
    let code = run(Lowering {
        explicit_resource_management: true,
    });
    assert!(
        code.contains("velyr-oxc-runtime/helpers/usingCtx"),
        "{code}"
    );
    assert!(!code.contains("using r"), "{code}");
    assert!(code.contains("_usingCtx.d()"), "{code}");
}

#[test]
fn keeps_using_when_the_engine_has_it() {
    let code = run(Lowering::default());
    assert!(code.contains("using r"), "{code}");
    assert!(!code.contains("usingCtx"), "{code}");
}
