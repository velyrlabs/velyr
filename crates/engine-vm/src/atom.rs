//! Interned property names: equal names intern to the same `Atom`, so key equality is an integer compare.

use std::collections::HashMap;

use velyr_engine_gc::external::{Charge, Kind};

use crate::string::JsStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Atom(u32);

/// Atoms are only meaningful for the interner that produced them.
#[derive(Debug)]
pub struct Interner {
    atoms: HashMap<JsStr, Atom>,
    names: Vec<JsStr>,
    /// Both tables by capacity, as `Kind::Interner` (the names are charged as strings).
    charge: Charge,
}

impl Default for Interner {
    fn default() -> Self {
        Self {
            atoms: HashMap::new(),
            names: Vec::new(),
            charge: Charge::new(Kind::Interner, 0),
        }
    }
}

impl Interner {
    pub fn intern(&mut self, name: &JsStr) -> Atom {
        if let Some(atom) = self.atoms.get(name) {
            return *atom;
        }
        let atom = Atom(u32::try_from(self.names.len()).expect("atom table overflow"));
        let capacities = (self.names.capacity(), self.atoms.capacity());
        self.names.push(name.clone());
        self.atoms.insert(name.clone(), atom);
        if capacities != (self.names.capacity(), self.atoms.capacity()) {
            self.charge.set(
                self.names.capacity() * size_of::<JsStr>()
                    + self.atoms.capacity() * (size_of::<(JsStr, Atom)>() + 1),
            );
        }
        atom
    }

    pub fn intern_str(&mut self, name: &str) -> Atom {
        self.intern(&JsStr::from_str(name))
    }

    pub fn resolve(&self, atom: Atom) -> &JsStr {
        &self.names[atom.0 as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_names_share_an_atom_across_representations_and_ropes() {
        let mut interner = Interner::default();
        let a = interner.intern_str("length");
        let from_utf16 = JsStr::from_utf16("length".encode_utf16().collect());
        let rope = JsStr::from_str("leng").concat(&JsStr::from_str("th"));
        assert_eq!(interner.intern(&from_utf16), a);
        assert_eq!(interner.intern(&rope), a);
        assert_ne!(interner.intern_str("size"), a);
    }

    #[test]
    fn resolves_back_to_the_name() {
        let mut interner = Interner::default();
        let atom = interner.intern_str("\u{20ac}");
        assert_eq!(interner.resolve(atom), &JsStr::from_str("\u{20ac}"));
    }
}
