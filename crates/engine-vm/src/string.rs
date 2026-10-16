//! `JsStr`: immutable ECMAScript string with latin1/utf16 storage, lazy ropes and a cached hash.

use std::cell::{Cell, RefCell};
use std::hash::{Hash, Hasher};
use std::mem;
use std::rc::Rc;

use velyr_engine_api::JsString;

/// Concatenations shorter than this are copied instead of building a rope node.
const MIN_ROPE_LEN: usize = 16;

enum Node {
    Latin1(Box<[u8]>),
    Utf16(Box<[u16]>),
    Rope(JsStr, JsStr),
}

struct Repr {
    len: usize,
    /// Every unit is <= 0xFF, so flattening a rope yields latin1.
    latin1: bool,
    hash: Cell<Option<u32>>,
    node: RefCell<Node>,
}

/// Flat view of a string's code units.
pub enum Flat<'a> {
    Latin1(&'a [u8]),
    Utf16(&'a [u16]),
}

#[derive(Clone)]
pub struct JsStr(Rc<Repr>);

impl JsStr {
    fn from_node(len: usize, latin1: bool, node: Node) -> Self {
        Self(Rc::new(Repr {
            len,
            latin1,
            hash: Cell::new(None),
            node: RefCell::new(node),
        }))
    }

    #[allow(clippy::should_implement_trait)] // infallible, same as `JsString::from_str`
    pub fn from_str(s: &str) -> Self {
        Self::from_utf16(s.encode_utf16().collect())
    }

    /// Narrows to latin1 when every unit fits.
    pub fn from_utf16(units: Vec<u16>) -> Self {
        let len = units.len();
        if units.iter().all(|u| *u <= 0xFF) {
            let bytes = units.iter().map(|u| *u as u8).collect();
            return Self::from_node(len, true, Node::Latin1(bytes));
        }
        Self::from_node(len, false, Node::Utf16(units.into()))
    }

    pub fn len(&self) -> usize {
        self.0.len
    }

    pub fn is_empty(&self) -> bool {
        self.0.len == 0
    }

    /// Concatenation; builds a rope node once the result is long enough.
    pub fn concat(&self, other: &JsStr) -> JsStr {
        let len = self.len() + other.len();
        let latin1 = self.0.latin1 && other.0.latin1;
        if len < MIN_ROPE_LEN {
            let mut units = self.to_utf16();
            units.extend(other.to_utf16());
            return Self::from_utf16(units);
        }
        Self::from_node(len, latin1, Node::Rope(self.clone(), other.clone()))
    }

    pub fn is_rope(&self) -> bool {
        matches!(*self.0.node.borrow(), Node::Rope(..))
    }

    /// True when the (flattened) storage is latin1.
    pub fn is_latin1(&self) -> bool {
        self.0.latin1
    }

    pub fn with_flat<R>(&self, f: impl FnOnce(Flat<'_>) -> R) -> R {
        self.flatten();
        match &*self.0.node.borrow() {
            Node::Latin1(bytes) => f(Flat::Latin1(bytes)),
            Node::Utf16(units) => f(Flat::Utf16(units)),
            Node::Rope(..) => unreachable!("flatten removes the rope"),
        }
    }

    pub fn to_utf16(&self) -> Vec<u16> {
        self.with_flat(|flat| match flat {
            Flat::Latin1(bytes) => bytes.iter().map(|b| u16::from(*b)).collect(),
            Flat::Utf16(units) => units.to_vec(),
        })
    }

    /// Lone surrogates become U+FFFD.
    pub fn to_string_lossy(&self) -> String {
        String::from_utf16_lossy(&self.to_utf16())
    }

    pub fn to_js_string(&self) -> JsString {
        JsString::from_utf16(self.to_utf16())
    }

    /// Iterative, so ropes built by 100k+ appends cannot overflow the stack.
    fn flatten(&self) {
        if !self.is_rope() {
            return;
        }
        let mut pending = vec![self.clone()];
        let flat = if self.0.latin1 {
            let mut out = Vec::with_capacity(self.len());
            collect(&mut pending, |flat| match flat {
                Flat::Latin1(bytes) => out.extend_from_slice(bytes),
                Flat::Utf16(_) => unreachable!("latin1 rope has a utf16 leaf"),
            });
            Node::Latin1(out.into())
        } else {
            let mut out = Vec::with_capacity(self.len());
            collect(&mut pending, |flat| match flat {
                Flat::Latin1(bytes) => out.extend(bytes.iter().map(|b| u16::from(*b))),
                Flat::Utf16(units) => out.extend_from_slice(units),
            });
            Node::Utf16(out.into())
        };
        // Dropping the old rope node releases the children (see `Drop for Repr`).
        *self.0.node.borrow_mut() = flat;
    }

    /// FNV-1a over UTF-16 units, so latin1 and utf16 storage of equal text hash alike.
    pub fn hash_value(&self) -> u32 {
        if let Some(hash) = self.0.hash.get() {
            return hash;
        }
        let fnv = |hash: u32, unit: u16| (hash ^ u32::from(unit)).wrapping_mul(16_777_619);
        let hash = self.with_flat(|flat| match flat {
            Flat::Latin1(bytes) => bytes
                .iter()
                .fold(2_166_136_261, |h, b| fnv(h, u16::from(*b))),
            Flat::Utf16(units) => units.iter().fold(2_166_136_261, |h, u| fnv(h, *u)),
        });
        self.0.hash.set(Some(hash));
        hash
    }
}

/// Appends the leaves of every pending rope, left to right, to the caller's buffer.
fn collect(pending: &mut Vec<JsStr>, mut push: impl FnMut(Flat<'_>)) {
    while let Some(s) = pending.pop() {
        match &*s.0.node.borrow() {
            Node::Rope(left, right) => {
                pending.push(right.clone());
                pending.push(left.clone());
            }
            Node::Latin1(bytes) => push(Flat::Latin1(bytes)),
            Node::Utf16(units) => push(Flat::Utf16(units)),
        }
    }
}

impl Drop for Repr {
    /// Unlinks rope children iteratively: the default recursive drop of a left-deep rope
    /// of 100k nodes would overflow the stack.
    fn drop(&mut self) {
        let mut stack = Vec::new();
        take_children(self.node.get_mut(), &mut stack);
        while let Some(child) = stack.pop() {
            if let Ok(mut repr) = Rc::try_unwrap(child.0) {
                take_children(repr.node.get_mut(), &mut stack);
            }
        }
    }
}

fn take_children(node: &mut Node, stack: &mut Vec<JsStr>) {
    if let Node::Rope(left, right) = mem::replace(node, Node::Latin1(Box::new([]))) {
        stack.push(left);
        stack.push(right);
    }
}

impl PartialEq for JsStr {
    fn eq(&self, other: &Self) -> bool {
        if Rc::ptr_eq(&self.0, &other.0) {
            return true;
        }
        if self.len() != other.len() || self.hash_value() != other.hash_value() {
            return false;
        }
        self.to_utf16() == other.to_utf16()
    }
}

impl Eq for JsStr {}

impl Hash for JsStr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(self.hash_value());
    }
}

impl std::fmt::Debug for JsStr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.to_string_lossy())
    }
}

