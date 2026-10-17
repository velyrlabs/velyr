//! Currency and percent presentation for `Intl.NumberFormat` (VELYR-175).
//! shortcut: the `icu_experimental::dimension` currency formatters take over the digits and
//! cannot yield parts, so tables cover the symbols of common currencies (English and
//! Portuguese columns; other languages use the English symbols), the display names in
//! `currency_names.tsv` (English for every code of `supportedValuesOf`, Portuguese for the
//! common ones, English otherwise) and the affix layout of the languages listed below (the
//! rest use English's).

use icu_locale::Locale;

pub(crate) const NBSP: &str = "\u{a0}";

/// `(code, English, narrow, Portuguese)`; an empty symbol means the code itself.
const SYMBOLS: [(&str, &str, &str, &str); 40] = [
    ("USD", "$", "$", "US$"),
    ("EUR", "\u{20ac}", "\u{20ac}", "\u{20ac}"),
    ("GBP", "\u{a3}", "\u{a3}", "\u{a3}"),
    ("JPY", "\u{a5}", "\u{a5}", "JP\u{a5}"),
    ("BRL", "R$", "R$", "R$"),
    ("CAD", "CA$", "$", "CA$"),
    ("AUD", "A$", "$", "AU$"),
    ("MXN", "MX$", "$", "MX$"),
    ("CNY", "CN\u{a5}", "\u{a5}", "CN\u{a5}"),
    ("INR", "\u{20b9}", "\u{20b9}", "\u{20b9}"),
    ("KRW", "\u{20a9}", "\u{20a9}", "\u{20a9}"),
    ("ILS", "\u{20aa}", "\u{20aa}", "\u{20aa}"),
    ("NZD", "NZ$", "$", "NZ$"),
    ("HKD", "HK$", "$", "HK$"),
    ("TWD", "NT$", "$", "NT$"),
    ("VND", "\u{20ab}", "\u{20ab}", "\u{20ab}"),
    ("PHP", "\u{20b1}", "\u{20b1}", "\u{20b1}"),
    ("CHF", "", "", ""),
    ("ARS", "", "$", ""),
    ("CLP", "", "$", ""),
    ("RUB", "", "\u{20bd}", ""),
    ("TRY", "", "\u{20ba}", ""),
    ("XAF", "FCFA", "FCFA", "FCFA"),
    ("XCD", "EC$", "$", "EC$"),
    ("XPF", "CFPF", "CFPF", "CFPF"),
    ("THB", "", "\u{e3f}", ""),
    ("UAH", "", "\u{20b4}", ""),
    ("NGN", "", "\u{20a6}", ""),
    ("PLN", "", "z\u{142}", ""),
    ("CZK", "", "K\u{10d}", ""),
    ("HUF", "", "Ft", ""),
    ("KZT", "", "\u{20b8}", ""),
    ("GEL", "", "\u{20be}", ""),
    ("EGP", "", "E\u{a3}", ""),
    ("BYN", "", "Br", ""),
    ("KHR", "", "\u{17db}", ""),
    ("LAK", "", "\u{20ad}", ""),
    ("MNT", "", "\u{20ae}", ""),
    ("PYG", "", "\u{20b2}", ""),
    ("BDT", "", "\u{9f3}", ""),
];

/// Currencies whose minor unit is not two digits (ISO 4217).
const DIGITS: [(&str, i16); 14] = [
    ("JPY", 0),
    ("KRW", 0),
    ("VND", 0),
    ("CLP", 0),
    ("ISK", 0),
    ("UGX", 0),
    ("XAF", 0),
    ("XOF", 0),
    ("PYG", 0),
    ("BHD", 3),
    ("KWD", 3),
    ("JOD", 3),
    ("OMR", 3),
    ("TND", 3),
];

pub(crate) fn fraction_digits(code: &str) -> i16 {
    let code = code.to_ascii_uppercase();
    DIGITS
        .iter()
        .find(|(known, _)| *known == code)
        .map_or(2, |(_, digits)| *digits)
}

/// Where the affixes of percent and currency go, by language and region.
pub(crate) struct Layout {
    /// The currency follows the number, after a no-break space.
    pub currency_after: bool,
    /// A prefixed currency is followed by a no-break space (pt-BR `R$ 1,00`).
    pub prefix_space: bool,
    /// A no-break space separates the number and the percent sign.
    pub percent_space: bool,
    /// The currency symbol column to read.
    portuguese: bool,
}

impl Layout {
    pub(crate) fn of(locale: &Locale) -> Self {
        let language = locale.id.language.as_str();
        let region = locale.id.region.map(|region| region.to_string());
        let portugal = language == "pt" && region.as_deref() == Some("PT");
        Self {
            currency_after: portugal
                || matches!(
                    language,
                    "de" | "es"
                        | "fr"
                        | "it"
                        | "ru"
                        | "sv"
                        | "fi"
                        | "cs"
                        | "sk"
                        | "da"
                        | "nb"
                        | "pl"
                        | "uk"
                        | "bg"
                ),
            prefix_space: language == "pt" && !portugal,
            percent_space: matches!(
                language,
                "de" | "es" | "fr" | "ru" | "sv" | "fi" | "cs" | "sk" | "da" | "nb" | "uk" | "bg"
            ),
            portuguese: language == "pt",
        }
    }

    /// The text for `code` under `display` (`symbol`, `narrowSymbol` or `code`).
    pub(crate) fn symbol(&self, code: &str, display: &str) -> String {
        let code = code.to_ascii_uppercase();
        let row = SYMBOLS.iter().find(|(known, ..)| *known == code);
        let pick = |symbol: &str| {
            if symbol.is_empty() {
                code.clone()
            } else {
                symbol.to_owned()
            }
        };
        match (display, row) {
            ("code", _) | (_, None) => code,
            ("narrowSymbol", Some((_, _, narrow, _))) => pick(narrow),
            (_, Some((_, english, _, portuguese))) => {
                pick(if self.portuguese { portuguese } else { english })
            }
        }
    }
}

const NAMES: &str = include_str!("currency_names.tsv");

impl Layout {
    /// The display name of `code` (`currencyDisplay: 'name'`); an unknown code names itself.
    pub(crate) fn name(&self, code: &str, one: bool) -> String {
        let code = code.to_ascii_uppercase();
        let row: Option<Vec<&str>> = NAMES
            .lines()
            .map(|line| line.split('\t').collect())
            .find(|fields: &Vec<&str>| fields[0] == code);
        let Some(row) = row else { return code };
        let column = 1 + usize::from(!one);
        let own = if self.portuguese { row[column + 2] } else { "" };
        if own.is_empty() { row[column] } else { own }.to_owned()
    }

    /// Whether a prefixed `symbol` needs a space before the number: the locale's pattern
    /// has one, or the symbol ends in a letter (`CHF 1.00`).
    pub(crate) fn spaces_prefix(&self, symbol: &str) -> bool {
        self.prefix_space || symbol.chars().last().is_some_and(char::is_alphabetic)
    }
}
