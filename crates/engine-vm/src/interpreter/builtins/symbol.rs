//! Symbols: the realm's symbol table (descriptions, the global registry, well-known
//! symbols) and the `Symbol` constructor and prototype (ECMA-262 §20.4).

use std::collections::HashMap;

use super::slots::Slot;
use super::{Interpreter, Intrinsic, JsResult, arg};
use crate::{JsStr, ObjectKey, PropertyDescriptor, SymbolId, Value};

pub(crate) const SYMBOL_ITERATOR: SymbolId = SymbolId(0);
pub(crate) const SYMBOL_HAS_INSTANCE: SymbolId = SymbolId(4);
pub(crate) const SYMBOL_IS_CONCAT_SPREADABLE: SymbolId = SymbolId(5);
pub(crate) const SYMBOL_MATCH: SymbolId = SymbolId(6);
pub(crate) const SYMBOL_MATCH_ALL: SymbolId = SymbolId(7);
pub(crate) const SYMBOL_REPLACE: SymbolId = SymbolId(8);
pub(crate) const SYMBOL_SEARCH: SymbolId = SymbolId(9);
pub(crate) const SYMBOL_SPECIES: SymbolId = SymbolId(10);
pub(crate) const SYMBOL_SPLIT: SymbolId = SymbolId(11);
pub(crate) const SYMBOL_TO_PRIMITIVE: SymbolId = SymbolId(12);
pub(crate) const SYMBOL_TO_STRING_TAG: SymbolId = SymbolId(13);
pub(crate) const SYMBOL_UNSCOPABLES: SymbolId = SymbolId(14);
pub(crate) const SYMBOL_DISPOSE: SymbolId = SymbolId(15);
pub(crate) const SYMBOL_ASYNC_DISPOSE: SymbolId = SymbolId(16);

/// Ids 1 and 2 are retired internal ids; user symbols start after the well-known ones.
const FIRST_USER_SYMBOL: u32 = 18;

const WELL_KNOWN: [(&str, u32); 15] = [
    ("asyncDispose", 16),
    ("asyncIterator", 3),
    ("dispose", 15),
    ("hasInstance", 4),
    ("isConcatSpreadable", 5),
    ("iterator", 0),
    ("match", 6),
    ("matchAll", 7),
    ("replace", 8),
    ("search", 9),
    ("species", 10),
    ("split", 11),
    ("toPrimitive", 12),
    ("toStringTag", 13),
    ("unscopables", 14),
];

pub(crate) struct SymbolTable {
    /// Indexed by symbol id (private names are never described).
    descriptions: Vec<Option<JsStr>>,
    registry: HashMap<JsStr, SymbolId>,
    registered: HashMap<SymbolId, JsStr>,
}

impl Default for SymbolTable {
    fn default() -> Self {
        let mut descriptions = vec![None; FIRST_USER_SYMBOL as usize];
        for (name, id) in WELL_KNOWN {
            descriptions[id as usize] = Some(JsStr::from_str(&format!("Symbol.{name}")));
        }
        Self {
            descriptions,
            registry: HashMap::new(),
            registered: HashMap::new(),
        }
    }
}

impl SymbolTable {
    pub fn create(&mut self, description: Option<JsStr>) -> SymbolId {
        let id = SymbolId(self.descriptions.len() as u32);
        self.descriptions.push(description);
        id
    }

    pub fn description(&self, symbol: SymbolId) -> Option<JsStr> {
        self.descriptions.get(symbol.0 as usize).cloned().flatten()
    }
}

impl Interpreter {
    /// The description of `symbol`, or `""` (for function names).
    pub(crate) fn symbol_description_text(&self, symbol: SymbolId) -> String {
        self.state
            .symbols
            .description(symbol)
            .map(|d| d.to_string_lossy())
            .unwrap_or_default()
    }

    /// SymbolDescriptiveString.
    pub(crate) fn symbol_descriptive_string(&self, symbol: SymbolId) -> JsStr {
        let description = self
            .state
            .symbols
            .description(symbol)
            .unwrap_or(JsStr::from_str(""));
        JsStr::from_str("Symbol(")
            .concat(&description)
            .concat(&JsStr::from_str(")"))
    }

