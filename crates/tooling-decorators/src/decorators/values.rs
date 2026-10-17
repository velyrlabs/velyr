//! Decorator expressions: values, `this` receivers for member-expression decorators and
//! optional memoization into temporaries.

use oxc_allocator::{TakeIn, Vec as ArenaVec};
use oxc_ast::ast::{Decorator, Expression};

use super::class::ClassLowering;

impl<'a> ClassLowering<'_, 'a, '_> {
    /// Decorator values passed to the helper and whether it is in `[this, fn]`
    /// pair form. With `setup`, non-identifier decorators are evaluated into temporaries there.
    pub(super) fn decorator_list(
        &mut self,
        decorators: ArenaVec<'a, Decorator<'a>>,
        setup: &mut Option<Vec<String>>,
    ) -> (Vec<String>, bool) {
        let mut pairs = Vec::with_capacity(decorators.len());
        for decorator in decorators {
            pairs.push(self.decorator(decorator.expression, setup));
        }
        let has_this = pairs.iter().any(|(this, _)| this.is_some());
        let parts: Vec<String> = if has_this {
            pairs
                .into_iter()
                .flat_map(|(this, f)| [this.unwrap_or_else(|| "void 0".into()), f])
                .collect()
        } else {
            pairs.into_iter().map(|(_, f)| f).collect()
        };
        (parts, has_this)
    }

    fn decorator(
        &mut self,
        mut expression: Expression<'a>,
        setup: &mut Option<Vec<String>>,
    ) -> (Option<String>, String) {
        if let Some(member) = expression.as_member_expression_mut() {
            let object_temp = self.pass.temp("obj");
            let object = member.object_mut().take_in(&self.pass.allocator);
            *member.object_mut() = self.template.expression(&format!("{object_temp};"));
            let object = self.template.slot(object);
            let function = self.template.slot(expression);
            return match setup {
                Some(setup) => {
                    let dec = self.pass.temp("dec");
                    setup.push(format!("{object_temp} = {object}"));
                    setup.push(format!("{dec} = {function}"));
                    (Some(object_temp), dec)
                }
                None => (Some(format!("{object_temp} = {object}")), function),
            };
        }
        let is_identifier = matches!(expression, Expression::Identifier(_));
        let value = self.template.slot(expression);
        match setup {
            Some(setup) if !is_identifier => {
                let dec = self.pass.temp("dec");
                setup.push(format!("{dec} = {value}"));
                (None, dec)
            }
            _ => (None, value),
        }
    }
}
