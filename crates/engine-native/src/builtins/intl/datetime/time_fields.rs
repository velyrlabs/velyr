//! Time-field handling the ICU4X field sets lack: the `fractionalSecond` part, time
//! patterns without an hour, `dayPeriod` names and single-field numeric widths.

use super::Spec;

use super::PartList;

/// Code points of the digit zero of every numbering system the prelude resolves.
const ZERO_DIGITS: [char; 30] = [
    '0',
    '\u{660}',
    '\u{6F0}',
    '\u{7C0}',
    '\u{966}',
    '\u{9E6}',
    '\u{A66}',
    '\u{AE6}',
    '\u{B66}',
    '\u{BE6}',
    '\u{C66}',
    '\u{CE6}',
    '\u{D66}',
    '\u{E50}',
    '\u{ED0}',
    '\u{F20}',
    '\u{1040}',
    '\u{17E0}',
    '\u{1810}',
    '\u{1946}',
    '\u{1C50}',
    '\u{A620}',
    '\u{A9D0}',
    '\u{ABF0}',
    '\u{FF10}',
    '\u{3007}',
    '\u{1E950}',
    '\u{1E140}',
    '\u{11136}',
    '\u{1D7CE}',
];

const HANIDEC: &str =
    "\u{3007}\u{4E00}\u{4E8C}\u{4E09}\u{56DB}\u{4E94}\u{516D}\u{4E03}\u{516B}\u{4E5D}";

/// The zero digit of the numbering system `digit` belongs to.
pub(super) fn zero_of(digit: char) -> Option<char> {
    if HANIDEC.contains(digit) {
        return Some('\u{3007}');
    }
    ZERO_DIGITS
        .iter()
        .find(|zero| (digit as u32).wrapping_sub(**zero as u32) < 10)
        .copied()
}

/// ICU4X writes `34.567` as one `second` part: split off the fraction. Its pattern keeps
/// the Latin decimal point for the Arabic numbering systems, whose own is `\u{66B}`.
pub(super) fn split_fraction(parts: &mut PartList, arabic_digits: bool) {
    let mut out = PartList::with_capacity(parts.len() + 2);
    for (kind, text) in parts.drain(..) {
        let cut = text.char_indices().find(|(_, c)| !c.is_numeric());
        match cut {
            Some((at, separator)) if kind == "second" => {
                out.push((kind, text[..at].to_owned()));
                let shown = if arabic_digits && separator == '.' {
                    '\u{66B}'
                } else {
                    separator
                };
                out.push(("literal", shown.to_string()));
                out.push((
                    "fractionalSecond",
                    text[at + separator.len_utf8()..].to_owned(),
                ));
            }
            _ => out.push((kind, text)),
        }
    }
    *parts = out;
}

fn requested(spec: &Spec, kind: &str) -> bool {
    match kind {
        "hour" | "minute" | "second" => spec.get(kind).is_some(),
        "fractionalSecond" => spec.get("fractionalSecondDigits").is_some(),
        "dayPeriod" => spec.get("dayPeriod").is_some() || spec.get("hour").is_some(),
        _ => true,
    }
}

/// A time pattern without an hour (the only kind `keep_requested` rewrites).
pub(super) fn is_hourless(spec: &Spec) -> bool {
    spec.get("hour").is_none()
        && spec.get("dateStyle").is_none()
        && spec.get("timeStyle").is_none()
        && ["minute", "second", "fractionalSecondDigits", "dayPeriod"]
            .iter()
            .any(|key| spec.get(key).is_some())
}

/// The patterns are built for an hour; without one, drops the fields that were not asked
/// for and the literals that no longer sit between two kept fields.
pub(super) fn keep_requested(spec: &Spec, parts: &mut PartList) {
    let kept = |kind: &str| kind != "literal" && requested(spec, kind);
    let first = parts.iter().position(|(kind, _)| kept(kind));
    let last = parts.iter().rposition(|(kind, _)| kept(kind));
    let (Some(first), Some(last)) = (first, last) else {
        return;
    };
    let mut index = 0;
    parts.retain(|(kind, _)| {
        index += 1;
        let at = index - 1;
        if *kind == "literal" {
            first < at && at < last
        } else {
            requested(spec, kind)
        }
    });
}

/// `numeric` on a lone minute or second is not padded (ICU4X always pads to two digits).
pub(super) fn unpad_single(spec: &Spec, parts: &mut PartList) {
    let mut fields = parts
        .iter()
        .filter(|(kind, _)| !matches!(*kind, "literal" | "fractionalSecond"));
    let (Some(&(kind, _)), None) = (fields.next(), fields.next()) else {
        return;
    };
    if !matches!(kind, "minute" | "second") || spec.get(kind) != Some("numeric") {
        return;
    }
    for (_, text) in parts.iter_mut().filter(|(k, _)| *k == kind) {
        let mut chars = text.chars();
        if chars.next().is_some_and(|c| zero_of(c) == Some(c)) && chars.count() == 1 {
            text.remove(0);
        }
    }
}

/// shortcut: English flexible day periods only (CLDR `B`); other locales keep AM/PM.
/// The no-break space CLDR puts before AM/PM becomes a plain space.
pub(super) fn flexible_day_period(parts: &mut PartList, hour: u8, minute: u8, narrow: bool) {
    let name = match hour {
        12 if minute == 0 && narrow => "n",
        12 if minute == 0 => "noon",
        6..=11 => "in the morning",
        12..=17 => "in the afternoon",
        18..=20 => "in the evening",
        _ => "at night",
    };
    for index in 0..parts.len() {
        if parts[index].0 != "dayPeriod" {
            continue;
        }
        parts[index].1 = name.to_owned();
        if let Some(before) = index.checked_sub(1).and_then(|at| parts.get_mut(at))
            && before.0 == "literal"
        {
            before.1 = " ".to_owned();
        }
    }
}

/// `hourCycle: 'h24'` writes midnight as 24; ICU4X has no such cycle and writes 0 (or 00).
pub(super) fn midnight_as_24(parts: &mut PartList) {
    for (_, text) in parts.iter_mut().filter(|(kind, _)| *kind == "hour") {
        if text.bytes().all(|b| b == b'0') {
            *text = "24".to_owned();
        }
    }
}

/// A zero offset is just `GMT` in CLDR; ICU4X writes `GMT+0` for zones given as an offset.
pub(super) fn zero_offset_as_gmt(parts: &mut PartList) {
    for (_, text) in parts.iter_mut().filter(|(kind, _)| *kind == "timeZoneName") {
        if matches!(text.as_str(), "GMT+0" | "GMT-0" | "GMT+00:00" | "GMT-00:00") {
            *text = "GMT".to_owned();
        }
    }
}