    /// Whether `symbol` can be held weakly (not in the global registry).
    pub(crate) fn can_be_held_weakly(&self, symbol: SymbolId) -> bool {
        !self.state.symbols.registered.contains_key(&symbol)
    }
}

pub(super) fn install(vm: &mut Interpreter) -> JsResult<()> {
    let proto = vm.intrinsic(Intrinsic::SymbolPrototype);
    let constructor = vm.constructor("Symbol", 0, symbol_constructor, proto)?;
    for (name, id) in WELL_KNOWN {
        vm.constant(constructor, name, Value::Symbol(SymbolId(id)))?;
    }
    vm.method(constructor, "for", 1, symbol_for)?;
    vm.method(constructor, "keyFor", 1, symbol_key_for)?;
    vm.method(proto, "toString", 0, symbol_to_string)?;
    vm.method(proto, "valueOf", 0, symbol_value_of)?;
    let key = vm.realm.key_str("description");
    vm.accessor(proto, key, "description", symbol_description, None)?;
    let to_primitive = vm.symbol_method(proto, SYMBOL_TO_PRIMITIVE, 1, symbol_value_of)?;
    let desc = PropertyDescriptor::data(to_primitive, false, false, true);
    vm.define_property(proto, ObjectKey::Symbol(SYMBOL_TO_PRIMITIVE), &desc)?;
    vm.define_to_string_tag(proto, "Symbol")
}

impl Interpreter {
    /// `@@toStringTag`: non-writable, non-enumerable, configurable.
    pub(crate) fn define_to_string_tag(
        &mut self,
        object: crate::ObjectRef,
        tag: &str,
    ) -> JsResult<()> {
        let desc = PropertyDescriptor::data(Value::str(tag), false, false, true);
        self.define_property(object, ObjectKey::Symbol(SYMBOL_TO_STRING_TAG), &desc)?;
        Ok(())
    }
}

fn symbol_constructor(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    if !matches!(vm.new_target(), Value::Undefined) {
        return vm.throw_type("Symbol is not a constructor");
    }
    let description = match arg(args, 0) {
        Value::Undefined => None,
        value => Some(vm.coerce_string(&value)?),
    };
    Ok(Value::Symbol(vm.state.symbols.create(description)))
}

fn symbol_for(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let key = vm.coerce_string(&arg(args, 0))?;
    if let Some(symbol) = vm.state.symbols.registry.get(&key) {
        return Ok(Value::Symbol(*symbol));
    }
    let symbol = vm.state.symbols.create(Some(key.clone()));
    vm.state.symbols.registry.insert(key.clone(), symbol);
    vm.state.symbols.registered.insert(symbol, key);
    Ok(Value::Symbol(symbol))
}

fn symbol_key_for(vm: &mut Interpreter, _: &Value, args: &[Value]) -> JsResult<Value> {
    let Value::Symbol(symbol) = arg(args, 0) else {
        return vm.throw_type("Symbol.keyFor: argument is not a symbol");
    };
    Ok(match vm.state.symbols.registered.get(&symbol) {
        Some(key) => Value::String(key.clone()),
        None => Value::Undefined,
    })
}

/// thisSymbolValue.
fn this_symbol(vm: &mut Interpreter, this: &Value, method: &str) -> JsResult<SymbolId> {
    match this {
        Value::Symbol(symbol) => Ok(*symbol),
        _ => match vm.value_slot(this) {
            Some(Slot::Symbol(symbol)) => Ok(symbol),
            _ => vm.throw_type(&format!("{method} requires that 'this' be a Symbol")),
        },
    }
}

fn symbol_to_string(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let symbol = this_symbol(vm, this, "Symbol.prototype.toString")?;
    Ok(Value::String(vm.symbol_descriptive_string(symbol)))
}

fn symbol_value_of(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let symbol = this_symbol(vm, this, "Symbol.prototype.valueOf")?;
    Ok(Value::Symbol(symbol))
}

fn symbol_description(vm: &mut Interpreter, this: &Value, _: &[Value]) -> JsResult<Value> {
    let symbol = this_symbol(vm, this, "Symbol.prototype.description")?;
    Ok(vm
        .state
        .symbols
        .description(symbol)
        .map_or(Value::Undefined, Value::String))
}
