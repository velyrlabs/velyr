//! Builds AST from JavaScript text with placeholders for existing nodes.
//!
//! `$$N` (an identifier in expression position) stands for the N-th slot expression, a
//! `$$eN;` field for the N-th class element and a `$$sN;` statement for the N-th statement list. Spans of template nodes are template-local.

use oxc_allocator::{Allocator, TakeIn, Vec as ArenaVec};
use oxc_ast::ast::{ClassBody, ClassElement, Expression, PropertyKey, Statement};
use oxc_ast_visit::{VisitMut, walk_mut};
use oxc_parser::Parser;
use oxc_span::SourceType;

pub(crate) struct Template<'a> {
    allocator: &'a Allocator,
    slots: Vec<Option<Expression<'a>>>,
    elements: Vec<Option<ClassElement<'a>>>,
    statement_lists: Vec<Option<ArenaVec<'a, Statement<'a>>>>,
}

impl<'a> Template<'a> {
    pub(crate) fn new(allocator: &'a Allocator) -> Self {
        Self {
            allocator,
            slots: Vec::new(),
            elements: Vec::new(),
            statement_lists: Vec::new(),
        }
    }

    /// Placeholder text for an existing expression.
    pub(crate) fn slot(&mut self, expression: Expression<'a>) -> String {
        self.slots.push(Some(expression));
        format!("$${}", self.slots.len() - 1)
    }

    /// Placeholder text (a whole class element) for an existing element.
    pub(crate) fn element(&mut self, element: ClassElement<'a>) -> String {
        self.elements.push(Some(element));
        format!("$$e{};", self.elements.len() - 1)
    }

    /// Placeholder text (a statement) for existing statements, spliced in place.
    pub(crate) fn statement_list(&mut self, statements: ArenaVec<'a, Statement<'a>>) -> String {
        self.statement_lists.push(Some(statements));
        format!("$$s{};", self.statement_lists.len() - 1)
    }

    pub(crate) fn statements(&mut self, text: &str) -> ArenaVec<'a, Statement<'a>> {
        let text: &'a str = self.allocator.alloc_str(text);
        let mut program = Parser::new(self.allocator, text, SourceType::mjs())
            .parse()
            .program;
        let mut substitute = Substitute {
            allocator: self.allocator,
            slots: &mut self.slots,
            elements: &mut self.elements,
            statement_lists: &mut self.statement_lists,
        };
        substitute.visit_program(&mut program);
        program.body
    }

    /// `text` must parse as a single expression statement.
    pub(crate) fn expression(&mut self, text: &str) -> Expression<'a> {
        match self.statements(text).pop() {
            Some(Statement::ExpressionStatement(mut stmt)) => {
                stmt.expression.take_in(&self.allocator)
            }
            _ => unreachable!("template is always an expression statement: {text}"),
        }
    }

    pub(crate) fn class_elements(&mut self, body: &str) -> ArenaVec<'a, ClassElement<'a>> {
        match self.statements(&format!("class _ {{ {body} }}")).pop() {
            Some(Statement::ClassDeclaration(mut class)) => {
                class.body.body.take_in(&self.allocator)
            }
            _ => unreachable!("template is always a class declaration"),
        }
    }
}

struct Substitute<'t, 'a> {
    allocator: &'a Allocator,
    slots: &'t mut Vec<Option<Expression<'a>>>,
    elements: &'t mut Vec<Option<ClassElement<'a>>>,
    statement_lists: &'t mut Vec<Option<ArenaVec<'a, Statement<'a>>>>,
}

fn placeholder_index(name: &str, prefix: &str) -> Option<usize> {
    name.strip_prefix(prefix)?.parse().ok()
}

impl<'a> VisitMut<'a> for Substitute<'_, 'a> {
    fn visit_statements(&mut self, statements: &mut ArenaVec<'a, Statement<'a>>) {
        let old = statements.take_in(&self.allocator);
        for mut statement in old {
            let index = match &statement {
                Statement::ExpressionStatement(stmt) => match &stmt.expression {
                    Expression::Identifier(ident) => placeholder_index(ident.name.as_str(), "$$s"),
                    _ => None,
                },
                _ => None,
            };
            match index {
                Some(index) => {
                    statements.extend(self.statement_lists[index].take().expect("list used once"))
                }
                None => {
                    self.visit_statement(&mut statement);
                    statements.push(statement);
                }
            }
        }
    }

    fn visit_expression(&mut self, expression: &mut Expression<'a>) {
        if let Expression::Identifier(ident) = expression
            && let Some(index) = placeholder_index(ident.name.as_str(), "$$")
        {
            *expression = self.slots[index].take().expect("slot used once");
            return;
        }
        walk_mut::walk_expression(self, expression);
    }

    fn visit_class_body(&mut self, body: &mut ClassBody<'a>) {
        for element in body.body.iter_mut() {
            let index = match element {
                ClassElement::PropertyDefinition(field) => match &field.key {
                    PropertyKey::StaticIdentifier(key) => {
                        placeholder_index(key.name.as_str(), "$$e")
                    }
                    _ => None,
                },
                _ => None,
            };
            match index {
                Some(index) => *element = self.elements[index].take().expect("element used once"),
                None => self.visit_class_element(element),
            }
        }
    }
}

/// A double-quoted JavaScript string literal for `value`.
pub(crate) fn js_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
