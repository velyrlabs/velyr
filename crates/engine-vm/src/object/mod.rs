//! Object model (VELYR-102): shapes with slots and a dictionary fallback, separate indexed
//! elements, full property descriptors, ordinary and Array exotic internal methods
//! (ECMA-262 §10.1, §10.4.2) and a monomorphic inline cache.

mod arguments;
mod array;
mod descriptor;
mod inline_cache;
mod key;
mod realm;
mod shape;
mod storage;

use velyr_engine_gc::{Gc, Trace, Tracer};

pub(crate) use arguments::ParameterMap;
pub(crate) use descriptor::validate_and_apply;
pub use descriptor::{Property, PropertyDescriptor, PropertyValue};
pub use inline_cache::InlineCache;
pub use key::{MAX_ARRAY_INDEX, ObjectKey};
pub use realm::{CallHook, InternalMethods, Realm};
pub use shape::{ShapeId, ShapeTable};

use crate::{TypeError, Value};
use storage::{Elements, NamedProperties};

pub type ObjectRef = Gc<JsObject>;

/// Abrupt completion of an internal method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectError {
    Type(String),
    Range(String),
}

impl From<TypeError> for ObjectError {
    fn from(error: TypeError) -> Self {
        Self::Type(error.0)
    }
}

pub type ObjectResult<T> = Result<T, ObjectError>;

/// Selects the internal-method behavior. Proxy and other exotic objects get their own
/// variant and dispatch arm in `InternalMethods for Realm`.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ObjectKind {
    Ordinary,
    /// `length` is virtual: never in the named storage.
    Array {
        length: u32,
        length_writable: bool,
    },
    /// Sloppy-mode arguments object whose mapped indices alias parameters (§10.4.4).
    Arguments(Gc<ParameterMap>),
}

/// Heap cell for every JS object.
#[derive(Debug)]
pub struct JsObject {
    kind: ObjectKind,
    prototype: Option<ObjectRef>,
    extensible: bool,
    properties: NamedProperties,
    elements: Elements,
}

impl JsObject {
    fn new(kind: ObjectKind, prototype: Option<ObjectRef>) -> Self {
        Self {
            kind,
            prototype,
            extensible: true,
            properties: NamedProperties::default(),
            elements: Elements::default(),
        }
    }
}

impl Trace for JsObject {
    fn trace(&self, tracer: &mut Tracer) {
        self.prototype.trace(tracer);
        if let ObjectKind::Arguments(map) = self.kind {
            tracer.mark(map.raw());
        }
        self.properties.trace(tracer);
        self.elements.trace(tracer);
    }
}

impl Trace for Value {
    fn trace(&self, tracer: &mut Tracer) {
        if let Value::Object(object) = self {
            tracer.mark(*object);
        }
    }
}
