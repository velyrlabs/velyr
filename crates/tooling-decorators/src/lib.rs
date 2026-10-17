//! Source pass that lowers `accessor` fields (and, later, TC39 decorators) to plain classes.
//!
//! V8 152 cannot parse these constructs and oxc 0.154 does not lower them (ADR-012, D10).
//! Exit plan: when oxc ships the transform, bump oxc and delete this crate.

mod accessor;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_ast::ast::{AccessorProperty, Decorator};
use oxc_ast_visit::Visit;
use oxc_semantic::Scoping;
use oxc_traverse::traverse_mut;

/// Which constructs `lower` rewrites.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub accessors: bool,
    pub decorators: bool,
}

/// True when the program holds an `accessor` property or any decorator.
pub fn needs_pass(program: &Program<'_>) -> bool {
    let mut finder = Finder(false);
    finder.visit_program(program);
    finder.0
}

struct Finder(bool);

impl<'a> Visit<'a> for Finder {
    fn visit_accessor_property(&mut self, _: &AccessorProperty<'a>) {
        self.0 = true;
    }

    fn visit_decorator(&mut self, _: &Decorator<'a>) {
        self.0 = true;
    }
}

/// Lowers the constructs enabled in `options`.
///
/// The returned `Scoping` is stale for rewritten classes: rebuild it with `SemanticBuilder`
/// before running further scope-aware passes.
pub fn lower<'a>(
    allocator: &'a Allocator,
    program: &mut Program<'a>,
    scoping: Scoping,
    options: Options,
) -> Scoping {
    if !options.accessors {
        return scoping;
    }
    let mut pass = accessor::AccessorPass::new(allocator, options);
    traverse_mut(&mut pass, allocator, program, scoping, ())
}
