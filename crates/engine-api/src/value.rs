//! Engine-agnostic JS values. No type here may borrow from an engine scope.

use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

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
/// explicit and lossy ([`JsString::to_string_lossy`]). Internally a string is either
/// UTF-16 or Latin-1 (one byte per unit, matching V8's one-byte strings); equality
/// and hashing look only at the code units, never at the representation.
#[derive(Debug, Clone, Default)]
pub struct JsString(Repr);

#[derive(Debug, Clone)]
enum Repr {
    Latin1 {
        bytes: Vec<u8>,
        /// Lazily widened copy so `as_utf16` can keep returning a borrowed slice.
        widened: OnceLock<Vec<u16>>,
    },
    Utf16(Vec<u16>),
}

impl Default for Repr {
    fn default() -> Self {
        Self::Utf16(Vec::new())
    }
}

impl JsString {
    pub fn from_utf16(units: Vec<u16>) -> Self {
        Self(Repr::Utf16(units))
    }

    /// Each byte is one code unit (U+0000..=U+00FF).
    pub fn from_latin1(bytes: Vec<u8>) -> Self {
        Self(Repr::Latin1 {
            bytes,
            widened: OnceLock::new(),
        })
    }

    #[allow(clippy::should_implement_trait)] // infallible, so `FromStr` would only add noise
    pub fn from_str(s: &str) -> Self {
        Self(Repr::Utf16(s.encode_utf16().collect()))
    }

    /// Lone surrogates become U+FFFD.
    pub fn to_string_lossy(&self) -> String {
        match &self.0 {
            Repr::Latin1 { bytes, .. } => bytes.iter().map(|&b| char::from(b)).collect(),
            Repr::Utf16(units) => String::from_utf16_lossy(units),
        }
    }

    /// Code units as UTF-16. A Latin-1 string is widened once and cached.
    pub fn as_utf16(&self) -> &[u16] {
        match &self.0 {
            Repr::Latin1 { bytes, widened } => {
                widened.get_or_init(|| bytes.iter().map(|&b| u16::from(b)).collect())
            }
            Repr::Utf16(units) => units,
        }
    }

    /// The raw bytes when the string is stored one byte per unit.
    pub fn as_latin1(&self) -> Option<&[u8]> {
        match &self.0 {
            Repr::Latin1 { bytes, .. } => Some(bytes),
            Repr::Utf16(_) => None,
        }
    }

    /// Length in UTF-16 code units, as `String.prototype.length`.
    pub fn len(&self) -> usize {
        match &self.0 {
            Repr::Latin1 { bytes, .. } => bytes.len(),
            Repr::Utf16(units) => units.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn units(&self) -> impl Iterator<Item = u16> + '_ {
        let (bytes, units) = match &self.0 {
            Repr::Latin1 { bytes, .. } => (bytes.as_slice(), &[][..]),
            Repr::Utf16(units) => (&[][..], units.as_slice()),
        };
        bytes
            .iter()
            .map(|&b| u16::from(b))
            .chain(units.iter().copied())
    }
}

impl PartialEq for JsString {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Repr::Latin1 { bytes: a, .. }, Repr::Latin1 { bytes: b, .. }) => a == b,
            (Repr::Utf16(a), Repr::Utf16(b)) => a == b,
            _ => self.len() == other.len() && self.units().eq(other.units()),
        }
    }
}

impl Eq for JsString {}

impl Hash for JsString {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.len());
        for unit in self.units() {
            state.write_u16(unit);
        }
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
    fn latin1_roundtrips_and_widens() {
        use std::collections::hash_map::DefaultHasher;
        let latin1 = JsString::from_latin1(b"caf\xe9".to_vec());
        let utf16 = JsString::from_utf16(vec![0x63, 0x61, 0x66, 0xe9]);
        assert_eq!(latin1, utf16);
        assert_eq!(latin1.as_latin1(), Some(&b"caf\xe9"[..]));
        assert_eq!(utf16.as_latin1(), None);
        assert_eq!(latin1.as_utf16(), utf16.as_utf16());
        assert_eq!(latin1.to_string_lossy(), "caf\u{e9}");
        assert_eq!(latin1.len(), 4);
        let hash = |s: &JsString| {
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        };
        assert_eq!(hash(&latin1), hash(&utf16));
        assert_ne!(latin1, JsString::from_utf16(vec![0x63, 0x61, 0x66, 0x100]));
        assert_eq!(JsString::from_latin1(Vec::new()), JsString::default());
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
