//! Static analysis of CommonJS source with oxc: the names `import` can see as named exports, the
//! `require` targets re-exported wholesale, and the `import()` calls that need a referrer.

use std::collections::{BTreeSet, HashMap};

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, AssignmentExpression, CallExpression, Expression, ImportExpression,
    ObjectPropertyKind, VariableDeclarator,
};
use oxc_ast_visit::Visit;
use oxc_parser::Parser;
use oxc_span::SourceType;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Analysis {
    pub exports: BTreeSet<String>,
    /// `module.exports = require("x")`, `module.exports = { ...require("x") }`,
    /// `Object.keys(x).forEach(k => exports[k] = x[k])` and `__exportStar(require("x"), exports)` targets.
    pub reexports: Vec<String>,
    /// Byte offsets of every `import(` expression.
    pub dynamic_imports: Vec<usize>,
}

/// `None` when the source does not parse as a script.
pub fn analyze(source: &str) -> Option<Analysis> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::cjs()).parse();
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let mut collector = Collector::default();
    collector.visit_program(&parsed.program);
    Some(collector.found)
}

/// Whether `source` uses `import`/`export` statements or `import.meta` (Node's syntax detection).
pub fn has_module_syntax(source: &str) -> bool {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::unambiguous()).parse();
    parsed.program.source_type.is_module()
}

/// Replaces each `import(` by `__velyrImport(` so dynamic imports resolve against the CommonJS file.
pub fn rewrite_dynamic_imports(source: &str, offsets: &[usize]) -> String {
    const KEYWORD: &str = "import";
    let mut out = String::with_capacity(source.len() + offsets.len() * 8);
    let mut last = 0;
    for &start in offsets {
        out.push_str(&source[last..start]);
        out.push_str("__velyrImport");
        last = start + KEYWORD.len();
    }
    out.push_str(&source[last..]);
    out
}

fn is_ident(expr: &Expression<'_>, name: &str) -> bool {
    matches!(expr, Expression::Identifier(id) if id.name == name)
}

/// `exports` or `module.exports`.
fn is_exports_object(expr: &Expression<'_>) -> bool {
    if is_ident(expr, "exports") {
        return true;
    }
    expr.as_member_expression().is_some_and(|member| {
        is_ident(member.object(), "module")
            && member
                .static_property_name()
                .is_some_and(|n| n == "exports")
    })
}

fn require_target(expr: &Expression<'_>) -> Option<String> {
    let Expression::CallExpression(call) = expr else {
        return None;
    };
    let [Argument::StringLiteral(target)] = call.arguments.as_slice() else {
        return None;
    };
    is_ident(&call.callee, "require").then(|| target.value.to_string())
}

/// Whether a `forEach` callback assigns `exports[k] = ...` with `k` its first parameter.
fn assigns_exports_by_key(callback: &Expression<'_>) -> bool {
    match callback {
        Expression::FunctionExpression(f) => f
            .body
            .as_ref()
            .is_some_and(|body| find_key_assignment(&f.params, |v| v.visit_function_body(body))),
        Expression::ArrowFunctionExpression(f) => {
            find_key_assignment(&f.params, |v| v.visit_arrow_function_body(&f.body))
        }
        _ => false,
    }
}

fn find_key_assignment<'p>(
    params: &'p oxc_ast::ast::FormalParameters<'_>,
    walk: impl FnOnce(&mut KeyAssignment<'p>),
) -> bool {
    let Some(key) = params
        .items
        .first()
        .and_then(|p| p.pattern.get_identifier_name())
    else {
        return false;
    };
    let mut finder = KeyAssignment {
        key: key.as_str(),
        found: false,
    };
    walk(&mut finder);
    finder.found
}

struct KeyAssignment<'k> {
    key: &'k str,
    found: bool,
}

impl<'a> Visit<'a> for KeyAssignment<'_> {
    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if let Some(member) = it.left.as_member_expression()
            && is_exports_object(member.object())
            && let oxc_ast::ast::MemberExpression::ComputedMemberExpression(computed) = member
            && is_ident(&computed.expression, self.key)
        {
            self.found = true;
        }
        oxc_ast_visit::walk::walk_assignment_expression(self, it);
    }
}

#[derive(Default)]
struct Collector {
    found: Analysis,
    /// Variables initialized with `require("literal")`, for the `Object.keys(x).forEach` reexport.
    required: HashMap<String, String>,
}

impl Collector {
    fn module_exports_value(&mut self, value: &Expression<'_>) {
        if let Some(target) = require_target(value) {
            self.found.reexports.push(target);
        }
        let Expression::ObjectExpression(object) = value else {
            return;
        };
        for property in &object.properties {
            match property {
                ObjectPropertyKind::ObjectProperty(property) if !property.computed => {
                    if let Some(name) = property.key.static_name() {
                        self.found.exports.insert(name.into_owned());
                    }
                }
                ObjectPropertyKind::SpreadProperty(spread) => {
                    if let Some(target) = require_target(&spread.argument) {
                        self.found.reexports.push(target);
                    }
                }
                ObjectPropertyKind::ObjectProperty(_) => {}
            }
        }
    }
}

