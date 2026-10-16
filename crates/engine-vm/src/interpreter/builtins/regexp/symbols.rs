//! `RegExp.prototype[@@match/@@matchAll/@@replace/@@search/@@split]` (ECMA-262 §22.2.6),
//! %RegExpStringIteratorPrototype% and GetSubstitution (shared with `String.prototype`).

use std::cell::RefCell;
use std::rc::Rc;

use super::super::slots::{RegExpIteratorState, Slot};
use super::super::string::{code_point_size, string_value};
use super::super::symbol::{
    SYMBOL_MATCH, SYMBOL_MATCH_ALL, SYMBOL_REPLACE, SYMBOL_SEARCH, SYMBOL_SPLIT,
};
use super::super::{Interpreter, Intrinsic, JsResult, arg, object_value};
use crate::interpreter::operators::to_uint32;
use crate::{JsStr, ObjectKey, ObjectRef, Value, same_value};

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    vm.symbol_method(proto, SYMBOL_MATCH, 1, regexp_match)?;
    vm.symbol_method(proto, SYMBOL_MATCH_ALL, 1, regexp_match_all)?;
    vm.symbol_method(proto, SYMBOL_REPLACE, 2, regexp_replace)?;
    vm.symbol_method(proto, SYMBOL_SEARCH, 1, regexp_search)?;
    vm.symbol_method(proto, SYMBOL_SPLIT, 2, regexp_split)?;
    let iterator = vm.intrinsic(Intrinsic::RegExpStringIteratorPrototype);
    vm.method(iterator, "next", 0, regexp_string_iterator_next)?;
    vm.define_to_string_tag(iterator, "RegExp String Iterator")
}

/// `this` and ToString(argument) for a symbol method.
fn receiver_and_text(
    vm: &mut Interpreter,
    this: &Value,
    args: &[Value],
    method: &str,
) -> JsResult<(ObjectRef, JsStr)> {
    let object = vm.this_object_for(this, &format!("RegExp.prototype[{method}]"))?;
    let text = vm.coerce_string(&arg(args, 0))?;
    Ok((object, text))
}

fn flags_of(vm: &mut Interpreter, object: ObjectRef) -> JsResult<Vec<u16>> {
    let flags = vm.get_named(object, "flags")?;
    Ok(vm.coerce_string(&flags)?.to_utf16())
}

fn has_flag(flags: &[u16], flag: u8) -> bool {
    flags.contains(&u16::from(flag))
}

fn is_full_unicode(flags: &[u16]) -> bool {
    has_flag(flags, b'u') || has_flag(flags, b'v')
}

/// AdvanceStringIndex.
fn advance(units: &[u16], index: f64, unicode: bool) -> f64 {
    if !unicode || index + 1.0 >= units.len() as f64 {
        return index + 1.0;
    }
    index + code_point_size(units, index as usize) as f64
}

/// ToString(Get(result, "0")).
fn matched_text(vm: &mut Interpreter, result: &Value) -> JsResult<JsStr> {
    let object = vm
        .as_object(result)
        .expect("RegExpExec results are objects");
    let matched = vm.get_key(object, ObjectKey::Index(0))?;
    vm.coerce_string(&matched)
}

/// Moves `lastIndex` past an empty match so global iteration makes progress.
fn step_over_empty(
    vm: &mut Interpreter,
    rx: ObjectRef,
    matched: &JsStr,
    units: &[u16],
    unicode: bool,
) -> JsResult<()> {
    if !matched.is_empty() {
        return Ok(());
    }
    let index = vm.last_index(rx)?;
    vm.set_last_index(rx, advance(units, index, unicode))
}

