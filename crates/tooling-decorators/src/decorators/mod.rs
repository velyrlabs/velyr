//! TC39 2023-11 decorators, lowered to the Babel `applyDecs2311` call shape.
//!
//! Each decorated class gets a leading static block that calls [`crate::HELPER_BINDING`] and
//! stores the returned initializers in temporaries declared (`let`) right before the enclosing
//! statement. Class decorators make the class binding a temporary (`_A`): every reference to
//! the class is renamed to it and `export class A` becomes `export { _A as A }`.

mod class;
mod element;
mod values;

use std::collections::{HashMap, HashSet};

use oxc_allocator::Box as ArenaBox;
use oxc_allocator::{Allocator, TakeIn, Vec as ArenaVec};
use oxc_ast::ast::{
    ArrowFunctionBody, ArrowFunctionExpression, BindingIdentifier, BindingPattern, Class,
    ClassType, Declaration, ExportDefaultDeclarationKind, Expression, IdentifierReference,
    ObjectProperty, Program, Statement, VariableDeclarator,
};
use oxc_ast_visit::{Visit, VisitMut, walk_mut};
use oxc_semantic::{ReferenceId, Scoping, SymbolId};

use crate::template::Template;
use class::{ClassResult, needs_rewrite};

pub(crate) fn lower<'a>(allocator: &'a Allocator, program: &mut Program<'a>, scoping: &Scoping) {
    let mut names = NameCollector(HashSet::from([crate::HELPER_BINDING.to_string()]));
    names.visit_program(program);
    let mut pass = DecoratorPass {
        allocator,
        scoping,
        names: names.0,
        temps: Vec::new(),
        renames: HashMap::new(),
        class_result: None,
        preset_binding: None,
        default_name: false,
    };
    pass.visit_program(program);
    if !pass.renames.is_empty() {
        Renamer {
            allocator,
            renames: pass.renames,
        }
        .visit_program(program);
    }
}

pub(crate) struct DecoratorPass<'a, 's> {
    allocator: &'a Allocator,
    scoping: &'s Scoping,
    names: HashSet<String>,
    /// Temporaries waiting for a `let` before the innermost enclosing statement.
    temps: Vec<String>,
    renames: HashMap<ReferenceId, String>,
    /// Pending work for the enclosing statement or expression of the class just rewritten.
    class_result: Option<ClassResult<'a>>,
    /// Binding chosen by a variable declarator for the class expression it initializes.
    preset_binding: Option<String>,
    /// The next class is an anonymous `export default` class that got a temporary name.
    default_name: bool,
}