impl Collector {
    /// `Object.keys(x).forEach(function (k) { exports[k] = x[k] })` with `x = require("t")`.
    fn keys_foreach_reexport(&mut self, it: &CallExpression<'_>) {
        let Some(foreach) = it.callee.as_member_expression() else {
            return;
        };
        if foreach.static_property_name() != Some("forEach") {
            return;
        }
        let Expression::CallExpression(keys) = foreach.object() else {
            return;
        };
        let (Some(callee), [Argument::Identifier(var)]) = (
            keys.callee.as_member_expression(),
            keys.arguments.as_slice(),
        ) else {
            return;
        };
        if is_ident(callee.object(), "Object")
            && callee.static_property_name() == Some("keys")
            && let Some(target) = self.required.get(var.name.as_str())
            && let [callback] = it.arguments.as_slice()
            && callback.as_expression().is_some_and(assigns_exports_by_key)
        {
            self.found.reexports.push(target.clone());
        }
    }

    /// `__exportStar(require("t"), exports)`, also as `tslib.__exportStar(...)`.
    fn export_star_reexport(&mut self, it: &CallExpression<'_>) {
        let helper = match &it.callee {
            Expression::Identifier(id) => Some(id.name.as_str()),
            callee => callee
                .as_member_expression()
                .and_then(|m| m.static_property_name()),
        };
        if helper == Some("__exportStar")
            && let [first, second] = it.arguments.as_slice()
            && second.as_expression().is_some_and(is_exports_object)
            && let Some(target) = first.as_expression().and_then(require_target)
        {
            self.found.reexports.push(target);
        }
    }
}

impl<'a> Visit<'a> for Collector {
    fn visit_variable_declarator(&mut self, it: &VariableDeclarator<'a>) {
        if let Some(name) = it.id.get_identifier_name()
            && let Some(target) = it.init.as_ref().and_then(require_target)
        {
            self.required.insert(name.to_string(), target);
        }
        oxc_ast_visit::walk::walk_variable_declarator(self, it);
    }

    fn visit_assignment_expression(&mut self, it: &AssignmentExpression<'a>) {
        if it.operator.is_assign()
            && let Some(member) = it.left.as_member_expression()
            && let Some(name) = member.static_property_name()
        {
            if is_exports_object(member.object()) {
                self.found.exports.insert(name.to_string());
            } else if name == "exports" && is_ident(member.object(), "module") {
                self.module_exports_value(&it.right);
            }
        }
        oxc_ast_visit::walk::walk_assignment_expression(self, it);
    }

    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        self.keys_foreach_reexport(it);
        self.export_star_reexport(it);
        // Object.defineProperty(exports, "name", ...)
        if let Some(callee) = it.callee.as_member_expression()
            && is_ident(callee.object(), "Object")
            && callee
                .static_property_name()
                .is_some_and(|n| n == "defineProperty")
            && let [
                Argument::Identifier(_) | Argument::StaticMemberExpression(_),
                Argument::StringLiteral(name),
                ..,
            ] = it.arguments.as_slice()
            && let Some(target) = it.arguments[0].as_expression()
            && is_exports_object(target)
        {
            self.found.exports.insert(name.value.to_string());
        }
        oxc_ast_visit::walk::walk_call_expression(self, it);
    }

    fn visit_import_expression(&mut self, it: &ImportExpression<'a>) {
        self.found.dynamic_imports.push(it.span.start as usize);
        oxc_ast_visit::walk::walk_import_expression(self, it);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<String> {
        analyze(source).unwrap().exports.into_iter().collect()
    }

    #[test]
    fn detects_the_four_export_patterns() {
        assert_eq!(
            names(
                "exports.a = 1; module.exports.b = 2; exports['c'] = 3; Object.defineProperty(module.exports, 'd', {});"
            ),
            ["a", "b", "c", "d"]
        );
        assert_eq!(
            names("module.exports = { a, b: 1, 'c': 2, ...rest, [k]: 3, m() {} };"),
            ["a", "b", "c", "m"]
        );
    }

    #[test]
    fn follows_reexports_and_ignores_lookalikes() {
        let found =
            analyze("module.exports = require('./x'); module.exports = { ...require('./y') };")
                .unwrap();
        assert_eq!(found.reexports, ["./x", "./y"]);
        assert!(
            names("other.exports.a = 1; exports.b += 1; const f = () => { exports.c = 1 };")
                == ["c"]
        );
    }

    #[test]
    fn follows_keys_foreach_and_export_star_reexports() {
        let found = analyze(
            "var a = require('./a'); Object.keys(a).forEach(function (k) { if (k !== 'default') exports[k] = a[k]; });\n\
             const b = require('./b'); Object.keys(b).forEach(k => { exports[k] = b[k]; });\n\
             __exportStar(require('./c'), exports); tslib.__exportStar(require('./d'), module.exports);",
        )
        .unwrap();
        assert_eq!(found.reexports, ["./a", "./b", "./c", "./d"]);
        let none = analyze(
            "var a = require('./a'); Object.keys(a).forEach(function (k) { log(k); });\n\
             Object.keys(unknown).forEach(k => { exports[k] = 1; });",
        )
        .unwrap();
        assert!(none.reexports.is_empty());
    }

    #[test]
    fn syntax_error_has_no_analysis() {
        assert!(analyze("exports.a = ;").is_none());
    }

    #[test]
    fn dynamic_imports_are_rewritten_and_module_syntax_is_sniffed() {
        let source = "const m = import('./a.mjs'); f(import(x));";
        let found = analyze(source).unwrap();
        assert_eq!(
            rewrite_dynamic_imports(source, &found.dynamic_imports),
            "const m = __velyrImport('./a.mjs'); f(__velyrImport(x));"
        );
        assert!(has_module_syntax("export const a = 1;"));
        assert!(!has_module_syntax("exports.a = 1; import('x');"));
    }
}
