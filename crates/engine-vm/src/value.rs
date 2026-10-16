//! The engine value: a 16-byte enum (NaN-boxing is out of scope).

use std::rc::Rc;

use velyr_engine_api::JsBigInt;
use velyr_engine_gc::GcRef;

use crate::string::JsStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SymbolId(pub u32);

#[derive(Debug, Clone, Default)]
pub enum Value {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(JsStr),
    Symbol(SymbolId),
    BigInt(Rc<JsBigInt>),
    Object(GcRef),
}

const _: () = assert!(size_of::<Value>() == 16);

/// Thrown by an abstract operation (`TypeError: <message>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeError(pub String);

pub type OpResult<T> = Result<T, TypeError>;

impl Value {
    pub fn str(s: &str) -> Self {
        Value::String(JsStr::from_str(s))
    }
}
