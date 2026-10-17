//! RegExp flags, pattern compilation through `regress` (with a cache per source and
//! flags), matching on UTF-16 text, EscapeRegExpPattern and `RegExp.escape`.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;

use regress::{Match, Regex};

use crate::JsStr;
use crate::convert::is_js_whitespace;

/// Compiled matchers kept before the cache starts over.
const CACHE_CAPACITY: usize = 512;

/// The `dgimsuvy` flags of a RegExp.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Flags {
    pub has_indices: bool,
    pub global: bool,
    pub ignore_case: bool,
    pub multiline: bool,
    pub dot_all: bool,
    pub unicode: bool,
    pub unicode_sets: bool,
    pub sticky: bool,
}

impl Flags {
    /// `None` for an unknown or repeated flag, or for `u` together with `v`.
    pub fn parse(units: &[u16]) -> Option<Self> {
        let mut flags = Self::default();
        for &unit in units {
            let flag = match char::from_u32(u32::from(unit))? {
                'd' => &mut flags.has_indices,
                'g' => &mut flags.global,
                'i' => &mut flags.ignore_case,
                'm' => &mut flags.multiline,
                's' => &mut flags.dot_all,
                'u' => &mut flags.unicode,
                'v' => &mut flags.unicode_sets,
                'y' => &mut flags.sticky,
                _ => return None,
            };
            if std::mem::replace(flag, true) {
                return None;
            }
        }
        (!(flags.unicode && flags.unicode_sets)).then_some(flags)
    }

    /// Whether the pattern and the text are read as code points (`u` or `v`).
    pub fn full_unicode(self) -> bool {
        self.unicode || self.unicode_sets
    }

    fn engine(self) -> regress::Flags {
        regress::Flags {
            icase: self.ignore_case,
            multiline: self.multiline,
            dot_all: self.dot_all,
            unicode: self.unicode,
            unicode_sets: self.unicode_sets,
            no_opt: false,
        }
    }
}

/// Parses `pattern` (code units) with `flags`; the error is the SyntaxError message.
pub(crate) fn compile(pattern: &[u16], flags: Flags) -> Result<Regex, String> {
    let code_points: Vec<u32> = if flags.full_unicode() {
        char::decode_utf16(pattern.iter().copied())
            .map(|unit| unit.map_or_else(|e| u32::from(e.unpaired_surrogate()), u32::from))
            .collect()
    } else {
        pattern.iter().map(|&unit| u32::from(unit)).collect()
    };
    Regex::from_unicode(code_points.into_iter(), flags.engine()).map_err(|error| {
        let source = String::from_utf16_lossy(pattern);
        format!("Invalid regular expression: /{source}/: {error}")
    })
}

/// Early error check of a regular expression literal (used by the bytecode compiler).
pub fn validate(pattern: &str, flags: &str) -> Result<(), String> {
    let flag_units: Vec<u16> = flags.encode_utf16().collect();
    let Some(parsed) = Flags::parse(&flag_units) else {
        return Err(format!("Invalid regular expression flags '{flags}'"));
    };
    let pattern: Vec<u16> = pattern.encode_utf16().collect();
    compile(&pattern, parsed).map(drop)
}

/// A compiled pattern; a sticky one only tries lastIndex (no forward search, so a split
/// that execs at every position stays linear).
pub(crate) struct Matcher {
    regex: Regex,
    sticky: bool,
}

impl Matcher {
    pub fn new(pattern: &[u16], flags: Flags) -> Result<Self, String> {
        Ok(Self {
            regex: compile(pattern, flags)?,
            sticky: flags.sticky,
        })
    }

    /// The first match at or after `start` (code units; surrogate pairs are one character
    /// only in `u`/`v` mode), only at `start` when sticky. `None` as soon as `interrupt`
    /// (the interpreter's termination flag) is raised, even mid-backtracking.
    pub fn find(
        &self,
        text: &[u16],
        start: usize,
        unicode: bool,
        interrupt: &AtomicBool,
    ) -> Option<Match> {
        self.regex
            .exec_interruptible(text, start, unicode, self.sticky, interrupt)
    }
}

/// Compiled matchers by (source, flags): every evaluation of a literal and every
/// `RegExp(...)` with a known pattern reuses the parse. Also keeps the code units of the
/// last subject string, so execs looping over one string (split, global replace) do not
/// copy it each time.
#[derive(Default)]
pub(crate) struct RegExpCache {
    matchers: HashMap<(Vec<u16>, Vec<u16>), Rc<Matcher>>,
    last_text: Option<(JsStr, Rc<[u16]>)>,
    pub(super) legacy: super::legacy::LegacyStatics,
}

impl RegExpCache {
    /// The code units of `text`; a hit is a pointer compare when it is the same string.
    /// shortcut: holds one subject copy per interpreter until the next exec on another
    /// string; drop it on GC if large inputs make that retention matter.
    pub fn utf16_of(&mut self, text: &JsStr) -> Rc<[u16]> {
        if let Some((cached, units)) = &self.last_text
            && cached == text
        {
            return units.clone();
        }
        let units: Rc<[u16]> = text.to_utf16().into();
        self.last_text = Some((text.clone(), units.clone()));
        units
    }