fn regexp_match(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (rx, text) = receiver_and_text(vm, this, args, "Symbol.match")?;
    let flags = flags_of(vm, rx)?;
    if !has_flag(&flags, b'g') {
        return vm.regexp_exec(rx, &text);
    }
    let unicode = is_full_unicode(&flags);
    vm.set_last_index(rx, 0.0)?;
    let units = text.to_utf16();
    let mut matches = Vec::new();
    loop {
        let result = vm.regexp_exec(rx, &text)?;
        if matches!(result, Value::Null) {
            return match matches.is_empty() {
                true => Ok(Value::Null),
                false => vm.array_from_list(&matches),
            };
        }
        let matched = matched_text(vm, &result)?;
        matches.push(Value::String(matched.clone()));
        step_over_empty(vm, rx, &matched, &units, unicode)?;
    }
}

fn regexp_match_all(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (rx, text) = receiver_and_text(vm, this, args, "Symbol.matchAll")?;
    let default = object_value(vm.intrinsic(Intrinsic::RegExpConstructor));
    let constructor = vm.species_constructor(rx, default)?;
    let flags = flags_of(vm, rx)?;
    let flag_value = string_value(flags.clone());
    let matcher = vm.construct_object(&constructor, &[object_value(rx), flag_value])?;
    let last_index = vm.last_index(rx)?;
    vm.set_last_index(matcher, last_index)?;
    let state = RegExpIteratorState {
        matcher: object_value(matcher),
        text,
        global: has_flag(&flags, b'g'),
        unicode: is_full_unicode(&flags),
        done: false,
    };
    let proto = vm.intrinsic(Intrinsic::RegExpStringIteratorPrototype);
    let slot = Slot::RegExpStringIterator(Rc::new(RefCell::new(state)));
    Ok(object_value(vm.new_slotted(proto, slot)?))
}

fn regexp_string_iterator_next(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let Some(Slot::RegExpStringIterator(state)) = vm.value_slot(this) else {
        return vm.throw_type("%RegExpStringIterator%.next called on incompatible receiver");
    };
    if state.borrow().done {
        return vm.iter_result(Value::Undefined, true);
    }
    let (matcher, text, global, unicode) = {
        let state = state.borrow();
        (
            state.matcher.clone(),
            state.text.clone(),
            state.global,
            state.unicode,
        )
    };
    let rx = vm.as_object(&matcher).expect("the matcher is an object");
    let result = vm.regexp_exec(rx, &text)?;
    if matches!(result, Value::Null) {
        state.borrow_mut().done = true;
        return vm.iter_result(Value::Undefined, true);
    }
    vm.keep(&result);
    if global {
        let matched = matched_text(vm, &result)?;
        step_over_empty(vm, rx, &matched, &text.to_utf16(), unicode)?;
    } else {
        state.borrow_mut().done = true;
    }
    vm.iter_result(result, false)
}

/// The match results of `rx` on `text`: every match when global, else the first.
fn collect_results(
    vm: &mut Interpreter,
    rx: ObjectRef,
    text: &JsStr,
    flags: &[u16],
) -> JsResult<Vec<Value>> {
    let global = has_flag(flags, b'g');
    if global {
        vm.set_last_index(rx, 0.0)?;
    }
    let units = text.to_utf16();
    let mut results = Vec::new();
    loop {
        let result = vm.regexp_exec(rx, text)?;
        if matches!(result, Value::Null) {
            return Ok(results);
        }
        vm.keep(&result);
        results.push(result.clone());
        if !global {
            return Ok(results);
        }
        let matched = matched_text(vm, &result)?;
        step_over_empty(vm, rx, &matched, &units, is_full_unicode(flags))?;
    }
}

