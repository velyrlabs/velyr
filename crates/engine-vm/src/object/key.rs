//! Object property keys, pre-classified so array indices never go through the interner.

use crate::{Atom, Flat, JsStr, SymbolId};

/// Largest array index is 2^32 - 2; "4294967295" is an ordinary string key.
pub const MAX_ARRAY_INDEX: u32 = u32::MAX - 1;

/// Property key as stored on objects. `Name` never holds a canonical array index:
/// build keys with `Realm::key` / `Realm::key_str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectKey {
    Index(u32),
    Name(Atom),
    Symbol(SymbolId),
}

/// Parses a canonical array index ("0", or no leading zero, below 2^32 - 1).
pub(crate) fn parse_array_index(name: &JsStr) -> Option<u32> {
    let len = name.len();
    if len == 0 || len > 10 {
        return None;
    }
    let digits: Vec<u16> = name.with_flat(|flat| match flat {
        Flat::Latin1(units) => units.iter().map(|u| u16::from(*u)).collect(),
        Flat::Utf16(units) => units.to_vec(),
    });
    if len > 1 && digits[0] == u16::from(b'0') {
        return None;
    }
    let mut value: u64 = 0;
    for unit in digits {
        let digit = char::from_u32(u32::from(unit))?.to_digit(10)?;
        value = value * 10 + u64::from(digit);
    }
    u32::try_from(value)
        .ok()
        .filter(|index| *index <= MAX_ARRAY_INDEX)
}