impl<'a> DecoratorPass<'a, '_> {
    /// Fresh JavaScript identifier `_<base>`, declared before the enclosing statement.
    fn temp(&mut self, base: &str) -> String {
        let name = self.unique(&format!("_{base}"));
        self.temps.push(name.clone());
        name
    }

    fn unique(&mut self, candidate: &str) -> String {
        let mut name = candidate.to_string();
        let mut counter = 2;
        while self.names.contains(&name) {
            name = format!("{candidate}{counter}");
            counter += 1;
        }
        self.names.insert(name.clone());
        name
    }

    fn rename_symbol(&mut self, symbol: SymbolId, to: &str) {
        for reference in self.scoping.get_resolved_reference_ids(symbol) {
            self.renames.insert(*reference, to.to_string());
        }
    }

    fn declare_temps(&mut self, mark: usize, out: &mut ArenaVec<'a, Statement<'a>>) {
        let temps = self.temps.split_off(mark);
        if !temps.is_empty() {
            let text = format!("let {};", temps.join(", "));
            out.extend(Template::new(self.allocator).statements(&text));
        }
    }

    /// Emits a class with class decorators: optional pre-evaluation, the class (or its
    /// identity wrapper) and, for `export [default] class`, `export { _A as A }`.
    fn finish_statement(&mut self, stmt: Statement<'a>, out: &mut ArenaVec<'a, Statement<'a>>) {
        let Some(result) = self.class_result.take() else {
            out.push(stmt);
            return;
        };
        let (class, exported) = match stmt {
            Statement::ClassDeclaration(class) => (class, None),
            Statement::ExportDeclaration(mut export) => {
                match export.declaration.take_in(&self.allocator) {
                    Declaration::ClassDeclaration(class) => {
                        let name = class.id.as_ref().map_or("default", |id| id.name.as_str());
                        let name = name.to_string();
                        (class, Some(name))
                    }
                    _ => unreachable!("only classes produce a class result"),
                }
            }
            Statement::ExportDefaultDeclaration(mut export) => {
                match export.declaration.take_in(&self.allocator) {
                    ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                        (class, Some("default".into()))
                    }
                    _ => unreachable!("only classes produce a class result"),
                }
            }
            other => unreachable!("class result outside a class statement: {other:?}"),
        };
        let binding = result.binding.clone();
        let mut template = Template::new(self.allocator);
        let mut text = String::new();
        if result.wrapper.is_some() {
            let wrapped = self.wrap_class(class, result);
            text.push_str(&format!("{};", template.slot(wrapped)));
        } else {
            if let Some(pre) = result.pre {
                text.push_str(&format!("{};", template.slot(pre)));
            }
            out.extend(template.statements(&text));
            text.clear();
            out.push(Statement::ClassDeclaration(class));
        }
        if let Some(exported) = exported {
            text.push_str(&format!("export {{ {binding} as {exported} }};"));
        }
        out.extend(template.statements(&text));
    }

    /// The class as an expression evaluating its pre-evaluation first; with a wrapper,
    /// `new class extends identity { static [class A {}]; ...statics } ()`.
    fn wrap_class(
        &mut self,
        mut class: ArenaBox<'a, Class<'a>>,
        result: ClassResult<'a>,
    ) -> Expression<'a> {
        class.r#type = ClassType::ClassExpression;
        let mut template = Template::new(self.allocator);
        let class = template.slot(Expression::ClassExpression(class));
        let pre = result
            .pre
            .map(|p| format!("{}, ", template.slot(p)))
            .unwrap_or_default();
        let Some(wrapper) = result.wrapper else {
            return template.expression(&format!("({pre}{class});"));
        };
        let members: String = wrapper
            .members
            .into_iter()
            .map(|m| template.element(m))
            .collect();
        let tail: String = wrapper
            .tail
            .iter()
            .map(|call| format!(", {call}"))
            .collect();
        let text = format!(
            "({pre}new class extends (function (v) {{ return v; }}) {{ static [{class}]; {members} \
             constructor() {{ super({}){tail}; }} }}());",
            result.binding
        );
        template.expression(&text)
    }

    /// `export default @dec class {}` needs a name to become a plain declaration.
    fn name_default_class(&mut self, stmt: &mut Statement<'a>) {
        if let Statement::ExportDefaultDeclaration(export) = stmt
            && let ExportDefaultDeclarationKind::ClassDeclaration(class) = &mut export.declaration
            && class.id.is_none()
            && !class.decorators.is_empty()
        {
            let name = self.unique("_default");
            class.id = Some(self.binding_identifier(&name));
            self.default_name = true;
        }
    }

    fn binding_identifier(&self, name: &str) -> BindingIdentifier<'a> {
        let text = format!("(class {name} {{}});");
        match Template::new(self.allocator).expression(&text) {
            Expression::ParenthesizedExpression(mut paren) => {
                match paren.expression.take_in(&self.allocator) {
                    Expression::ClassExpression(mut class) => class.id.take().expect("named"),
                    _ => unreachable!("template is a class expression"),
                }
            }
            _ => unreachable!("template is parenthesized"),
        }
    }
}

impl<'a> VisitMut<'a> for DecoratorPass<'a, '_> {
    fn visit_statements(&mut self, stmts: &mut ArenaVec<'a, Statement<'a>>) {
        let old = stmts.take_in(&self.allocator);
        for mut stmt in old {
            let mark = self.temps.len();
            self.name_default_class(&mut stmt);
            self.visit_statement(&mut stmt);
            let mut replacement = ArenaVec::new_in(&self.allocator);
            self.finish_statement(stmt, &mut replacement);
            self.declare_temps(mark, stmts);
            stmts.extend(replacement);
        }
    }

