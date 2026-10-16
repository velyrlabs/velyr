//! The `RegExp` constructor and prototype (ECMA-262 §22.2), matching with `regress`. The
//! symbol methods behind the String methods live in `symbols`, parsing in `syntax`.

mod symbols;
mod syntax;

use std::ops::Range;
use std::rc::Rc;

use regress::Match;

use super::array::species_getter;
use super::slots::Slot;
use super::symbol::{SYMBOL_MATCH, SYMBOL_SPECIES};
use super::{ErrorKind, Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::{JsStr, ObjectKey, ObjectRef, PropertyDescriptor, Value, same_value, to_boolean};
pub(crate) use symbols::{Substitution, get_substitution};
pub(crate) use syntax::RegExpCache;
pub use syntax::validate;
use syntax::{Flags, Matcher, escape_pattern};

/// `[[OriginalSource]]`, `[[OriginalFlags]]` and `[[RegExpMatcher]]`.
pub(crate) struct RegExpData {
    source: JsStr,
    flags: JsStr,
    parsed: Flags,
    matcher: Rc<Matcher>,
}

/// Flag accessors of `RegExp.prototype`, in the order `flags` reads them.
const FLAG_NAMES: [(char, &str); 8] = [
    ('d', "hasIndices"),
    ('g', "global"),
    ('i', "ignoreCase"),
    ('m', "multiline"),
    ('s', "dotAll"),
    ('u', "unicode"),
    ('v', "unicodeSets"),
    ('y', "sticky"),
];

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::RegExpPrototype);
    let constructor = vm.constructor("RegExp", 2, regexp_constructor, proto)?;
    vm.set_intrinsic(Intrinsic::RegExpConstructor, constructor);
    vm.method(constructor, "escape", 1, regexp_escape)?;
    let species = ObjectKey::Symbol(SYMBOL_SPECIES);
    vm.accessor(
        constructor,
        species,
        "[Symbol.species]",
        species_getter,
        None,
    )?;
    vm.method(proto, "exec", 1, regexp_exec)?;
    vm.method(proto, "test", 1, regexp_test)?;
    vm.method(proto, "toString", 0, regexp_to_string)?;
    let getters: [Builtin; 8] = [
        has_indices,
        global,
        ignore_case,
        multiline,
        dot_all,
        unicode,
        unicode_sets,
        sticky,
    ];
    for ((_, name), getter) in FLAG_NAMES.into_iter().zip(getters) {
        let key = vm.realm.key_str(name);
        vm.accessor(proto, key, name, getter, None)?;
    }
    let key = vm.realm.key_str("flags");
    vm.accessor(proto, key, "flags", regexp_flags, None)?;
    let key = vm.realm.key_str("source");
    vm.accessor(proto, key, "source", regexp_source, None)?;
    symbols::install(vm, proto)
}

type Builtin = super::Builtin;

impl Interpreter {
    /// IsRegExp: `@@match` decides when present, else the `[[RegExpMatcher]]` slot.
    pub(crate) fn is_regexp(&mut self, value: &Value) -> JsResult<bool> {
        let Some(object) = self.as_object(value) else {
            return Ok(false);
        };
        let matcher = self.get_key(object, ObjectKey::Symbol(SYMBOL_MATCH))?;
        if !matches!(matcher, Value::Undefined) {
            return Ok(to_boolean(&matcher));
        }
        Ok(matches!(self.slot(object), Some(Slot::RegExp(_))))
    }

    /// RegExpCreate(P, F).
    pub(crate) fn regexp_create(&mut self, pattern: &Value, flags: &Value) -> JsResult<ObjectRef> {
        let constructor = object_value(self.intrinsic(Intrinsic::RegExpConstructor));
        let object = self.regexp_alloc(&constructor)?;
        self.regexp_initialize(object, pattern, flags)
    }

    /// RegExpAlloc: the prototype from `new_target` and a non-configurable `lastIndex`.
    fn regexp_alloc(&mut self, new_target: &Value) -> JsResult<ObjectRef> {
        let default = self.intrinsic(Intrinsic::RegExpPrototype);
        let object = self.create_from_new_target(new_target, default, None)?;
        let key = self.realm.key_str("lastIndex");
        let desc = PropertyDescriptor::data(Value::Undefined, true, false, false);
        self.define_or_throw(object, key, &desc)?;
        Ok(object)
    }

