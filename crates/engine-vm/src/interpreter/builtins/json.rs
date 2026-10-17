//! The `JSON` namespace object (ECMA-262 §25.5): `JSON.parse` (with reviver) lives here,
//! `JSON.stringify` in the `stringify` submodule.

mod source;
mod stringify;

use super::{Interpreter, JsResult, arg, object_value};
use crate::{JsStr, ObjectKey, ObjectRef, Value};
use source::SourceNode;

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let json = vm.new_object()?;
    let global = vm.state.intrinsics.global;
    vm.define_method(global, "JSON", object_value(json))?;
    vm.method(json, "parse", 2, json_parse)?;
    vm.method(json, "stringify", 3, stringify::json_stringify)?;
    vm.method(json, "rawJSON", 1, source::raw_json)?;
    vm.method(json, "isRawJSON", 1, source::is_raw_json)?;
    vm.define_to_string_tag(json, "JSON")
}

/// The enumerable own string keys of `object`, in [[OwnPropertyKeys]] order.
pub(super) fn enumerable_keys(vm: &mut Interpreter, object: ObjectRef) -> JsResult<Vec<ObjectKey>> {
    let mut keys = Vec::new();
    for key in vm.own_keys(object)? {
        if matches!(key, ObjectKey::Symbol(_)) {
            continue;
        }
        if vm.own_property(object, key)?.is_some_and(|p| p.enumerable) {
            keys.push(key);
        }
    }
    Ok(keys)
}

fn json_parse(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let text = vm.coerce_string(&arg(args, 0))?.to_utf16();
    let reviver = arg(args, 1);
    let mut parser = Parser {
        units: &text,
        position: 0,
        track: vm.is_callable(&reviver),
        node: None,
    };
    parser.skip_whitespace();
    let value = parser.value(vm)?;
    parser.skip_whitespace();
    if parser.position < text.len() {
        return parser.error(vm);
    }
    if !vm.is_callable(&reviver) {
        return Ok(value);
    }
    let node = parser.node.take();
    let root = vm.new_object()?;
    let empty = vm.string_key(&JsStr::from_str(""));
    vm.create_data_or_throw(root, empty, value)?;
    internalize(vm, root, empty, &reviver, node.as_ref())
}

/// InternalizeJSONProperty.
fn internalize(
    vm: &mut Interpreter,
    holder: ObjectRef,
    name: ObjectKey,
    reviver: &Value,
    node: Option<&SourceNode>,
) -> JsResult<Value> {
    let value = vm.get_key(holder, name)?;
    vm.keep(&value);
    let node = SourceNode::matching(node, &value);
    if let Some(object) = vm.as_object(&value) {
        let keys = match vm.is_array(&value)? {
            true => {
                let length = vm.length_of(object)?;
                let mut keys = Vec::new();
                let mut index = 0.0;
                while index < length {
                    keys.push(vm.index_key(index));
                    index += 1.0;
                }
                keys
            }
            false => enumerable_keys(vm, object)?,
        };
        for key in keys {
            let child = node.and_then(|node| node.child(key));
            let element = internalize(vm, object, key, reviver, child)?;
            if matches!(element, Value::Undefined) {
                vm.delete_key(object, key)?;
            } else {
                let desc = crate::PropertyDescriptor::data(element, true, true, true);
                vm.define_own(object, key, &desc)?;
            }
        }
    }
    let name = vm.key_to_value(name);
    let context = vm.new_object()?;
    if let Some(source) = node.and_then(SourceNode::source) {
        let key = vm.realm.key_str("source");
        vm.create_data_or_throw(context, key, Value::String(source.clone()))?;
    }
    let context = object_value(context);
    vm.call_value(reviver, &object_value(holder), &[name, value, context])
}

struct Parser<'a> {
    units: &'a [u16],
    position: usize,
    /// Build `node` for each parsed value (only when a reviver will read it).
    track: bool,
    /// Source node of the value parsed last.
    node: Option<SourceNode>,
}

