//! Legacy static properties of `RegExp` (`$1`..`$9`, `input`, `lastMatch`...), after
//! proposal-regexp-legacy-features.

use std::ops::Range;
use std::rc::Rc;

use super::super::{Builtin, Interpreter, Intrinsic, JsResult, arg};
use regress::Match;

use crate::{JsStr, ObjectRef, Value};

/// The captures of the last successful legacy match.
pub(crate) struct LastMatch {
    units: Rc<[u16]>,
    range: Range<usize>,
    captures: Vec<Option<Range<usize>>>,
}

impl LastMatch {
    pub(super) fn new(
        units: Rc<[u16]>,
        range: Range<usize>,
        captures: Vec<Option<Range<usize>>>,
    ) -> Self {
        Self {
            units,
            range,
            captures,
        }
    }

    fn slice(&self, range: Range<usize>) -> Value {
        Value::String(JsStr::from_utf16(self.units[range].to_vec()))
    }
}

/// What the getters other than `input` see.
#[derive(Default)]
pub(crate) enum Others {
    /// Nothing matched yet: every getter answers the empty string.
    #[default]
    Empty,
    Matched(LastMatch),
    /// A non-legacy regexp ran last: the getters throw.
    Invalid,
}

#[derive(Default)]
pub(crate) struct LegacyStatics {
    /// `None` once invalidated.
    input: Option<JsStr>,
    others: Others,
}

impl LegacyStatics {
    pub(super) fn update(&mut self, input: JsStr, last: LastMatch) {
        self.input = Some(input);
        self.others = Others::Matched(last);
    }

    pub(super) fn invalidate(&mut self) {
        self.input = None;
        self.others = Others::Invalid;
    }
}

/// Update or invalidate the statics after a successful match.
pub(super) fn record(
    statics: &mut LegacyStatics,
    found: &Match,
    text: &JsStr,
    units: &Rc<[u16]>,
    legacy: bool,
) {
    if !legacy {
        statics.invalidate();
        return;
    }
    let captures = (1..=found.captures.len()).map(|i| found.group(i)).collect();
    statics.update(
        text.clone(),
        LastMatch::new(units.clone(), found.range(), captures),
    );
}

pub(super) fn install(vm: &mut Interpreter, constructor: ObjectRef) -> JsResult<()> {
    let getters: [(&[&str], Builtin); 5] = [
        (&["lastMatch", "$&"], |vm, this, _| {
            last(vm, this, |m| m.slice(m.range.clone()))
        }),
        (&["lastParen", "$+"], |vm, this, _| {
            last(vm, this, |m| {
                m.captures.last().map_or(Value::str(""), |c| capture(m, c))
            })
        }),
        (&["leftContext", "$`"], |vm, this, _| {
            last(vm, this, |m| m.slice(0..m.range.start))
        }),
        (&["rightContext", "$'"], |vm, this, _| {
            last(vm, this, |m| m.slice(m.range.end..m.units.len()))
        }),
        (&["input", "$_"], input_getter),
    ];
    for (names, getter) in getters {
        for name in names {
            let setter = (*name == "input" || *name == "$_").then_some(input_setter as Builtin);
            let key = vm.realm.key_str(name);
            vm.accessor(constructor, key, name, getter, setter)?;
        }
    }
    let digits: [Builtin; 9] = [
        digit::<1>, digit::<2>, digit::<3>, digit::<4>, digit::<5>, digit::<6>, digit::<7>,
        digit::<8>, digit::<9>,
    ];
    for (index, getter) in digits.into_iter().enumerate() {
        let name = format!("${}", index + 1);
        let key = vm.realm.key_str(&name);
        vm.accessor(constructor, key, &name, getter, None)?;
    }
    Ok(())
}

fn capture(last: &LastMatch, range: &Option<Range<usize>>) -> Value {
    range
        .clone()
        .map_or(Value::str(""), |range| last.slice(range))
}

fn check_receiver(vm: &mut Interpreter, this: &Value) -> JsResult<()> {
    let constructor = vm.intrinsic(Intrinsic::RegExpConstructor);
    if vm.as_object(this) == Some(constructor) {
        return Ok(());
    }
    vm.throw_type("RegExp legacy static accessor called on an incompatible receiver")
}

fn last(
    vm: &mut Interpreter,
    this: &Value,
    read: impl FnOnce(&LastMatch) -> Value,
) -> JsResult<Value> {
    check_receiver(vm, this)?;
    match &vm.state.regexps.legacy.others {
        Others::Empty => Ok(Value::str("")),
        Others::Matched(last) => Ok(read(last)),
        Others::Invalid => vm.throw_type("RegExp legacy static properties are not available"),
    }
}

fn digit<const N: usize>(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    last(vm, this, |m| {
        m.captures
            .get(N - 1)
            .map_or(Value::str(""), |c| capture(m, c))
    })
}

fn input_getter(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    check_receiver(vm, this)?;
    let statics = &vm.state.regexps.legacy;
    match (&statics.input, &statics.others) {
        (Some(input), _) => Ok(Value::String(input.clone())),
        (None, Others::Empty) => Ok(Value::str("")),
        (None, _) => vm.throw_type("RegExp legacy static properties are not available"),
    }
}

fn input_setter(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    check_receiver(vm, this)?;
    let text = vm.coerce_string(&arg(args, 0))?;
    vm.state.regexps.legacy.input = Some(text);
    Ok(Value::Undefined)
}