    /// RegExpInitialize: parses the pattern (through the cache) and resets `lastIndex`.
    fn regexp_initialize(
        &mut self,
        object: ObjectRef,
        pattern: &Value,
        flags: &Value,
    ) -> JsResult<ObjectRef> {
        let source = self.string_or_empty(pattern)?;
        let flag_text = self.string_or_empty(flags)?;
        let flag_units = flag_text.to_utf16();
        let Some(parsed) = Flags::parse(&flag_units) else {
            let message = format!(
                "Invalid regular expression flags '{}'",
                flag_text.to_string_lossy()
            );
            return Err(self.error(ErrorKind::Syntax, &message));
        };
        let compiled = self
            .state
            .regexps
            .get_or_compile(&source.to_utf16(), &flag_units, parsed);
        let matcher = compiled.map_err(|message| self.error(ErrorKind::Syntax, &message))?;
        let data = RegExpData {
            source,
            flags: flag_text,
            parsed,
            matcher,
        };
        self.set_slot(object, Slot::RegExp(Rc::new(data)));
        self.set_last_index(object, 0.0)?;
        Ok(object)
    }

    fn string_or_empty(&mut self, value: &Value) -> JsResult<JsStr> {
        match value {
            Value::Undefined => Ok(JsStr::from_str("")),
            value => self.coerce_string(value),
        }
    }

    pub(crate) fn set_last_index(&mut self, object: ObjectRef, index: f64) -> JsResult<()> {
        let key = self.realm.key_str("lastIndex");
        self.set_or_throw(object, key, Value::Number(index))
    }

    /// ToLength(Get(R, "lastIndex")).
    pub(crate) fn last_index(&mut self, object: ObjectRef) -> JsResult<f64> {
        let value = self.get_named(object, "lastIndex")?;
        self.coerce_length(&value)
    }

    /// RegExpExec: a callable `exec` decides, else the built-in matcher.
    pub(crate) fn regexp_exec(&mut self, object: ObjectRef, text: &JsStr) -> JsResult<Value> {
        let exec = self.get_named(object, "exec")?;
        if self.is_callable(&exec) {
            let result =
                self.call_value(&exec, &object_value(object), &[Value::String(text.clone())])?;
            if !matches!(result, Value::Object(_) | Value::Null) {
                return self.throw_type(
                    "RegExp exec method returned something other than an Object or null",
                );
            }
            return Ok(result);
        }
        let data = this_regexp(self, &object_value(object), "exec")?;
        self.builtin_exec(object, &data, text)
    }

    /// RegExpBuiltinExec.
    fn builtin_exec(
        &mut self,
        object: ObjectRef,
        data: &RegExpData,
        text: &JsStr,
    ) -> JsResult<Value> {
        let flags = data.parsed;
        let mut last_index = self.last_index(object)?;
        let tracks_index = flags.global || flags.sticky;
        if !tracks_index {
            last_index = 0.0;
        }
        let units = self.state.regexps.utf16_of(text);
        if last_index > units.len() as f64 {
            return self.match_failed(object, tracks_index);
        }
        let start = last_index as usize;
        let found = data
            .matcher
            .find(&units, start, flags.full_unicode(), &self.terminated);
        // An interrupted match reads as no match; the raised flag tells them apart.
        self.check_termination()?;
        let Some(found) = found.filter(|m| !flags.sticky || m.start() == start) else {
            return self.match_failed(object, tracks_index);
        };
        if tracks_index {
            self.set_last_index(object, found.end() as f64)?;
        }
        self.match_array(&found, text, &units, flags.has_indices)
    }

    fn match_failed(&mut self, object: ObjectRef, tracks_index: bool) -> JsResult<Value> {
        if tracks_index {
            self.set_last_index(object, 0.0)?;
        }
        Ok(Value::Null)
    }