    fn visit_class(&mut self, class: &mut Class<'a>) {
        let preset = self.preset_binding.take();
        let default_name = std::mem::take(&mut self.default_name);
        walk_mut::walk_class(self, class);
        if needs_rewrite(class) {
            self.class_result = self.rewrite_class(class, preset, default_name);
        }
    }

    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        walk_mut::walk_expression(self, expression);
        if !matches!(expression, Expression::ClassExpression(_)) {
            return;
        }
        let Some(result) = self.class_result.take() else {
            return;
        };
        let binding = result.binding.clone();
        let Expression::ClassExpression(class) = expression.take_in(&self.allocator) else {
            unreachable!("checked above");
        };
        let mut template = Template::new(self.allocator);
        let value = template.slot(self.wrap_class(class, result));
        *expression = template.expression(&format!("({value}, {binding});"));
    }

    fn visit_variable_declarator(&mut self, declarator: &mut VariableDeclarator<'a>) {
        if let BindingPattern::BindingIdentifier(id) = &declarator.id
            && let Some(Expression::ClassExpression(class)) = &mut declarator.init
            && class.id.is_none()
            && !class.decorators.is_empty()
        {
            let binding = self.temp(id.name.as_str());
            if let Some(symbol) = id.symbol_id.get() {
                let mut inner = InnerReferences {
                    targets: self
                        .scoping
                        .get_resolved_reference_ids(symbol)
                        .iter()
                        .copied()
                        .collect(),
                    found: Vec::new(),
                };
                inner.visit_class(class);
                for reference in inner.found {
                    self.renames.insert(reference, binding.clone());
                }
            }
            class.id = Some(self.binding_identifier(id.name.as_str()));
            self.preset_binding = Some(binding);
        }
        walk_mut::walk_variable_declarator(self, declarator);
    }

    fn visit_arrow_function_expression(&mut self, arrow: &mut ArrowFunctionExpression<'a>) {
        let mark = self.temps.len();
        walk_mut::walk_arrow_function_expression(self, arrow);
        if self.temps.len() == mark || matches!(arrow.body, ArrowFunctionBody::FunctionBody(_)) {
            return;
        }
        let temps = self.temps.split_off(mark).join(", ");
        let mut template = Template::new(self.allocator);
        let body = template.slot(arrow.body.take_in(&self.allocator).into_expression());
        let text = format!("() => {{ let {temps}; return {body}; }};");
        if let Expression::ArrowFunctionExpression(mut wrapper) = template.expression(&text) {
            arrow.body = wrapper.body.take_in(&self.allocator);
        }
    }
}

struct NameCollector(HashSet<String>);

impl<'a> Visit<'a> for NameCollector {
    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        self.0.insert(ident.name.as_str().to_string());
    }

    fn visit_binding_identifier(&mut self, ident: &BindingIdentifier<'a>) {
        self.0.insert(ident.name.as_str().to_string());
    }
}

struct InnerReferences {
    targets: HashSet<ReferenceId>,
    found: Vec<ReferenceId>,
}

impl<'a> Visit<'a> for InnerReferences {
    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        if let Some(reference) = ident.reference_id.get()
            && self.targets.contains(&reference)
        {
            self.found.push(reference);
        }
    }
}

struct Renamer<'a> {
    allocator: &'a Allocator,
    renames: HashMap<ReferenceId, String>,
}

impl<'a> VisitMut<'a> for Renamer<'a> {
    fn visit_identifier_reference(&mut self, ident: &mut IdentifierReference<'a>) {
        if let Some(name) = ident
            .reference_id
            .get()
            .and_then(|id| self.renames.get(&id))
        {
            ident.name = self.allocator.alloc_str(name).into();
        }
    }

    fn visit_object_property(&mut self, property: &mut ObjectProperty<'a>) {
        walk_mut::walk_object_property(self, property);
        if property.shorthand
            && let Expression::Identifier(value) = &property.value
            && property.key.static_name().as_deref() != Some(value.name.as_str())
        {
            property.shorthand = false;
        }
    }
}
