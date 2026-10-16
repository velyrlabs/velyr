//! `String.prototype` methods that delegate to a pattern's well-known symbol method:
//! `split`, `replace`, `replaceAll`, `match`, `matchAll` and `search`. String patterns are
//! handled here; other patterns become a RegExp (RegExpCreate) and use its symbol method.

use super::super::regexp::{Substitution, get_substitution};
use super::super::symbol::{
    SYMBOL_MATCH, SYMBOL_MATCH_ALL, SYMBOL_REPLACE, SYMBOL_SEARCH, SYMBOL_SPLIT,
};
use super::super::{Interpreter, JsResult, arg, object_value};
use super::access::index_of;
use super::{string_value, this_units};
use crate::interpreter::operators::to_uint32;
use crate::{ObjectKey, ObjectRef, SymbolId, Value};

pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
    vm.method(proto, "match", 1, string_match)?;
    vm.method(proto, "matchAll", 1, string_match_all)?;
    vm.method(proto, "replace", 2, string_replace)?;
    vm.method(proto, "replaceAll", 2, string_replace_all)?;
    vm.method(proto, "search", 1, string_search)?;
    vm.method(proto, "split", 2, string_split)
}

/// Calls `pattern[symbol](this, ...rest)` when the pattern is an object with that method.
fn delegate(
    vm: &mut Interpreter,
    pattern: &Value,
    symbol: SymbolId,
    this: &Value,
    rest: &[Value],
) -> JsResult<Option<Value>> {
    // ES2025: only objects are asked for their symbol method (primitives are not).
    if !matches!(pattern, Value::Object(_)) {
        return Ok(None);
    }
    let Some(method) = vm.get_method(pattern, ObjectKey::Symbol(symbol))? else {
        return Ok(None);
    };
    let mut args = vec![this.clone()];
    args.extend_from_slice(rest);
    vm.call_value(&method, pattern, &args).map(Some)
}

/// `Invoke(RegExpCreate(pattern, flags), symbol, [ToString(this)])`.
fn invoke_on_regexp(
    vm: &mut Interpreter,
    this: &Value,
    pattern: &Value,
    flags: &str,
    symbol: SymbolId,
) -> JsResult<Value> {
    let text = Value::String(vm.coerce_string(this)?);
    vm.keep(&text);
    let flags = match flags {
        "" => Value::Undefined,
        flags => Value::str(flags),
    };
    let rx = object_value(vm.regexp_create(pattern, &flags)?);
    let method = vm.get_value(&rx, ObjectKey::Symbol(symbol), None)?;
    vm.call_function(&method, &rx, &[text])
}

fn string_match(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    vm.require_coercible(this, "String.prototype.match")?;
    let pattern = arg(args, 0);
    if let Some(result) = delegate(vm, &pattern, SYMBOL_MATCH, this, &[])? {
        return Ok(result);
    }
    invoke_on_regexp(vm, this, &pattern, "", SYMBOL_MATCH)
}

fn string_match_all(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    vm.require_coercible(this, "String.prototype.matchAll")?;
    let pattern = arg(args, 0);
    if vm.is_regexp(&pattern)? {
        require_global_flag(vm, &pattern, "matchAll")?;
    }
    if let Some(result) = delegate(vm, &pattern, SYMBOL_MATCH_ALL, this, &[])? {
        return Ok(result);
    }
    invoke_on_regexp(vm, this, &pattern, "g", SYMBOL_MATCH_ALL)
}

fn string_search(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    vm.require_coercible(this, "String.prototype.search")?;
    let pattern = arg(args, 0);
    if let Some(result) = delegate(vm, &pattern, SYMBOL_SEARCH, this, &[])? {
        return Ok(result);
    }
    invoke_on_regexp(vm, this, &pattern, "", SYMBOL_SEARCH)
}

fn require_global_flag(vm: &mut Interpreter, pattern: &Value, method: &str) -> JsResult<()> {
    let object = vm.as_object(pattern).expect("IsRegExp values are objects");
    let flags = vm.get_named(object, "flags")?;
    vm.require_coercible(&flags, "flags")?;
    let flags = vm.coerce_string(&flags)?.to_utf16();
    if !flags.contains(&u16::from(b'g')) {
        let message = format!("String.prototype.{method} called with a non-global RegExp argument");
        return vm.throw_type(&message);
    }
    Ok(())
}