fn regexp_replace(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (rx, text) = receiver_and_text(vm, this, args, "Symbol.replace")?;
    let units = text.to_utf16();
    let replace_value = arg(args, 1);
    let template = match vm.is_callable(&replace_value) {
        true => None,
        false => Some(vm.coerce_string(&replace_value)?.to_utf16()),
    };
    let flags = flags_of(vm, rx)?;
    let results = collect_results(vm, rx, &text, &flags)?;
    let mut out = Vec::new();
    let mut next_position = 0;
    for result in results {
        let object = vm
            .as_object(&result)
            .expect("RegExpExec results are objects");
        let captures_count = (vm.length_of(object)? - 1.0).max(0.0) as u32;
        let matched = matched_text(vm, &result)?.to_utf16();
        let position = vm.get_named(object, "index")?;
        let position = vm.coerce_integer(&position)?.clamp(0.0, units.len() as f64) as usize;
        let mut captures = Vec::new();
        for index in 1..=captures_count {
            let capture = vm.get_key(object, ObjectKey::Index(index))?;
            let capture = match capture {
                Value::Undefined => Value::Undefined,
                capture => Value::String(vm.coerce_string(&capture)?),
            };
            captures.push(capture);
        }
        let named = vm.get_named(object, "groups")?;
        let replacement = match &template {
            None => {
                let mut call_args = vec![string_value(matched.clone())];
                call_args.extend(captures);
                call_args.push(Value::Number(position as f64));
                call_args.push(Value::String(text.clone()));
                if !matches!(named, Value::Undefined) {
                    call_args.push(named);
                }
                let value = vm.call_value(&replace_value, &Value::Undefined, &call_args)?;
                vm.coerce_string(&value)?.to_utf16()
            }
            Some(template) => {
                let named = match named {
                    Value::Undefined => Value::Undefined,
                    named => object_value(vm.coerce_object(&named)?),
                };
                let matched = Substitution {
                    matched: &matched,
                    text: &units,
                    position,
                };
                get_substitution(vm, &matched, &captures, &named, template)?
            }
        };
        if position >= next_position {
            out.extend_from_slice(&units[next_position..position]);
            out.extend(replacement);
            next_position = position + matched.len();
        }
    }
    if next_position < units.len() {
        out.extend_from_slice(&units[next_position..]);
    }
    Ok(string_value(out))
}

/// The match a replacement template is expanded for.
pub(crate) struct Substitution<'a> {
    pub matched: &'a [u16],
    pub text: &'a [u16],
    pub position: usize,
}

/// GetSubstitution: expands `$$`, `$&`, `` $` ``, `$'`, `$n`/`$nn` and `$<name>`.
pub(crate) fn get_substitution(
    vm: &mut Interpreter,
    found: &Substitution,
    captures: &[Value],
    named: &Value,
    template: &[u16],
) -> JsResult<Vec<u16>> {
    let Substitution {
        matched,
        text,
        position,
    } = *found;
    let unit_char = |unit: Option<&u16>| unit.and_then(|&u| char::from_u32(u32::from(u)));
    let mut out = Vec::new();
    let mut i = 0;
    while i < template.len() {
        if template[i] != u16::from(b'$') || i + 1 == template.len() {
            out.push(template[i]);
            i += 1;
            continue;
        }
        let width = match unit_char(template.get(i + 1)) {
            Some('$') => {
                out.push(u16::from(b'$'));
                2
            }
            Some('&') => {
                out.extend_from_slice(matched);
                2
            }
            Some('`') => {
                out.extend_from_slice(&text[..position]);
                2
            }
            Some('\'') => {
                let tail = (position + matched.len()).min(text.len());
                out.extend_from_slice(&text[tail..]);
                2
            }
            Some('<') => named_reference(vm, &mut out, &template[i..], named)?,
            Some(c) if c.is_ascii_digit() => {
                let first = c as usize - '0' as usize;
                let second = unit_char(template.get(i + 2)).and_then(|c| c.to_digit(10));
                // `$nn` only when the two digits name an existing capture.
                let (index, width) = match second.map(|d| first * 10 + d as usize) {
                    Some(index) if index <= captures.len() => (index, 3),
                    _ => (first, 2),
                };
                match index.checked_sub(1).and_then(|k| captures.get(k)) {
                    Some(Value::String(capture)) => out.extend(capture.to_utf16()),
                    Some(_) => {}
                    None => out.extend_from_slice(&template[i..i + width]),
                }
                width
            }
            _ => {
                out.push(template[i]);
                1
            }
        };
        i += width;
    }
    Ok(out)
}

