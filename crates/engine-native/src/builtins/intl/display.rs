//! `Intl.DisplayNames` op: region, language and script names come from ICU4X
//! `icu_experimental::displaynames` with `intl-experimental`, otherwise from a table;
//! currencies, calendars and date-time fields always come from `display_table`.

use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Interpreter, NativeFn};

use super::display_table::{CALENDARS, CURRENCIES, FIELDS, Row, lookup};
use super::{OpResult, string_arg};

pub(super) fn ops() -> Vec<(&'static str, u32, NativeFn)> {
    vec![("__intl_display_name", 5, std::rc::Rc::new(of))]
}

/// `__intl_display_name(locale, type, style, languageDisplay, code)` -> name, or `undefined`
/// when there is no data (the prelude applies `fallback`). `code` is already canonical.
fn of(_: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let (tag, kind, style, language_display, code) = (
        string_arg(args, 0),
        string_arg(args, 1),
        string_arg(args, 2),
        string_arg(args, 3),
        string_arg(args, 4),
    );
    let portuguese = tag.starts_with("pt");
    let table = |rows: &[Row]| lookup(rows, &code, portuguese).map(str::to_owned);
    let name = match kind.as_str() {
        "currency" => table(CURRENCIES),
        "calendar" => table(CALENDARS),
        "dateTimeField" => table(FIELDS),
        _ => icu_or_table(&tag, &kind, &style, &language_display, &code),
    };
    Ok(name.map_or(Value::Undefined, |name| Value::str(&name)))
}

#[cfg(feature = "intl-experimental")]
fn icu_or_table(
    tag: &str,
    kind: &str,
    style: &str,
    language_display: &str,
    code: &str,
) -> Option<String> {
    use icu_experimental::displaynames::multi::{
        LanguageDisplayNames, LocaleDisplayNamesFormatter, RegionDisplayNames, ScriptDisplayNames,
    };
    use icu_experimental::displaynames::{DisplayNamesOptions, LanguageDisplay, Style};
    use icu_locale::Locale;
    use icu_locale::subtags::{Language, Region, Script};

    let mut options = DisplayNamesOptions::default();
    options.style = Some(match style {
        "short" => Style::Short,
        "narrow" => Style::Narrow,
        _ => Style::Long,
    });
    if language_display == "standard" {
        options.language_display = LanguageDisplay::Standard;
    }
    let prefs = (&super::locale(tag)).into();
    match kind {
        "region" => RegionDisplayNames::try_new(prefs, options)
            .ok()?
            .of(Region::try_from_str(code).ok()?)
            .map(str::to_owned),
        "script" => ScriptDisplayNames::try_new(prefs, options)
            .ok()?
            .of(Script::try_from_str(code).ok()?)
            .map(str::to_owned),
        _ => {
            let locale = Locale::try_from_str(code).ok()?;
            // The formatter always answers; require data for the language subtag itself.
            let language: Language = locale.id.language;
            LanguageDisplayNames::try_new(prefs, options)
                .ok()?
                .of(language)?;
            let formatter = LocaleDisplayNamesFormatter::try_new(prefs, options).ok()?;
            Some(formatter.of(&locale).into_owned())
        }
    }
}

#[cfg(not(feature = "intl-experimental"))]
fn icu_or_table(tag: &str, kind: &str, _: &str, _: &str, code: &str) -> Option<String> {
    use super::display_table::{LANGUAGES, REGIONS, SCRIPTS};

    let rows = match kind {
        "region" => REGIONS,
        "script" => SCRIPTS,
        _ => LANGUAGES,
    };
    lookup(rows, code, tag.starts_with("pt")).map(str::to_owned)
}
