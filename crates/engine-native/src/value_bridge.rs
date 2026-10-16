//! Conversions between `velyr-engine-vm` values and the `velyr-engine-api` boundary types.

use std::rc::Rc;

use velyr_engine_api::{Handle, JsException, Value as ApiValue};
use velyr_engine_gc::GcRef;
use velyr_engine_vm::{JsStr, TypeError, Value};

use crate::compiler::{JsResult, throw};

/// Handles map 1:1 onto `GcRef`; the caller validates that the slot exists.
pub fn import(value: &ApiValue) -> Value {
    match value {
        ApiValue::Undefined => Value::Undefined,
        ApiValue::Null => Value::Null,
        ApiValue::Bool(b) => Value::Bool(*b),
        ApiValue::Number(n) => Value::Number(*n),
        ApiValue::String(s) => Value::String(JsStr::from(s)),
        ApiValue::BigInt(b) => Value::BigInt(Rc::new(b.clone())),
        ApiValue::Object(h) => Value::Object(GcRef::new(h.id, h.generation)),
    }
}

/// Symbols have no representation in `engine-api` yet.
pub fn export(value: &Value) -> JsResult<ApiValue> {
    Ok(match value {
        Value::Undefined => ApiValue::Undefined,
        Value::Null => ApiValue::Null,
        Value::Bool(b) => ApiValue::Bool(*b),
        Value::Number(n) => ApiValue::Number(*n),
        Value::String(s) => ApiValue::String(s.to_js_string()),
        Value::BigInt(b) => ApiValue::BigInt((**b).clone()),
        Value::Object(r) => ApiValue::Object(Handle {
            id: r.index(),
            generation: r.generation(),
        }),
        Value::Symbol(_) => return Err(throw("TypeError", "Symbol cannot cross the engine API")),
    })
}

pub fn exception(error: TypeError) -> JsException {
    throw("TypeError", error.0)
}

#[cfg(test)]
mod tests {
    use velyr_engine_api::{JsBigInt, JsString};
    use velyr_engine_vm::SymbolId;

    use super::*;

    #[test]
    fn round_trips_every_api_value() {
        let values = [
            ApiValue::Undefined,
            ApiValue::Null,
            ApiValue::Bool(true),
            ApiValue::Number(1.5),
            ApiValue::String(JsString::from_utf16(vec![0x20ac, 0xD800])),
            ApiValue::BigInt(JsBigInt::from_i64(-9)),
            ApiValue::Object(Handle {
                id: 3,
                generation: 2,
            }),
        ];
        for value in values {
            assert_eq!(export(&import(&value)).unwrap(), value);
        }
    }

    #[test]
    fn symbols_cannot_be_exported() {
        let error = export(&Value::Symbol(SymbolId(1))).unwrap_err();
        assert!(error.message.starts_with("TypeError:"), "{}", error.message);
    }
}
