//! Engine-agnostic JS values. No type here may borrow from an engine scope.

/// Isolate-scoped reference to an engine-owned object.
///
/// A handle is only meaningful for the isolate that produced it and must be
/// returned with [`Isolate::release`](crate::Isolate::release) (or by leaving a
/// [`scope`](crate::Isolate::scope)). The `generation` detects stale handles: an
/// adapter must reject a handle whose generation no longer matches its slot
/// instead of aliasing a newer object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle {
    pub id: u32,
    pub generation: u32,
}

/// A JavaScript value as seen by the runtime. Primitives are owned by value;
/// objects (functions, arrays, promises, buffers, ...) are opaque [`Handle`]s.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(JsString),
    BigInt(JsBigInt),
    Object(Handle),
}

/// An ECMAScript string: a sequence of UTF-16 code units, lone surrogates included.
///
/// Rust `String` cannot represent every JS string, so conversion to UTF-8 is
/// explicit and lossy ([`JsString::to_string_lossy`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct JsString(Vec<u16>);

impl JsString {
    pub fn from_utf16(units: Vec<u16>) -> Self {
        Self(units)
    }

    #[allow(clippy::should_implement_trait)] // infallible, so `FromStr` would only add noise
    pub fn from_str(s: &str) -> Self {
        Self(s.encode_utf16().collect())
    }

    /// Lone surrogates become U+FFFD.
    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(&self.0)
    }

    pub fn as_utf16(&self) -> &[u16] {
        &self.0
    }

    /// Length in UTF-16 code units, as `String.prototype.length`.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<&str> for JsString {
    fn from(s: &str) -> Self {
        Self::from_str(s)
    }
}

/// An arbitrary-precision integer: sign plus little-endian magnitude bytes.
///
/// Always normalized (no trailing zero bytes, zero is never negative), so
/// `PartialEq` is value equality.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct JsBigInt {
    negative: bool,
    magnitude: Vec<u8>,
}

impl JsBigInt {
    pub fn new(negative: bool, mut magnitude: Vec<u8>) -> Self {
        while magnitude.last() == Some(&0) {
            magnitude.pop();
        }
        let negative = negative && !magnitude.is_empty();
        Self {
            negative,
            magnitude,
        }
    }

    pub fn from_i64(value: i64) -> Self {
        Self::new(value < 0, value.unsigned_abs().to_le_bytes().to_vec())
    }

    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// Little-endian magnitude bytes; empty for zero.
    pub fn magnitude_le(&self) -> &[u8] {
        &self.magnitude
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_string_preserves_lone_surrogate() {
        let units = vec![0x61, 0xD800, 0x62];
        let s = JsString::from_utf16(units.clone());
        assert_eq!(s.as_utf16(), units.as_slice());
        assert_eq!(s.to_string_lossy(), "a\u{FFFD}b");
        assert_eq!(s.clone(), s);
    }

    #[test]
    fn js_string_from_str_roundtrips_astral() {
        let s = JsString::from_str("h\u{1F600}");
        assert_eq!(s.as_utf16().len(), 3);
        assert_eq!(s.to_string_lossy(), "h\u{1F600}");
        assert!(!s.is_empty());
    }

    #[test]
    fn js_bigint_normalizes() {
        let zero = JsBigInt::new(true, vec![0, 0]);
        assert_eq!(zero, JsBigInt::from_i64(0));
        assert!(!zero.is_negative());
        assert!(zero.magnitude_le().is_empty());
        let n = JsBigInt::from_i64(-258);
        assert!(n.is_negative());
        assert_eq!(n.magnitude_le(), &[2, 1]);
        assert_eq!(
            JsBigInt::from_i64(i64::MIN).magnitude_le(),
            &[0, 0, 0, 0, 0, 0, 0, 0x80]
        );
    }
}
