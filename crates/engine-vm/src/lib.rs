//! Value model and ECMAScript abstract operations for the native engine (ADR-009, Track C).

mod atom;
mod bigint;
mod bigint_ops;
pub mod bytecode;
mod convert;
mod equality;
pub mod exception;
pub mod interpreter;
mod object;
mod string;
mod value;

pub use atom::{Atom, Interner};
pub use convert::*;
pub use equality::{loose_equals, same_value, same_value_zero, strict_equals};
pub use object::{
    CallHook, InlineCache, InternalMethods, JsObject, MAX_ARRAY_INDEX, ObjectError, ObjectKey,
    ObjectRef, ObjectResult, Property, PropertyDescriptor, PropertyValue, Realm, ShapeId,
    ShapeTable,
};
pub use string::{Flat, JsStr};
pub use value::{OpResult, SymbolId, TypeError, Value};