    pub fn get_or_compile(
        &mut self,
        pattern: &[u16],
        flag_units: &[u16],
        flags: Flags,
    ) -> Result<Rc<Matcher>, String> {
        let key = (pattern.to_vec(), flag_units.to_vec());
        if let Some(matcher) = self.matchers.get(&key) {
            return Ok(matcher.clone());
        }
        let matcher = Rc::new(Matcher::new(pattern, flags)?);
        if self.matchers.len() >= CACHE_CAPACITY {
            self.matchers.clear();
        }
        self.matchers.insert(key, matcher.clone());
        Ok(matcher)
    }
}

/// EscapeRegExpPattern: `/` and line terminators escaped so `/source/flags` re-parses.
pub(crate) fn escape_pattern(source: &[u16]) -> Vec<u16> {
    if source.is_empty() {
        return "(?:)".encode_utf16().collect();
    }
    let mut out = Vec::with_capacity(source.len());
    let mut in_class = false;
    let mut units = source.iter().copied();
    while let Some(unit) = units.next() {
        match unit {
            0x5C => {
                out.push(unit);
                match units.next() {
                    Some(next) if is_line_terminator(next) => push_name(&mut out, next),
                    Some(next) => out.push(next),
                    None => {}
                }
            }
            0x2F if !in_class => out.extend_from_slice(&[0x5C, 0x2F]),
            0x5B => {
                in_class = true;
                out.push(unit);
            }
            0x5D => {
                in_class = false;
                out.push(unit);
            }
            _ if is_line_terminator(unit) => {
                out.push(0x5C);
                push_name(&mut out, unit);
            }
            _ => out.push(unit),
        }
    }
    out
}

fn is_line_terminator(unit: u16) -> bool {
    matches!(unit, 0x0A | 0x0D | 0x2028 | 0x2029)
}

/// The escape letters of a line terminator, without the backslash.
fn push_name(out: &mut Vec<u16>, unit: u16) {
    let name = match unit {
        0x0A => "n",
        0x0D => "r",
        0x2028 => "u2028",
        _ => "u2029",
    };
    out.extend(name.encode_utf16());
}

/// `RegExp.escape(S)` (ES2026): a pattern that matches `S` literally.
pub(crate) fn regexp_escape(text: &[u16]) -> Vec<u16> {
    let mut out = Vec::new();
    for (index, c) in char::decode_utf16(text.iter().copied()).enumerate() {
        let code_point = c.map_or_else(|e| u32::from(e.unpaired_surrogate()), u32::from);
        let is_alphanumeric = char::from_u32(code_point).is_some_and(|c| c.is_ascii_alphanumeric());
        if index == 0 && is_alphanumeric {
            out.extend(format!("\\x{code_point:02x}").encode_utf16());
            continue;
        }
        encode_for_escape(&mut out, code_point);
    }
    out
}

/// EncodeForRegExpEscape.
fn encode_for_escape(out: &mut Vec<u16>, code_point: u32) {
    let c = char::from_u32(code_point);
    if c.is_some_and(|c| "^$\\.*+?()[]{}|/".contains(c)) {
        out.push(0x5C);
        out.push(code_point as u16);
        return;
    }
    let control = match code_point {
        0x09 => Some('t'),
        0x0A => Some('n'),
        0x0B => Some('v'),
        0x0C => Some('f'),
        0x0D => Some('r'),
        _ => None,
    };
    if let Some(letter) = control {
        out.extend_from_slice(&[0x5C, letter as u16]);
        return;
    }
    let is_surrogate = (0xD800..0xE000).contains(&code_point);
    let needs_escape = is_surrogate
        || c.is_some_and(|c| ",-=<>#&!%:;@~'`\"".contains(c) || is_js_whitespace(c))
        || matches!(code_point, 0x2028 | 0x2029);
    if !needs_escape {
        push_code_point(out, code_point);
    } else if code_point <= 0xFF {
        out.extend(format!("\\x{code_point:02x}").encode_utf16());
    } else {
        let mut units = Vec::new();
        push_code_point(&mut units, code_point);
        for unit in units {
            out.extend(format!("\\u{unit:04x}").encode_utf16());
        }
    }
}

fn push_code_point(out: &mut Vec<u16>, code_point: u32) {
    match char::from_u32(code_point) {
        Some(c) => out.extend_from_slice(c.encode_utf16(&mut [0; 2])),
        None => out.push(code_point as u16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_reuses_the_compiled_matcher_per_source_and_flags() {
        let mut cache = RegExpCache::default();
        let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        let flags = Flags::parse(&units("g")).unwrap();
        let first = cache
            .get_or_compile(&units("a+"), &units("g"), flags)
            .unwrap();
        let again = cache
            .get_or_compile(&units("a+"), &units("g"), flags)
            .unwrap();
        let other = cache.get_or_compile(&units("a+"), &units(""), Flags::default());
        assert!(Rc::ptr_eq(&first, &again));
        assert!(!Rc::ptr_eq(&first, &other.unwrap()));
    }

    #[test]
    fn escape_pattern_escapes_slashes_outside_classes_and_line_terminators() {
        let escaped =
            |s: &str| String::from_utf16(&escape_pattern(&s.encode_utf16().collect::<Vec<_>>()));
        assert_eq!(escaped("a/b[/]\n\\\n").unwrap(), "a\\/b[/]\\n\\n");
        assert_eq!(escaped("").unwrap(), "(?:)");
    }
}