    /// The exec result: captures, `index`, `input`, `groups` and, for `d`, `indices`.
    fn match_array(
        &mut self,
        found: &Match,
        text: &JsStr,
        units: &[u16],
        has_indices: bool,
    ) -> JsResult<Value> {
        let count = found.captures.len() + 1;
        let array = self.array_create(count as f64, None)?;
        let key = self.realm.key_str("index");
        self.create_data_or_throw(array, key, Value::Number(found.start() as f64))?;
        let key = self.realm.key_str("input");
        self.create_data_or_throw(array, key, Value::String(text.clone()))?;
        for index in 0..count {
            let value = found
                .group(index)
                .map_or(Value::Undefined, |range| substring(units, range));
            self.create_data_or_throw(array, ObjectKey::Index(index as u32), value)?;
        }
        let groups = self.groups_object(found, |_, range| Ok(substring(units, range)))?;
        let key = self.realm.key_str("groups");
        self.create_data_or_throw(array, key, groups)?;
        if has_indices {
            let indices = self.match_indices(found)?;
            let key = self.realm.key_str("indices");
            self.create_data_or_throw(array, key, indices)?;
        }
        Ok(object_value(array))
    }

    /// The null-prototype `groups` object (or `undefined` without named groups).
    fn groups_object(
        &mut self,
        found: &Match,
        mut value_of: impl FnMut(&mut Self, Range<usize>) -> JsResult<Value>,
    ) -> JsResult<Value> {
        if found.named_groups().next().is_none() {
            return Ok(Value::Undefined);
        }
        let groups = self.new_object_with(None)?;
        for (name, range) in found.named_groups() {
            let value = match range {
                Some(range) => value_of(self, range)?,
                None => Value::Undefined,
            };
            let key = self.realm.key_str(name);
            self.create_data_or_throw(groups, key, value)?;
        }
        Ok(object_value(groups))
    }

    /// MakeMatchIndicesIndexPairArray.
    fn match_indices(&mut self, found: &Match) -> JsResult<Value> {
        let count = found.captures.len() + 1;
        let array = self.array_create(count as f64, None)?;
        for index in 0..count {
            let pair = match found.group(index) {
                Some(range) => self.index_pair(range)?,
                None => Value::Undefined,
            };
            self.create_data_or_throw(array, ObjectKey::Index(index as u32), pair)?;
        }
        let groups = self.groups_object(found, |vm, range| vm.index_pair(range))?;
        let key = self.realm.key_str("groups");
        self.create_data_or_throw(array, key, groups)?;
        Ok(object_value(array))
    }

    fn index_pair(&mut self, range: Range<usize>) -> JsResult<Value> {
        let pair = [
            Value::Number(range.start as f64),
            Value::Number(range.end as f64),
        ];
        self.array_from_list(&pair)
    }
}

fn substring(units: &[u16], range: Range<usize>) -> Value {
    Value::String(JsStr::from_utf16(units[range].to_vec()))
}

/// `this` as a RegExp object with its data.
fn this_regexp(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<Rc<RegExpData>> {
    match vm.value_slot(this) {
        Some(Slot::RegExp(data)) => Ok(data),
        _ => vm.throw_type(&format!(
            "Method RegExp.prototype.{method} called on incompatible receiver"
        )),
    }
}

fn regexp_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let pattern = arg(args, 0);
    let flags = arg(args, 1);
    let pattern_is_regexp = vm.is_regexp(&pattern)?;
    let mut new_target = vm.new_target();
    if matches!(new_target, Value::Undefined) {
        new_target = object_value(vm.intrinsic(Intrinsic::RegExpConstructor));
        if pattern_is_regexp && matches!(flags, Value::Undefined) {
            let object = vm.as_object(&pattern).expect("IsRegExp values are objects");
            let constructor = vm.get_named(object, "constructor")?;
            if same_value(&new_target, &constructor) {
                return Ok(pattern);
            }
        }
    }
    let (source, flags) = match vm.value_slot(&pattern) {
        Some(Slot::RegExp(data)) => {
            let flags = match flags {
                Value::Undefined => Value::String(data.flags.clone()),
                flags => flags,
            };
            (Value::String(data.source.clone()), flags)
        }
        _ if pattern_is_regexp => {
            let object = vm.as_object(&pattern).expect("IsRegExp values are objects");
            let source = vm.get_named(object, "source")?;
            vm.keep(&source);
            let flags = match flags {
                Value::Undefined => vm.get_named(object, "flags")?,
                flags => flags,
            };
            (source, flags)
        }
        _ => (pattern, flags),
    };
    vm.keep(&source);
    vm.keep(&flags);
    let object = vm.regexp_alloc(&new_target)?;
    Ok(object_value(vm.regexp_initialize(object, &source, &flags)?))
}

