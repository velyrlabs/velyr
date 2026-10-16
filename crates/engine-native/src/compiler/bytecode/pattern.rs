//! Destructuring of binding patterns (declarations, parameters, catch) and assignment
//! patterns. Array patterns use the iterator protocol and close the iterator when the
//! destructuring completes early or throws.

use oxc_ast::ast::{
    ArrayAssignmentTarget, ArrayPattern, AssignmentTarget, AssignmentTargetMaybeDefault,
    AssignmentTargetPattern, AssignmentTargetProperty, BindingPattern, Expression,
    ObjectAssignmentTarget, ObjectPattern, PropertyKey,
};
use velyr_engine_vm::bytecode::{ConstIndex, Instruction as I, Reg, RegRange};

use super::assignment::{Place, simple_target_name};
use super::emitter::Label;
use super::function::{FunctionCompiler, binding_name};
use super::literal::static_key;
use crate::compiler::JsResult;

/// Where an assignment-pattern element stores its value.
pub enum TargetPlace<'b, 'a> {
    Simple(Place<'b, 'a>),
    Pattern(&'b AssignmentTargetPattern<'a>),
}

/// A property key that is already evaluated.
enum Key {
    Named(ConstIndex),
    Computed(Reg),
}

impl FunctionCompiler<'_> {
    /// Binds `acc` to `pattern` with InitializeBinding semantics.
    pub fn bind_pattern(&mut self, pattern: &BindingPattern) -> JsResult<()> {
        let mark = self.e.mark();
        let result = match pattern {
            BindingPattern::BindingIdentifier(id) => self.initialize_binding(id.symbol_id()),
            BindingPattern::AssignmentPattern(p) => self
                .default_value(&p.right, binding_name(&p.left))
                .and_then(|()| self.bind_pattern(&p.left)),
            BindingPattern::ObjectPattern(p) => self.object_binding(p),
            BindingPattern::ArrayPattern(p) => self.array_binding(p),
        };
        self.e.release(mark);
        result
    }

    /// Replaces an `undefined` in `acc` with the initializer.
    pub fn default_value(&mut self, initializer: &Expression, name: Option<&str>) -> JsResult<()> {
        let skip = self.e.new_label();
        self.e.jump(I::JumpIfNotUndefined, skip);
        self.expression_named(initializer, name)?;
        self.e.bind(skip);
        Ok(())
    }

    fn object_binding(&mut self, pattern: &ObjectPattern) -> JsResult<()> {
        let source = self.coercible_source();
        let keys = pattern
            .rest
            .as_ref()
            .map(|_| self.e.alloc_range(pattern.properties.len() as u32));
        for (index, property) in pattern.properties.iter().enumerate() {
            let key = self.evaluate_key(&property.key, property.computed, keys, index)?;
            self.get_key(source, &key);
            self.bind_pattern(&property.value)?;
        }
        if let Some(rest) = &pattern.rest {
            self.rest_object(source, keys);
            self.bind_pattern(&rest.argument)?;
        }
        Ok(())
    }

    fn array_binding(&mut self, pattern: &ArrayPattern) -> JsResult<()> {
        let record = self.e.alloc_range(3).first;
        self.e.emit(I::GetIterator { record });
        let start = self.e.bound_label();
        for element in &pattern.elements {
            self.step_or_undefined(record);
            if let Some(element) = element {
                self.bind_pattern(element)?;
            }
        }
        if let Some(rest) = &pattern.rest {
            self.rest_array(record);
            self.bind_pattern(&rest.argument)?;
        }
        self.close_iterator(record, start);
        Ok(())
    }

    /// Destructures the value held in `source` into an assignment pattern.
    pub fn assign_pattern(
        &mut self,
        pattern: &AssignmentTargetPattern,
        source: Reg,
    ) -> JsResult<()> {
        let mark = self.e.mark();
        self.e.emit(I::Ldar(source));
        let result = match pattern {
            AssignmentTargetPattern::ArrayAssignmentTarget(p) => self.array_assignment(p),
            AssignmentTargetPattern::ObjectAssignmentTarget(p) => self.object_assignment(p),
        };
        self.e.release(mark);
        result
    }

    fn object_assignment(&mut self, pattern: &ObjectAssignmentTarget) -> JsResult<()> {
        let source = self.coercible_source();
        let keys = pattern
            .rest
            .as_ref()
            .map(|_| self.e.alloc_range(pattern.properties.len() as u32));
        for (index, property) in pattern.properties.iter().enumerate() {
            match property {
                AssignmentTargetProperty::AssignmentTargetPropertyIdentifier(p) => {
                    let name = self.e.string(&p.binding.name);
                    if let Some(keys) = keys {
                        self.e.emit(I::LdaConstant(name));
                        self.e.emit(I::Star(Reg(keys.first.0 + index as u32)));
                    }
                    self.get_key(source, &Key::Named(name));
                    if let Some(init) = &p.init {
                        self.default_value(init, Some(&p.binding.name))?;
                    }
                    self.store_identifier(&p.binding)?;
                }
                AssignmentTargetProperty::AssignmentTargetPropertyProperty(p) => {
                    let key = self.evaluate_key(&p.name, p.computed, keys, index)?;
                    let (target, init) = split_default(&p.binding);
                    let place = self.prepare_target(target)?;
                    self.get_key(source, &key);
                    if let Some(init) = init {
                        self.default_value(init, target_name(target))?;
                    }
                    self.finish_target(&place)?;
                }
            }
        }
        if let Some(rest) = &pattern.rest {
            let place = self.prepare_target(&rest.target)?;
            self.rest_object(source, keys);
            self.finish_target(&place)?;
        }
        Ok(())
    }

    fn array_assignment(&mut self, pattern: &ArrayAssignmentTarget) -> JsResult<()> {
        let record = self.e.alloc_range(3).first;
        self.e.emit(I::GetIterator { record });
        let start = self.e.bound_label();
        for element in &pattern.elements {
            let Some(element) = element else {
                self.step_or_undefined(record);
                continue;
            };
            let (target, init) = split_default(element);
            let place = self.prepare_target(target)?;
            self.step_or_undefined(record);
            if let Some(init) = init {
                self.default_value(init, target_name(target))?;
            }
            self.finish_target(&place)?;
        }
        if let Some(rest) = &pattern.rest {
            let place = self.prepare_target(&rest.target)?;
            self.rest_array(record);
            self.finish_target(&place)?;
        }
        self.close_iterator(record, start);
        Ok(())
    }

    /// Evaluates the reference of a target before its value is produced.
    pub fn prepare_target<'b, 'a>(
        &mut self,
        target: &'b AssignmentTarget<'a>,
    ) -> JsResult<TargetPlace<'b, 'a>> {
        match target.as_simple_assignment_target() {
            Some(simple) => Ok(TargetPlace::Simple(self.prepare_place(simple, false)?)),
            None => Ok(TargetPlace::Pattern(target.to_assignment_target_pattern())),
        }
    }

    /// Stores `acc` into a prepared target.
    pub fn finish_target(&mut self, place: &TargetPlace) -> JsResult<()> {
        match place {
            TargetPlace::Simple(place) => self.store_place(place),
            TargetPlace::Pattern(pattern) => {
                let value = self.e.alloc();
                self.e.emit(I::Star(value));
                self.assign_pattern(pattern, value)
            }
        }
    }

    /// Keeps `acc` in a register after RequireObjectCoercible.
    fn coercible_source(&mut self) -> Reg {
        let source = self.e.alloc();
        self.e.emit(I::Star(source));
        self.e.emit(I::CheckObjectCoercible);
        source
    }

    /// Evaluates a pattern key; with a rest element the key is also kept in `keys[index]`.
    fn evaluate_key(
        &mut self,
        key: &PropertyKey,
        computed: bool,
        keys: Option<RegRange>,
        index: usize,
    ) -> JsResult<Key> {
        let slot = keys.map(|keys| Reg(keys.first.0 + index as u32));
        match static_key(key, computed)? {
            Some(name) => {
                let name = self.e.string(&name);
                if let Some(slot) = slot {
                    self.e.emit(I::LdaConstant(name));
                    self.e.emit(I::Star(slot));
                }
                Ok(Key::Named(name))
            }
            None => {
                let register = self.computed_key(key)?;
                if let Some(slot) = slot {
                    self.e.emit(I::Star(slot));
                    return Ok(Key::Computed(slot));
                }
                Ok(Key::Computed(register))
            }
        }
    }

    fn get_key(&mut self, source: Reg, key: &Key) {
        match *key {
            Key::Named(name) => self.e.emit(I::GetNamed {
                object: source,
                name,
                cache: 0,
            }),
            Key::Computed(register) => {
                self.e.emit(I::Ldar(register));
                self.e.emit(I::GetKeyed { object: source });
            }
        }
    }

    fn rest_object(&mut self, source: Reg, keys: Option<RegRange>) {
        self.e.emit(I::CreateObject);
        let target = self.e.alloc();
        self.e.emit(I::Star(target));
        self.e.emit(I::Ldar(source));
        let excluded = keys.unwrap_or(RegRange {
            first: target,
            count: 0,
        });
        self.e.emit(I::CopyDataProperties { target, excluded });
        self.e.emit(I::Ldar(target));
    }

    /// `acc` = next value, or `undefined` once the iterator is done.
    fn step_or_undefined(&mut self, record: Reg) {
        let exhausted = self.e.new_label();
        let end = self.e.new_label();
        self.e.emit(I::Ldar(done_flag(record)));
        self.e.jump(I::JumpIfTrue, exhausted);
        self.e
            .jump(|done| I::IteratorStep { record, done }, exhausted);
        self.e.jump(I::Jump, end);
        self.e.bind(exhausted);
        self.e.emit(I::LdaUndefined);
        self.e.bind(end);
    }

    /// `acc` = array of the remaining values.
    fn rest_array(&mut self, record: Reg) {
        self.e.emit(I::CreateArray);
        let array = self.e.alloc();
        self.e.emit(I::Star(array));
        let head = self.e.bound_label();
        let end = self.e.new_label();
        self.e.emit(I::Ldar(done_flag(record)));
        self.e.jump(I::JumpIfTrue, end);
        self.e.jump(|done| I::IteratorStep { record, done }, end);
        self.e.emit(I::ArrayPush { array });
        self.e.jump(I::Jump, head);
        self.e.bind(end);
        self.e.emit(I::Ldar(array));
    }

    /// Closes the iterator after the elements; a throw from `start` on closes it too.
    fn close_iterator(&mut self, record: Reg, start: Label) {
        let end = self.e.bound_label();
        self.e.emit(I::IteratorClose { record });
        let after = self.e.new_label();
        self.e.jump(I::Jump, after);
        let handler = self.e.bound_label();
        self.e.add_handler(start, end, handler, None);
        let exception = self.e.alloc();
        self.e.emit(I::Star(exception));
        self.e.emit(I::IteratorCloseAfterThrow { record });
        self.e.emit(I::Ldar(exception));
        self.e.emit(I::ReThrow);
        self.e.bind(after);
    }
}

fn done_flag(record: Reg) -> Reg {
    Reg(record.0 + 2)
}

fn split_default<'b, 'a>(
    element: &'b AssignmentTargetMaybeDefault<'a>,
) -> (&'b AssignmentTarget<'a>, Option<&'b Expression<'a>>) {
    match element {
        AssignmentTargetMaybeDefault::AssignmentTargetWithDefault(d) => (&d.binding, Some(&d.init)),
        target => (target.to_assignment_target(), None),
    }
}

fn target_name<'b>(target: &'b AssignmentTarget) -> Option<&'b str> {
    target
        .as_simple_assignment_target()
        .and_then(simple_target_name)
}
