//! `accessor x = v` becomes a private storage field plus a `get`/`set` pair.

use std::collections::HashSet;

use oxc_allocator::{Allocator, CloneIn, TakeIn, Vec as ArenaVec};
use oxc_ast::ast::{AccessorProperty, Class, ClassElement, PropertyKey, Statement};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use oxc_traverse::{Traverse, TraverseCtx};

use crate::Options;

pub(crate) struct AccessorPass<'a> {
    allocator: &'a Allocator,
    options: Options,
    key_counter: usize,
    /// Hoisted key declarations waiting for the innermost enclosing statement to close.
    pending: Vec<Statement<'a>>,
    /// Claimed declarations, keyed by the span of the statement they precede.
    ready: Vec<(Span, Vec<Statement<'a>>)>,
}

impl<'a> AccessorPass<'a> {
    pub(crate) fn new(allocator: &'a Allocator, options: Options) -> Self {
        Self {
            allocator,
            options,
            key_counter: 0,
            pending: Vec::new(),
            ready: Vec::new(),
        }
    }

    fn should_lower(&self, element: &ClassElement<'a>) -> bool {
        match element {
            ClassElement::AccessorProperty(p) => {
                !(self.options.decorators && !p.decorators.is_empty())
            }
            _ => false,
        }
    }

    fn parse(&self, text: String) -> Vec<ClassElement<'a>> {
        let text: &'a str = self.allocator.alloc_str(&text);
        let parsed = Parser::new(self.allocator, text, SourceType::mjs()).parse();
        let mut program = parsed.program;
        match program.body.pop() {
            Some(Statement::ClassDeclaration(mut class)) => class.body.body.drain(..).collect(),
            _ => unreachable!("template is always a class declaration"),
        }
    }

    fn hoist_key(&mut self, key: &mut PropertyKey<'a>) -> String {
        let name = format!("_accessor_key{}", self.key_counter);
        self.key_counter += 1;
        let text: &'a str = self.allocator.alloc_str(&format!("const {name} = 0;"));
        let mut program = Parser::new(self.allocator, text, SourceType::mjs())
            .parse()
            .program;
        let Some(Statement::VariableDeclaration(mut decl)) = program.body.pop() else {
            unreachable!("template is always a variable declaration");
        };
        let original = key.take_in(&self.allocator);
        decl.declarations[0].init = Some(original.into_expression());
        self.pending.push(Statement::VariableDeclaration(decl));
        name
    }

    fn lower_one(
        &mut self,
        mut acc: oxc_allocator::Box<'a, AccessorProperty<'a>>,
        taken: &mut HashSet<String>,
    ) -> Vec<ClassElement<'a>> {
        let mut storage = format!("{}_accessor_storage", base_name(&acc.key));
        while taken.contains(&storage) {
            storage.push('_');
        }
        taken.insert(storage.clone());

        let hoisted = acc.computed && acc.key.static_name().is_none();
        let key_text = if hoisted {
            format!("[{}]", self.hoist_key(&mut acc.key))
        } else {
            "k".to_string()
        };
        let st = if acc.r#static { "static " } else { "" };
        let mut elements = self.parse(format!(
            "class _ {{ {st}#{storage} = 0; {st}get {key_text}() {{ return this.#{storage}; }} \
             {st}set {key_text}(v) {{ this.#{storage} = v; }} }}"
        ));
        if let ClassElement::PropertyDefinition(field) = &mut elements[0] {
            field.value = acc.value.take();
        }
        if !hoisted {
            let computed = acc.computed;
            for (index, element) in elements.iter_mut().enumerate().skip(1) {
                if let ClassElement::MethodDefinition(method) = element {
                    method.key = if index == 1 {
                        acc.key.clone_in(self.allocator)
                    } else {
                        acc.key.take_in(&self.allocator)
                    };
                    method.computed = computed;
                }
            }
        }
        if let ClassElement::MethodDefinition(getter) = &mut elements[1] {
            getter.decorators =
                std::mem::replace(&mut acc.decorators, ArenaVec::new_in(&self.allocator));
        }
        elements
    }
}

pub(crate) fn base_name(key: &PropertyKey<'_>) -> String {
    let name = match key {
        PropertyKey::PrivateIdentifier(p) => Some(p.name.as_str().to_string()),
        other => other.static_name().map(|n| n.into_owned()),
    };
    match name {
        Some(n) if is_plain_identifier(&n) => n,
        _ => "computed".to_string(),
    }
}

fn is_plain_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

pub(crate) fn private_names(class: &Class<'_>) -> HashSet<String> {
    class
        .body
        .body
        .iter()
        .filter_map(|element| match element {
            ClassElement::MethodDefinition(m) => private_name(&m.key),
            ClassElement::PropertyDefinition(p) => private_name(&p.key),
            ClassElement::AccessorProperty(p) => private_name(&p.key),
            _ => None,
        })
        .collect()
}

fn private_name(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::PrivateIdentifier(p) => Some(p.name.as_str().to_string()),
        _ => None,
    }
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

impl<'a> Traverse<'a, ()> for AccessorPass<'a> {
    fn exit_class(&mut self, class: &mut Class<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
        if !class.body.body.iter().any(|e| self.should_lower(e)) {
            return;
        }
        let mut taken = private_names(class);
        let old = class.body.body.take_in(&self.allocator);
        let mut out = ArenaVec::with_capacity_in(old.len() + 2, &self.allocator);
        for element in old {
            if !self.should_lower(&element) {
                out.push(element);
                continue;
            }
            let ClassElement::AccessorProperty(acc) = element else {
                unreachable!()
            };
            out.extend(self.lower_one(acc, &mut taken));
        }
        class.body.body = out;
    }

    fn exit_statement(&mut self, stmt: &mut Statement<'a>, _ctx: &mut TraverseCtx<'a, ()>) {
        if !self.pending.is_empty() {
            self.ready
                .push((stmt.span(), std::mem::take(&mut self.pending)));
        }
    }

    fn exit_statements(
        &mut self,
        stmts: &mut ArenaVec<'a, Statement<'a>>,
        _ctx: &mut TraverseCtx<'a, ()>,
    ) {
        if self.ready.is_empty() {
            return;
        }
        let old = stmts.take_in(&self.allocator);
        for stmt in old {
            let span = stmt.span();
            let mut index = 0;
            while index < self.ready.len() {
                if contains(span, self.ready[index].0) {
                    stmts.extend(self.ready.remove(index).1);
                } else {
                    index += 1;
                }
            }
            stmts.push(stmt);
        }
    }
}
