use super::super::analyze;
use super::{AnnexBFunction, BlockBinding, Location, VarTarget};

/// Runs `check` on the Annex B info of the declaration of function `name` (the last symbol
/// with that name that has an entry), or `None` when no declaration qualifies.
fn try_annex_b_of(text: &str, name: &str) -> Option<AnnexBFunction> {
    analyze(text, false, true, |_, _, analysis| {
        let symbols = analysis.scoping.symbol_ids();
        let found = symbols
            .filter(|&symbol| analysis.name(symbol) == name)
            .find_map(|symbol| analysis.annex_b_function(symbol).cloned());
        Ok(found)
    })
    .ok()
    .flatten()
}

fn annex_b_of(text: &str, name: &str) -> AnnexBFunction {
    try_annex_b_of(text, name).expect("annex b entry")
}

#[test]
fn moved_block_function_gets_synthetic_block_binding() {
    let info = annex_b_of("function g() { { function f() {} } return f; }", "f");
    assert!(matches!(info.block, BlockBinding::Synthetic(_)));
    assert!(matches!(info.var, VarTarget::Symbol(_)));
}

#[test]
fn existing_var_keeps_block_symbol() {
    let info = annex_b_of("function g() { var f; { function f() {} } }", "f");
    assert!(matches!(info.block, BlockBinding::Symbol(_)));
    assert!(matches!(info.var, VarTarget::Symbol(_)));
}

#[test]
fn lexical_conflict_or_parameter_disables_annex_b() {
    assert!(try_annex_b_of("function g() { let f; { function f() {} } }", "f").is_none());
    assert!(try_annex_b_of("function g(f) { { function f() {} } }", "f").is_none());
    assert!(try_annex_b_of("function g() { { let f; { function f() {} } } }", "f").is_none());
    assert!(try_annex_b_of("'use strict'; { function f() {} }", "f").is_none());
}

#[test]
fn global_block_function_targets_global() {
    assert!(matches!(
        annex_b_of("{ function f() {} }", "f").var,
        VarTarget::Global(_)
    ));
}

#[test]
fn captured_block_function_gets_context_slot() {
    let info = annex_b_of("function g() { { function f() {} (() => f)(); } }", "f");
    assert!(matches!(
        info.block,
        BlockBinding::Synthetic(Location::Context { .. })
    ));
}

#[test]
fn globals_are_listed_in_source_order() {
    let globals = analyze(
        "{ function b() {} } { function a() {} function b() {} }",
        false,
        true,
        |_, _, analysis| {
            Ok(analysis
                .annex_b_globals()
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>())
        },
    );
    assert_eq!(globals.unwrap(), ["b", "a"]);
}

#[test]
fn catch_parameter_is_found_for_var_in_catch_block() {
    let found = analyze(
        "try {} catch (e) { { var e = 2; } }",
        false,
        true,
        |_, _, analysis| {
            let scope = analysis
                .scoping
                .scope_descendants_from_root()
                .last()
                .unwrap();
            let param = analysis.catch_parameter_for(scope, "e");
            Ok((
                param.map(|symbol| analysis.name(symbol).to_owned()),
                analysis.catch_parameter_for(scope, "x"),
            ))
        },
    );
    assert_eq!(found.unwrap(), (Some("e".to_owned()), None));
}

#[test]
fn var_outside_catch_has_no_catch_parameter() {
    let found = analyze(
        "try {} catch (e) {} { var e; }",
        false,
        true,
        |_, _, analysis| {
            let scope = analysis
                .scoping
                .scope_descendants_from_root()
                .last()
                .unwrap();
            Ok(analysis.catch_parameter_for(scope, "e"))
        },
    );
    assert!(found.unwrap().is_none());
}
