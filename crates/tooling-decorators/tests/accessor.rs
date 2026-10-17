use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use velyr_tooling_decorators::{Options, lower, needs_pass};

fn run(source: &str) -> String {
    let allocator = Allocator::default();
    let mut program = Parser::new(&allocator, source, SourceType::ts())
        .parse()
        .program;
    assert!(needs_pass(&program));
    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();
    let options = Options {
        accessors: true,
        decorators: false,
    };
    lower(&allocator, &mut program, scoping, options);
    Codegen::new().build(&program).code
}

#[test]
fn lowers_instance_static_private_and_computed() {
    let out =
        run("class A { accessor x = 1; static accessor y; accessor #z = 3; accessor [k()] = 4; }");
    assert!(!out.contains("accessor x"), "{out}");
    assert!(
        !out.contains("accessor #z") && !out.contains("accessor ["),
        "{out}"
    );
    for needle in [
        "#x_accessor_storage = 1",
        "get x()",
        "set x(",
        "static #y_accessor_storage",
        "get #z()",
    ] {
        assert!(out.contains(needle), "missing {needle}:\n{out}");
    }
    assert_eq!(out.matches("k()").count(), 1, "{out}");
    assert!(
        out.find("const ").unwrap() < out.find("class A").unwrap(),
        "{out}"
    );
}

#[test]
fn storage_name_avoids_existing_private_names() {
    let out = run("class A { #x_accessor_storage = 0; accessor x = 1; }");
    assert!(out.contains("#x_accessor_storage_ = 1"), "{out}");
}

#[test]
fn hoists_inside_nested_function_before_the_statement() {
    let out = run("function f(k) { const j = k; return class { accessor [j()] = 1 }; }");
    assert!(
        out.find("const j").unwrap() < out.find("j()").unwrap(),
        "{out}"
    );
    assert!(!out.contains(" accessor "), "{out}");
}

#[test]
fn needs_pass_is_false_for_plain_classes() {
    let allocator = Allocator::default();
    let program = Parser::new(&allocator, "class A { x = 1 }", SourceType::ts())
        .parse()
        .program;
    assert!(!needs_pass(&program));
}