impl From<&str> for JsStr {
    fn from(s: &str) -> Self {
        Self::from_str(s)
    }
}

impl From<&JsString> for JsStr {
    fn from(s: &JsString) -> Self {
        Self::from_utf16(s.as_utf16().to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_and_latin1_text_uses_latin1_storage() {
        assert!(JsStr::from_str("abc").is_latin1());
        assert!(JsStr::from_str("caf\u{e9}").is_latin1());
        assert!(!JsStr::from_str("\u{20ac}").is_latin1());
    }

    #[test]
    fn utf16_keeps_lone_surrogates() {
        let s = JsStr::from_utf16(vec![0xD800, 0x61]);
        assert_eq!(s.len(), 2);
        assert_eq!(s.to_utf16(), vec![0xD800, 0x61]);
        assert_eq!(s.to_string_lossy(), "\u{fffd}a");
    }

    #[test]
    fn equal_text_is_equal_and_hashes_alike_across_representations() {
        let latin1 = JsStr::from_str("abc");
        let narrowed = JsStr::from_utf16(vec![0x61, 0x62, 0x63]);
        assert_eq!(latin1, narrowed);
        assert_eq!(latin1.hash_value(), narrowed.hash_value());
        assert_ne!(latin1, JsStr::from_str("abd"));
        assert_ne!(JsStr::from_str("a"), JsStr::from_str("\u{20ac}"));
    }

    #[test]
    fn short_concat_is_flat_and_long_concat_is_a_rope() {
        let short = JsStr::from_str("ab").concat(&JsStr::from_str("cd"));
        assert!(!short.is_rope());
        let long = JsStr::from_str("0123456789").concat(&JsStr::from_str("abcdefghij"));
        assert!(long.is_rope());
        assert_eq!(long.len(), 20);
        assert_eq!(long.to_string_lossy(), "0123456789abcdefghij");
        assert!(!long.is_rope(), "reading flattens the rope in place");
    }

    #[test]
    fn rope_mixing_latin1_and_utf16_flattens_to_utf16() {
        let s = JsStr::from_str("0123456789").concat(&JsStr::from_str("\u{20ac}abcdefghij"));
        assert!(!s.is_latin1());
        assert_eq!(s.to_string_lossy(), "0123456789\u{20ac}abcdefghij");
    }

    #[test]
    fn deep_ropes_flatten_and_drop_without_overflowing_the_stack() {
        let piece = JsStr::from_str("ab");
        let mut left_deep = JsStr::from_str("");
        let mut right_deep = JsStr::from_str("");
        for _ in 0..200_000 {
            left_deep = left_deep.concat(&piece);
            right_deep = piece.concat(&right_deep);
        }
        assert_eq!(left_deep.len(), 400_000);
        assert_eq!(left_deep, right_deep);
        let never_flattened = JsStr::from_str("0123456789012345").concat(&left_deep);
        drop(never_flattened);
    }

    #[test]
    fn converts_to_and_from_the_api_string() {
        let api = JsString::from_utf16(vec![0x20ac, 0x61]);
        assert_eq!(JsStr::from(&api).to_js_string(), api);
    }
}
