//! `en`/`pt` relative time patterns for builds without `intl-experimental`.
// shortcut: hand-written CLDR subset (no pt-PT variants, narrow = short for pt); upgrade by
// enabling intl-experimental, which reads the full CLDR data.

use fixed_decimal::{Decimal, Sign};
use icu_plurals::{PluralCategory, PluralRules};

use super::super::locale;
use super::{RelativeParts, literal, number_parts};

const UNITS: [&str; 8] = [
    "second", "minute", "hour", "day", "week", "month", "quarter", "year",
];

/// Singular and plural unit word (with its leading space), then the phrases for -1, 0, 1.
type Row = (&'static str, &'static str, [Option<&'static str>; 3]);

const fn row(one: &'static str, other: &'static str, auto: [Option<&'static str>; 3]) -> Row {
    (one, other, auto)
}

const NONE: Option<&str> = None;

const EN_LONG: [Row; 8] = [
    row(" second", " seconds", [NONE, Some("now"), NONE]),
    row(" minute", " minutes", [NONE, Some("this minute"), NONE]),
    row(" hour", " hours", [NONE, Some("this hour"), NONE]),
    row(
        " day",
        " days",
        [Some("yesterday"), Some("today"), Some("tomorrow")],
    ),
    row(
        " week",
        " weeks",
        [Some("last week"), Some("this week"), Some("next week")],
    ),
    row(
        " month",
        " months",
        [Some("last month"), Some("this month"), Some("next month")],
    ),
    row(
        " quarter",
        " quarters",
        [
            Some("last quarter"),
            Some("this quarter"),
            Some("next quarter"),
        ],
    ),
    row(
        " year",
        " years",
        [Some("last year"), Some("this year"), Some("next year")],
    ),
];

const EN_SHORT: [Row; 8] = [
    row(" sec.", " sec.", [NONE, Some("now"), NONE]),
    row(" min.", " min.", [NONE, Some("this minute"), NONE]),
    row(" hr.", " hr.", [NONE, Some("this hour"), NONE]),
    row(
        " day",
        " days",
        [Some("yesterday"), Some("today"), Some("tomorrow")],
    ),
    row(
        " wk.",
        " wk.",
        [Some("last wk."), Some("this wk."), Some("next wk.")],
    ),
    row(
        " mo.",
        " mo.",
        [Some("last mo."), Some("this mo."), Some("next mo.")],
    ),
    row(
        " qtr.",
        " qtrs.",
        [Some("last qtr."), Some("this qtr."), Some("next qtr.")],
    ),
    row(
        " yr.",
        " yr.",
        [Some("last yr."), Some("this yr."), Some("next yr.")],
    ),
];

const EN_NARROW: [Row; 8] = [
    row("s", "s", [NONE, Some("now"), NONE]),
    row("min", "min", [NONE, Some("this minute"), NONE]),
    row("h", "h", [NONE, Some("this hour"), NONE]),
    row(
        "d",
        "d",
        [Some("yesterday"), Some("today"), Some("tomorrow")],
    ),
    row(
        "w",
        "w",
        [Some("last wk."), Some("this wk."), Some("next wk.")],
    ),
    row(
        "mo",
        "mo",
        [Some("last mo."), Some("this mo."), Some("next mo.")],
    ),
    row(
        "q",
        "q",
        [Some("last qtr."), Some("this qtr."), Some("next qtr.")],
    ),
    row(
        "y",
        "y",
        [Some("last yr."), Some("this yr."), Some("next yr.")],
    ),
];

const PT_LONG: [Row; 8] = [
    row(" segundo", " segundos", [NONE, Some("agora"), NONE]),
    row(" minuto", " minutos", [NONE, Some("este minuto"), NONE]),
    row(" hora", " horas", [NONE, Some("esta hora"), NONE]),
    row(
        " dia",
        " dias",
        [Some("ontem"), Some("hoje"), Some("amanhã")],
    ),
    row(
        " semana",
        " semanas",
        [
            Some("semana passada"),
            Some("esta semana"),
            Some("próxima semana"),
        ],
    ),
    row(
        " mês",
        " meses",
        [Some("mês passado"), Some("este mês"), Some("próximo mês")],
    ),
    row(
        " trimestre",
        " trimestres",
        [
            Some("trimestre passado"),
            Some("este trimestre"),
            Some("próximo trimestre"),
        ],
    ),
    row(
        " ano",
        " anos",
        [Some("ano passado"), Some("este ano"), Some("próximo ano")],
    ),
];

const PT_SHORT: [Row; 8] = [
    row(" seg.", " seg.", [NONE, Some("agora"), NONE]),
    row(" min.", " min.", [NONE, Some("este minuto"), NONE]),
    row(" h", " h", [NONE, Some("esta hora"), NONE]),
    row(
        " dia",
        " dias",
        [Some("ontem"), Some("hoje"), Some("amanhã")],
    ),
    row(
        " sem.",
        " sem.",
        [
            Some("semana passada"),
            Some("esta semana"),
            Some("próxima semana"),
        ],
    ),
    row(
        " mês",
        " meses",
        [Some("mês passado"), Some("este mês"), Some("próximo mês")],
    ),
    row(
        " trim.",
        " trim.",
        [
            Some("trimestre passado"),
            Some("este trimestre"),
            Some("próximo trimestre"),
        ],
    ),
    row(
        " ano",
        " anos",
        [Some("ano passado"), Some("este ano"), Some("próximo ano")],
    ),
];

fn rows(portuguese: bool, style: &str) -> &'static [Row; 8] {
    match (portuguese, style) {
        (false, "short") => &EN_SHORT,
        (false, "narrow") => &EN_NARROW,
        (false, _) => &EN_LONG,
        (true, "long") => &PT_LONG,
        (true, _) => &PT_SHORT,
    }
}

/// The `(before, after)` text around the number for the past or the future.
fn frame(portuguese: bool, past: bool) -> (&'static str, &'static str) {
    match (portuguese, past) {
        (false, true) => ("", " ago"),
        (false, false) => ("in ", ""),
        (true, true) => ("há ", ""),
        (true, false) => ("em ", ""),
    }
}

pub(super) fn parts(
    tag: &str,
    style: &str,
    unit: &str,
    auto: bool,
    decimal: Decimal,
) -> Option<RelativeParts> {
    let portuguese = tag.starts_with("pt");
    let (one, other, phrases) = rows(portuguese, style)[UNITS.iter().position(|u| *u == unit)?];
    if auto {
        let slot = match (decimal.sign() == Sign::Negative, decimal.absolute.is_zero()) {
            (_, true) => 1,
            (true, false) => 0,
            (false, false) => 2,
        };
        if let Some(phrase) = phrases[slot] {
            return Some(literal(phrase));
        }
    }
    let past = decimal.sign() == Sign::Negative;
    let rules = PluralRules::try_new_cardinal((&locale(tag)).into()).ok()?;
    let word = if rules.category_for(&decimal) == PluralCategory::One {
        one
    } else {
        other
    };
    let (before, after) = frame(portuguese, past);
    let mut out = literal(before);
    out.parts.extend(number_parts(tag, &decimal)?.parts);
    out.parts.push(("literal", format!("{word}{after}")));
    Some(out)
}
