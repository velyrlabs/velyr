//! JSON.parse source text access (proposal-json-parse-with-source): the parse-time
//! `SourceNode` tree handed to revivers, and `JSON.rawJSON` / `JSON.isRawJSON`.

use super::super::slots::Slot;
use super::super::{Interpreter, JsResult, arg, object_value};
use super::Parser;
use crate::equality::same_value;
use crate::{JsStr, ObjectKey, Value};

/// Parse-time shape of a JSON value. `value` is what the parser produced, compared with
/// the live property (SameValue) before the source text is exposed to the reviver.
pub(super) enum SourceNode {
    Primitive {
        value: Value,
        source: JsStr,
    },
    Array {
        value: Value,
        items: Vec<SourceNode>,
    },
    Object {
        value: Value,
        members: Vec<(ObjectKey, SourceNode)>,
    },
}

impl SourceNode {
    fn value(&self) -> &Value {
        match self {
            Self::Primitive { value, .. }
            | Self::Array { value, .. }
            | Self::Object { value, .. } => value,
        }
    }

    /// This node if the property still holds the value parsed from it.
    pub(super) fn matching<'a>(node: Option<&'a Self>, current: &Value) -> Option<&'a Self> {
        node.filter(|node| same_value(node.value(), current))
    }

    /// The source text, for an unmodified primitive.
    pub(super) fn source(&self) -> Option<&JsStr> {
        match self {
            Self::Primitive { source, .. } => Some(source),
            _ => None,
        }
    }

    pub(super) fn child(&self, key: ObjectKey) -> Option<&Self> {
        match (self, key) {
            (Self::Array { items, .. }, ObjectKey::Index(index)) => items.get(index as usize),
            (Self::Object { members, .. }, key) => members
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, node)| node),
            _ => None,
        }
    }
}

const JSON_WHITESPACE: [u16; 4] = [0x09, 0x0A, 0x0D, 0x20];

pub(super) fn raw_json(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let text = vm.coerce_string(&arg(args, 0))?;
    let units = text.to_utf16();
    let first = units.first().copied();
    let last = units.last().copied();
    let invalid = first.is_none_or(|u| JSON_WHITESPACE.contains(&u) || u == 0x7B || u == 0x5B)
        || last.is_some_and(|u| JSON_WHITESPACE.contains(&u));
    let mut parser = Parser {
        units: &units,
        position: 0,
        track: false,
        node: None,
    };
    if invalid {
        return parser.error(vm);
    }
    parser.value(vm)?;
    if parser.position < units.len() {
        return parser.error(vm);
    }
    let object = vm.new_object_with(None)?;
    let key = vm.realm.key_str("rawJSON");
    vm.create_data_or_throw(object, key, Value::String(text))?;
    vm.set_slot(object, Slot::RawJson);
    vm.set_integrity(object, true)?;
    Ok(object_value(object))
}

pub(super) fn is_raw_json(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    Ok(Value::Bool(matches!(
        vm.value_slot(&arg(args, 0)),
        Some(Slot::RawJson)
    )))
}
