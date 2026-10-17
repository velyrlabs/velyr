//! Source pass that lowers `accessor` fields and TC39 2023-11 decorators to plain classes.
//!
//! V8 152 cannot parse these constructs and oxc 0.154 does not lower them (ADR-012, D10).
//! Decorators lower to the Babel 2023-11 shape: the output calls [`HELPER_BINDING`] and does
//! not import it; the caller adds the import when [`uses_helper`] is true.
//! Exit plan: when oxc ships the transform, bump oxc and delete this crate.

mod accessor;
mod decorators;
mod template;

use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_ast::ast::{AccessorProperty, Decorator, IdentifierReference};
use oxc_ast_visit::Visit;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_traverse::traverse_mut;

/// Local binding the lowered code calls; the caller imports it (default export of the
/// `applyDecs2311` helper of `@oxc-project/runtime`).
pub const HELPER_BINDING: &str = "_applyDecs2311";
/// Helper module name inside the oxc runtime package (`<runtime>/helpers/applyDecs2311`).
pub const HELPER_NAME: &str = "applyDecs2311";

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
/// before running further scope-aware passes. `scoping` must match `program` on entry.
pub fn lower<'a>(
    allocator: &'a Allocator,
    program: &mut Program<'a>,
    scoping: Scoping,
    options: Options,
) -> Scoping {
    let scoping = if options.decorators {
        decorators::lower(allocator, program, &scoping);
        // Template nodes carry no scope ids; the accessor traversal needs them.
        SemanticBuilder::new()
            .build(program)
            .semantic
            .into_scoping()
    } else {
        scoping
    };
    if !options.accessors {
        return scoping;
    }
    let mut pass = accessor::AccessorPass::new(allocator, options);
    traverse_mut(&mut pass, allocator, program, scoping, ())
}

/// True when the lowered program references [`HELPER_BINDING`], so the caller must import it.
pub fn uses_helper(program: &Program<'_>) -> bool {
    let mut finder = HelperFinder(false);
    finder.visit_program(program);
    finder.0
}

struct HelperFinder(bool);

impl<'a> Visit<'a> for HelperFinder {
    fn visit_identifier_reference(&mut self, ident: &IdentifierReference<'a>) {
        self.0 |= ident.name.as_str() == HELPER_BINDING;
    }
}
