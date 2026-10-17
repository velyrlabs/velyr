//! Unit patterns for `Intl.NumberFormat` `style:'unit'`, read from `unit_data.tsv`.
//! shortcut: ICU4X keeps unit data in `icu_experimental::dimension`, whose formatters take
//! over the digits and cannot yield parts or compound units, so a small table covers the
//! sanctioned units for `en` and `pt`, the `kilometer-per-hour` compound for `de`, `ja`,
//! `ko` and `zh` (Traditional), and every other language uses the `en` rows. Replace with
//! CLDR unit data when a pattern-level API exists.

const DATA: &str = include_str!("unit_data.tsv");

#[derive(Clone, Copy)]
pub(super) enum Width {
    Short,
    Narrow,
    Long,
}

impl Width {
    pub(super) fn of(name: &str) -> Self {
        match name {
            "narrow" => Self::Narrow,
            "long" => Self::Long,
            _ => Self::Short,
        }
    }
}

/// Columns: `lang unit short_one short_other narrow long_one long_other`; `=` repeats the
/// previous column.
fn find(lang: &str, unit: &str, width: Width, one: bool) -> Option<&'static str> {
    let row: Vec<&str> = DATA
        .lines()
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .find(|fields| fields[0] == lang && fields[1] == unit)?;
    let (singular, plural) = match width {
        Width::Short => (row[2], row[3]),
        Width::Narrow => (row[4], row[4]),
        Width::Long => (row[5], row[6]),
    };
    Some(if one || plural == "=" {
        singular
    } else {
        plural
    })
}

/// The row of `unit` in the first language of `chain` that has one.
fn find_in(chain: &[&str], unit: &str, width: Width, one: bool) -> Option<&'static str> {
    chain.iter().find_map(|lang| find(lang, unit, width, one))
}

/// The unit's text without its number placeholder (`{0} hour` -> `hour`).
fn name_of(pattern: &str) -> String {
    pattern.replace("{0}", "").trim().to_owned()
}

fn compose(chain: &[&str], unit: &str, width: Width, one: bool) -> Option<String> {
    let (numerator, denominator) = unit.split_once("-per-")?;
    let per = find_in(chain, "per", width, true)?;
    let top = find_in(chain, numerator, width, one)?;
    let bottom = name_of(find_in(chain, denominator, width, true)?);
    Some(per.replace("{0}", top).replace("{1}", &bottom))
}

/// The pattern (with a `{0}` placeholder) for `unit` in `lang`, or `None` for an unknown unit.
/// A language's own row wins, then its own compound rule, then the same two for `en`.
pub(super) fn pattern(lang: &str, unit: &str, width: Width, one: bool) -> Option<String> {
    let chain: &[&str] = if lang == "en" { &["en"] } else { &[lang, "en"] };
    chain.iter().enumerate().find_map(|(index, lang)| {
        find(lang, unit, width, one).map(str::to_owned).or_else(|| {
            find(lang, "per", width, true)?;
            compose(&chain[index..], unit, width, one)
        })
    })
}

/// `(kind, text)` entries of `affix`: whitespace is a literal, the rest the unit text.
pub(super) fn affix_entries(text: &str) -> Vec<(&'static str, String)> {
    let core = text.trim();
    let Some(start) = text.find(core).filter(|_| !core.is_empty()) else {
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![("literal", text.to_owned())]
        };
    };
    [
        ("literal", &text[..start]),
        ("unit", core),
        ("literal", &text[start + core.len()..]),
    ]
    .into_iter()
    .filter(|(_, text)| !text.is_empty())
    .map(|(kind, text)| (kind, text.to_owned()))
    .collect()
}