impl Parser<'_> {
    fn error<T>(&self, vm: &mut Interpreter) -> JsResult<T> {
        let message = match self.units.get(self.position) {
            Some(&unit) => format!(
                "Unexpected token '{}' in JSON at position {}",
                char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}'),
                self.position
            ),
            None => "Unexpected end of JSON input".to_string(),
        };
        Err(vm.error(crate::interpreter::ErrorKind::Syntax, &message))
    }

    fn peek(&self) -> Option<u8> {
        let unit = *self.units.get(self.position)?;
        u8::try_from(unit).ok().or(Some(0xFF))
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn expect_word(&mut self, vm: &mut Interpreter, word: &str, value: Value) -> JsResult<Value> {
        for byte in word.bytes() {
            if self.peek() != Some(byte) {
                return self.error(vm);
            }
            self.position += 1;
        }
        Ok(value)
    }

    fn value(&mut self, vm: &mut Interpreter) -> JsResult<Value> {
        let start = self.position;
        let value = self.parse_value(vm)?;
        if self.track && !matches!(self.units[start], 0x7B | 0x5B) {
            let source = JsStr::from_utf16(self.units[start..self.position].to_vec());
            self.node = Some(SourceNode::Primitive {
                value: value.clone(),
                source,
            });
        }
        Ok(value)
    }

    fn parse_value(&mut self, vm: &mut Interpreter) -> JsResult<Value> {
        match self.peek() {
            Some(b'{') => self.object(vm),
            Some(b'[') => self.array(vm),
            Some(b'"') => Ok(Value::String(JsStr::from_utf16(self.string(vm)?))),
            Some(b't') => self.expect_word(vm, "true", Value::Bool(true)),
            Some(b'f') => self.expect_word(vm, "false", Value::Bool(false)),
            Some(b'n') => self.expect_word(vm, "null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(vm),
            _ => self.error(vm),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.position;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.position += 1;
        }
        self.position - start
    }

    fn number(&mut self, vm: &mut Interpreter) -> JsResult<Value> {
        let start = self.position;
        if self.peek() == Some(b'-') {
            self.position += 1;
        }
        if self.peek() == Some(b'0') {
            self.position += 1;
        } else if self.digits() == 0 {
            return self.error(vm);
        }
        if self.peek() == Some(b'.') {
            self.position += 1;
            if self.digits() == 0 {
                return self.error(vm);
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.position += 1;
            }
            if self.digits() == 0 {
                return self.error(vm);
            }
        }
        let text: String = self.units[start..self.position]
            .iter()
            .map(|&u| u as u8 as char)
            .collect();
        Ok(Value::Number(text.parse().unwrap_or(f64::NAN)))
    }

    fn hex4(&mut self, vm: &mut Interpreter) -> JsResult<u16> {
        let mut code = 0u16;
        for _ in 0..4 {
            let digit = self.peek().and_then(|b| char::from(b).to_digit(16));
            let Some(digit) = digit else {
                return self.error(vm);
            };
            code = code * 16 + digit as u16;
            self.position += 1;
        }
        Ok(code)
    }

    fn string(&mut self, vm: &mut Interpreter) -> JsResult<Vec<u16>> {
        self.position += 1;
        let mut out = Vec::new();
        loop {
            let Some(&unit) = self.units.get(self.position) else {
                return self.error(vm);
            };
            match unit {
                0x22 => {
                    self.position += 1;
                    return Ok(out);
                }
                0x5C => {
                    self.position += 1;
                    let escaped = match self.peek() {
                        Some(b'"') => 0x22,
                        Some(b'\\') => 0x5C,
                        Some(b'/') => 0x2F,
                        Some(b'b') => 0x08,
                        Some(b'f') => 0x0C,
                        Some(b'n') => 0x0A,
                        Some(b'r') => 0x0D,
                        Some(b't') => 0x09,
                        Some(b'u') => {
                            self.position += 1;
                            out.push(self.hex4(vm)?);
                            continue;
                        }
                        _ => return self.error(vm),
                    };
                    out.push(escaped);
                    self.position += 1;
                }
                0..0x20 => return self.error(vm),
                _ => {
                    out.push(unit);
                    self.position += 1;
                }
            }
        }
    }

    fn array(&mut self, vm: &mut Interpreter) -> JsResult<Value> {
        self.position += 1;
        let array = vm.array_create(0.0, None)?;
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.position += 1;
            return Ok(object_value(array));
        }
        let mut index = 0;
        let mut items = Vec::new();
        loop {
            self.skip_whitespace();
            let element = self.value(vm)?;
            items.extend(self.node.take());
            vm.create_data_property(array, ObjectKey::Index(index), element)?;
            index += 1;
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b']') => {
                    self.position += 1;
                    let value = object_value(array);
                    if self.track {
                        self.node = Some(SourceNode::Array {
                            value: value.clone(),
                            items,
                        });
                    }
                    return Ok(value);
                }
                _ => return self.error(vm),
            }
        }
    }

    fn finish_object(&mut self, object: ObjectRef, members: Vec<(ObjectKey, SourceNode)>) -> Value {
        let value = object_value(object);
        if self.track {
            self.node = Some(SourceNode::Object {
                value: value.clone(),
                members,
            });
        }
        value
    }

    fn object(&mut self, vm: &mut Interpreter) -> JsResult<Value> {
        self.position += 1;
        let object = vm.new_object()?;
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.position += 1;
            return Ok(self.finish_object(object, Vec::new()));
        }
        let mut members = Vec::new();
        loop {
            self.skip_whitespace();
            if self.peek() != Some(b'"') {
                return self.error(vm);
            }
            let name = JsStr::from_utf16(self.string(vm)?);
            self.skip_whitespace();
            if self.peek() != Some(b':') {
                return self.error(vm);
            }
            self.position += 1;
            self.skip_whitespace();
            let value = self.value(vm)?;
            let key = vm.string_key(&name);
            if let Some(node) = self.node.take() {
                members.retain(|(k, _)| *k != key);
                members.push((key, node));
            }
            vm.create_data_property(object, key, value)?;
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.position += 1,
                Some(b'}') => {
                    self.position += 1;
                    return Ok(self.finish_object(object, members));
                }
                _ => return self.error(vm),
            }
        }
    }
}
