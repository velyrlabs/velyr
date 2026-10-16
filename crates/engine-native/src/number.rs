//! Operators of the bootstrap VM on engine values: `+` and the numeric binary operators.

use velyr_engine_gc::GcRef;
use velyr_engine_vm::{Hint, JsStr, OpResult, Value, to_number, to_primitive, to_string};

use crate::compiler::JsResult;
use crate::value_bridge::exception;

/// Every object in the bootstrap is a host function; its primitive form is its source text.
fn host_function_primitive(_: GcRef, _: Hint) -> OpResult<Value> {
    Ok(Value::String(JsStr::from_str(
        "function () { [native code] }",
    )))
}

pub fn to_num(value: &Value) -> JsResult<f64> {
    to_number(value, &mut host_function_primitive).map_err(exception)
}

/// ECMA-262 13.15.3 ApplyStringOrNumericBinaryOperator for `+`.
pub fn add(a: &Value, b: &Value) -> JsResult<Value> {
    let a = to_primitive(a, Hint::Default, &mut host_function_primitive).map_err(exception)?;
    let b = to_primitive(b, Hint::Default, &mut host_function_primitive).map_err(exception)?;
    if matches!(a, Value::String(_)) || matches!(b, Value::String(_)) {
        let a = to_string(&a, &mut host_function_primitive).map_err(exception)?;
        let b = to_string(&b, &mut host_function_primitive).map_err(exception)?;
        return Ok(Value::String(a.concat(&b)));
    }
    Ok(Value::Number(to_num(&a)? + to_num(&b)?))
}

#[cfg(test)]
mod tests {
    use velyr_engine_vm::SymbolId;

    use super::*;

    #[test]
    fn add_concatenates_when_either_side_is_a_string() {
        let sum = add(&Value::str("a"), &Value::Number(1.0)).unwrap();
        assert!(matches!(sum, Value::String(s) if s.to_string_lossy() == "a1"));
        let sum = add(&Value::Null, &Value::str("x")).unwrap();
        assert!(matches!(sum, Value::String(s) if s.to_string_lossy() == "nullx"));
    }

    #[test]
    fn add_sums_numbers_and_coerces_primitives() {
        let sum = add(&Value::Bool(true), &Value::Number(1.0)).unwrap();
        assert!(matches!(sum, Value::Number(n) if n == 2.0));
        let sum = add(&Value::Undefined, &Value::Number(1.0)).unwrap();
        assert!(matches!(sum, Value::Number(n) if n.is_nan()));
    }

    #[test]
    fn symbols_do_not_coerce() {
        let error = add(&Value::Symbol(SymbolId(0)), &Value::Number(1.0)).unwrap_err();
        assert!(error.message.starts_with("TypeError:"), "{}", error.message);
    }
}