fn regexp_escape(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let Value::String(text) = arg(args, 0) else {
        return vm.throw_type("RegExp.escape requires a string");
    };
    Ok(Value::String(JsStr::from_utf16(syntax::regexp_escape(
        &text.to_utf16(),
    ))))
}

fn regexp_exec(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let data = this_regexp(vm, this, "exec")?;
    let object = vm.as_object(this).expect("RegExp values are objects");
    let text = vm.coerce_string(&arg(args, 0))?;
    vm.builtin_exec(object, &data, &text)
}

fn regexp_test(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let object = vm.this_object_for(this, "RegExp.prototype.test")?;
    let text = vm.coerce_string(&arg(args, 0))?;
    let result = vm.regexp_exec(object, &text)?;
    Ok(Value::Bool(!matches!(result, Value::Null)))
}

fn regexp_to_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let object = vm.this_object_for(this, "RegExp.prototype.toString")?;
    let source = vm.get_named(object, "source")?;
    let source = vm.coerce_string(&source)?;
    let flags = vm.get_named(object, "flags")?;
    let flags = vm.coerce_string(&flags)?;
    let mut units: Vec<u16> = vec![0x2F];
    units.extend(source.to_utf16());
    units.push(0x2F);
    units.extend(flags.to_utf16());
    Ok(Value::String(JsStr::from_utf16(units)))
}

fn regexp_flags(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let object = vm.this_object_for(this, "RegExp.prototype.flags getter")?;
    let mut flags = String::new();
    for (flag, name) in FLAG_NAMES {
        if to_boolean(&vm.get_named(object, name)?) {
            flags.push(flag);
        }
    }
    Ok(Value::str(&flags))
}

fn regexp_source(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    match regexp_or_prototype(vm, this, "source")? {
        Some(data) => Ok(Value::String(JsStr::from_utf16(escape_pattern(
            &data.source.to_utf16(),
        )))),
        None => Ok(Value::str("(?:)")),
    }
}

/// The data of a RegExp `this`; `None` for `RegExp.prototype` itself (whose accessors
/// answer without throwing).
fn regexp_or_prototype(
    vm: &mut Interpreter,
    this: &Value,
    name: &str,
) -> JsResult<Option<Rc<RegExpData>>> {
    if let Some(Slot::RegExp(data)) = vm.value_slot(this) {
        return Ok(Some(data));
    }
    let proto = vm.intrinsic(Intrinsic::RegExpPrototype);
    if vm.as_object(this) == Some(proto) {
        return Ok(None);
    }
    vm.throw_type(&format!(
        "RegExp.prototype.{name} getter called on incompatible receiver"
    ))
}

fn flag(
    vm: &mut Interpreter,
    this: &Value,
    name: &str,
    read: fn(Flags) -> bool,
) -> JsResult<Value> {
    Ok(match regexp_or_prototype(vm, this, name)? {
        Some(data) => Value::Bool(read(data.parsed)),
        None => Value::Undefined,
    })
}

macro_rules! flag_getters {
    ($($getter:ident => $field:ident, $name:literal;)*) => {
        $(fn $getter(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
            flag(vm, this, $name, |flags| flags.$field)
        })*
    };
}

flag_getters! {
    has_indices => has_indices, "hasIndices";
    global => global, "global";
    ignore_case => ignore_case, "ignoreCase";
    multiline => multiline, "multiline";
    dot_all => dot_all, "dotAll";
    unicode => unicode, "unicode";
    unicode_sets => unicode_sets, "unicodeSets";
    sticky => sticky, "sticky";
}