/// `$<name>` at the start of `rest`; returns the code units consumed.
fn named_reference(
    vm: &mut Interpreter,
    out: &mut Vec<u16>,
    rest: &[u16],
    named: &Value,
) -> JsResult<usize> {
    let close = rest.iter().position(|&u| u == u16::from(b'>'));
    let (Some(close), Some(groups)) = (close, vm.as_object(named)) else {
        out.extend_from_slice(&rest[..2]);
        return Ok(2);
    };
    let name = JsStr::from_utf16(rest[2..close].to_vec());
    let key = vm.string_key(&name);
    let capture = vm.get_key(groups, key)?;
    if !matches!(capture, Value::Undefined) {
        out.extend(vm.coerce_string(&capture)?.to_utf16());
    }
    Ok(close + 1)
}

fn regexp_search(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (rx, text) = receiver_and_text(vm, this, args, "Symbol.search")?;
    let previous = vm.get_named(rx, "lastIndex")?;
    vm.keep(&previous);
    let zero = Value::Number(0.0);
    if !same_value(&previous, &zero) {
        vm.set_last_index(rx, 0.0)?;
    }
    let result = vm.regexp_exec(rx, &text)?;
    let current = vm.get_named(rx, "lastIndex")?;
    if !same_value(&current, &previous) {
        let key = vm.realm.key_str("lastIndex");
        vm.set_or_throw(rx, key, previous)?;
    }
    match vm.as_object(&result) {
        Some(result) => vm.get_named(result, "index"),
        None => Ok(Value::Number(-1.0)),
    }
}

fn regexp_split(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    let (rx, text) = receiver_and_text(vm, this, args, "Symbol.split")?;
    let default = object_value(vm.intrinsic(Intrinsic::RegExpConstructor));
    let constructor = vm.species_constructor(rx, default)?;
    let mut flags = flags_of(vm, rx)?;
    let unicode = is_full_unicode(&flags);
    if !has_flag(&flags, b'y') {
        flags.push(u16::from(b'y'));
    }
    let splitter = vm.construct_object(&constructor, &[object_value(rx), string_value(flags)])?;
    let limit = match arg(args, 1) {
        Value::Undefined => u32::MAX,
        limit => to_uint32(vm.coerce_number(&limit)?),
    } as usize;
    let mut parts: Vec<Value> = Vec::new();
    if limit == 0 {
        return vm.array_from_list(&parts);
    }
    let units = text.to_utf16();
    let size = units.len();
    if size == 0 {
        if matches!(vm.regexp_exec(splitter, &text)?, Value::Null) {
            parts.push(Value::String(text));
        }
        return vm.array_from_list(&parts);
    }
    let (mut p, mut q) = (0, 0);
    while q < size {
        vm.set_last_index(splitter, q as f64)?;
        let result = vm.regexp_exec(splitter, &text)?;
        let Some(object) = vm.as_object(&result) else {
            q = advance(&units, q as f64, unicode) as usize;
            continue;
        };
        let end = (vm.last_index(splitter)? as usize).min(size);
        if end == p {
            q = advance(&units, q as f64, unicode) as usize;
            continue;
        }
        parts.push(string_value(units[p..q].to_vec()));
        if parts.len() == limit {
            return vm.array_from_list(&parts);
        }
        p = end;
        let captures = (vm.length_of(object)? - 1.0).max(0.0) as u32;
        for index in 1..=captures {
            let capture = vm.get_key(object, ObjectKey::Index(index))?;
            parts.push(capture);
            if parts.len() == limit {
                return vm.array_from_list(&parts);
            }
        }
        q = p;
    }
    parts.push(string_value(units[p..size].to_vec()));
    vm.array_from_list(&parts)
}