/// The replacement for one match: the functional result or the substitution template.
fn replacement_for(
    vm: &mut Interpreter,
    replace: &Value,
    template: &Option<Vec<u16>>,
    matched: &[u16],
    text: &[u16],
    position: usize,
) -> JsResult<Vec<u16>> {
    match template {
        Some(template) => {
            let found = Substitution {
                matched,
                text,
                position,
            };
            get_substitution(vm, &found, &[], &Value::Undefined, template)
        }
        None => {
            let args = [
                string_value(matched.to_vec()),
                Value::Number(position as f64),
                string_value(text.to_vec()),
            ];
            let result = vm.call_value(replace, &Value::Undefined, &args)?;
            Ok(vm.coerce_string(&result)?.to_utf16())
        }
    }
}

fn replace(vm: &mut Interpreter, this: &Value, args: &[Value], all: bool) -> JsResult<Value> {
    let method = if all { "replaceAll" } else { "replace" };
    vm.require_coercible(this, &format!("String.prototype.{method}"))?;
    let search_value = arg(args, 0);
    let replace_value = arg(args, 1);
    if all && vm.is_regexp(&search_value)? {
        require_global_flag(vm, &search_value, method)?;
    }
    let rest = [this.clone(), replace_value.clone()];
    if let Some(result) = delegate(vm, &search_value, SYMBOL_REPLACE, &rest[0], &rest[1..])? {
        return Ok(result);
    }
    let text = vm.coerce_string(this)?.to_utf16();
    let search = vm.coerce_string(&search_value)?.to_utf16();
    let template = match vm.is_callable(&replace_value) {
        true => None,
        false => Some(vm.coerce_string(&replace_value)?.to_utf16()),
    };
    let mut positions = Vec::new();
    let mut from = 0;
    while let Some(position) = index_of(&text, &search, from) {
        positions.push(position);
        if !all {
            break;
        }
        from = position + search.len().max(1);
    }
    let mut out = Vec::new();
    let mut end_of_last = 0;
    for position in positions {
        out.extend_from_slice(&text[end_of_last..position]);
        let piece = replacement_for(vm, &replace_value, &template, &search, &text, position)?;
        out.extend(piece);
        end_of_last = position + search.len();
    }
    out.extend_from_slice(&text[end_of_last.min(text.len())..]);
    Ok(string_value(out))
}

fn string_replace(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    replace(vm, this, args, false)
}

fn string_replace_all(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    replace(vm, this, args, true)
}

fn string_split(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
    vm.require_coercible(this, "String.prototype.split")?;
    let separator = arg(args, 0);
    let limit = arg(args, 1);
    if let Some(result) = delegate(
        vm,
        &separator,
        SYMBOL_SPLIT,
        this,
        std::slice::from_ref(&limit),
    )? {
        return Ok(result);
    }
    let text = this_units(vm, this, "split")?;
    let limit = match limit {
        Value::Undefined => u32::MAX,
        value => to_uint32(vm.coerce_number(&value)?),
    } as usize;
    let separator_text = vm.coerce_string(&separator)?.to_utf16();
    if limit == 0 {
        return vm.array_from_list(&[]);
    }
    if matches!(separator, Value::Undefined) {
        return vm.array_from_list(&[string_value(text)]);
    }
    let mut parts: Vec<Value> = Vec::new();
    if separator_text.is_empty() {
        parts.extend(
            text.iter()
                .take(limit)
                .map(|unit| string_value(vec![*unit])),
        );
        return vm.array_from_list(&parts);
    }
    if text.is_empty() {
        return vm.array_from_list(&[string_value(text)]);
    }
    let mut start = 0;
    while let Some(found) = index_of(&text, &separator_text, start) {
        parts.push(string_value(text[start..found].to_vec()));
        if parts.len() == limit {
            return vm.array_from_list(&parts);
        }
        start = found + separator_text.len();
    }
    parts.push(string_value(text[start..].to_vec()));
    vm.array_from_list(&parts)
}
