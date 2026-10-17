//! TC39 2023-11 decorators lowering. `tests/golden/*.babel.js` is Babel 8's own output
//! (helpers stripped) kept as the reference shape; `tests/golden/*.js` is this pass's output.
//! Regenerate the latter with `UPDATE_GOLDEN=1`.

use std::path::PathBuf;

use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use velyr_tooling_decorators::{HELPER_BINDING, Options, lower, needs_pass, uses_helper};

fn run(source: &str) -> String {
    run_as(source, SourceType::mjs())
}

/// Lowers `source` (TypeScript) and checks the output parses as `output`.
fn run_as(source: &str, output: SourceType) -> String {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let mut program = parsed.program;
    assert!(needs_pass(&program));
    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();
    let options = Options {
        accessors: true,
        decorators: true,
    };
    lower(&allocator, &mut program, scoping, options);
    assert!(uses_helper(&program));
    let code = Codegen::new().build(&program).code;
    let reparsed = Parser::new(&allocator, &code, output).parse();
    assert!(
        reparsed.diagnostics.is_empty(),
        "{:?}\n{code}",
        reparsed.diagnostics
    );
    code
}

fn assert_lowered(out: &str) {
    assert!(out.contains(&format!("{HELPER_BINDING}(")), "{out}");
    assert!(!out.contains('@'), "{out}");
    assert!(!out.contains("accessor "), "{out}");
    assert!(!out.contains("import "), "{out}");
}

fn check_golden(name: &str, out: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/golden/{name}.js"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, out).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(out, expected, "golden {name} differs");
}

#[test]
fn class_decorator() {
    let out = run("@dec class A {}\nnew A();");
    assert_lowered(&out);
    assert!(out.contains("new _A()"), "{out}");
    check_golden("class_decorator", &out);
}

#[test]
fn member_decorators() {
    let out = run(
        "class A { @dec m() {} @dec static x = 1; @dec accessor y = 2; @dec get g() { return 1; } }",
    );
    assert_lowered(&out);
    for needle in [
        "_init_x(1)",
        "_init_y(this, 2)",
        "_initProto(this)",
        "get y()",
    ] {
        assert!(out.contains(needle), "missing {needle}:\n{out}");
    }
    check_golden("members", &out);
}

#[test]
fn export_class_decorator() {
    let out = run("export @dec class B {}");
    assert_lowered(&out);
    assert!(out.contains("export { _B as B }"), "{out}");
    check_golden("export_class", &out);
}

#[test]
fn private_method_decorator() {
    let out = run("class C { @dec #p() {} }");
    assert_lowered(&out);
    assert!(out.contains("#p = _call_p"), "{out}");
    assert!(out.contains("#p in _"), "{out}");
    check_golden("private_method", &out);
}

#[test]
fn decorator_before_export_and_member_expression() {
    let out = run("@a.b.c(1) export class D { @x.y static [k()]() {} }");
    assert_lowered(&out);
    assert_eq!(out.matches("k()").count(), 1, "{out}");
    assert!(out.contains("export { _D as D }"), "{out}");
}

#[test]
fn private_static_field_getter_setter_and_heritage() {
    let out = run(
        "class E extends Base { @d static #s = 1; @d accessor #q = 1; @d set v(x) {} @d get #h() { return 1; } }",
    );
    assert_lowered(&out);
    for needle in [
        "o.#s",
        "_get_q(this)",
        "_call_h(this)",
        "extends (_Base = Base)",
    ] {
        assert!(out.contains(needle), "missing {needle}:\n{out}");
    }
}

#[test]
fn class_expression_and_arrow_body() {
    let out =
        run("const F = @dec class { static y = F; };\nconst g = () => class { @dec m() {} };");
    assert_lowered(&out);
    assert!(out.contains("class F"), "{out}");
    assert!(out.contains("y = _F"), "{out}");
    assert!(out.contains("return class"), "{out}");
}

#[test]
fn class_decorator_moves_static_fields_to_the_decorated_class() {
    let out =
        run("@dec class A { static #s = 1; static { log(this); } static read() { return A.#s; } }");
    assert_lowered(&out);
    assert!(out.contains("new class extends"), "{out}");
    assert!(out.contains("_A.#s"), "{out}");
    assert!(!out.contains("static #s"), "{out}");
}

#[test]
fn anonymous_default_export_keeps_its_name() {
    let out = run("export default @dec class {}");
    assert_lowered(&out);
    assert!(out.contains("value: \"default\""), "{out}");
    assert!(out.contains("as default }"), "{out}");
}

#[test]
fn keeps_typescript_annotations_for_the_strip_pass() {
    let out = run_as(
        "class A { @dec private x: number = 1; @dec m<T>(a: T): T { return a; } }",
        SourceType::ts(),
    );
    assert_lowered(&out);
    assert!(out.contains("private x: number"), "{out}");
}
