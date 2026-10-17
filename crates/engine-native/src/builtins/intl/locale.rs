//! `Intl.Locale` ops: tag building and canonicalization, field access and
//! maximize/minimize. The JS side (`prelude_locale.js`) validates the option types and
//! keeps the canonical tag in the instance slot.

use std::str::FromStr;

use icu_locale::extensions::unicode::{Key, Value as KeywordValue};
use icu_locale::subtags::{Language, Region, Script, Variant, Variants};
use icu_locale::{Locale, LocaleCanonicalizer, LocaleExpander};
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter, NativeFn};

use super::{OpResult, range_error, string_arg};

const SEPARATOR: &str = "\u{1}";
/// Keywords set by `canonical` (args 5..), in order.
const SETTABLE: [&str; 7] = ["ca", "co", "hc", "kf", "kn", "nu", "fw"];
/// Keywords reported by `parts` (fields 5..): the settable ones, then the region hints.
const REPORTED: [&str; 9] = ["ca", "co", "hc", "kf", "kn", "nu", "fw", "rg", "sd"];

fn invalid(vm: &mut Interpreter, what: &str, value: &str) -> Abrupt {
    range_error(
        vm,
        &format!("Incorrect locale information provided: {what} {value}"),
    )
}

fn parse(vm: &mut Interpreter, tag: &str) -> Result<Locale, Abrupt> {
    Locale::try_from_str(tag).map_err(|_| invalid(vm, "tag", tag))
}

/// `(tag, language, script, region, variants, ca, co, hc, kf, kn, nu, fw)` -> canonical tag;
/// an empty string leaves the part of `tag` alone and any keyword may be `"true"`.
fn canonical(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let mut locale = parse(vm, &string_arg(args, 0))?;
    let text = |i| string_arg(args, i);
    if !text(1).is_empty() {
        locale.id.language =
            Language::from_str(&text(1)).map_err(|_| invalid(vm, "language", &text(1)))?;
    }
    if !text(2).is_empty() {
        locale.id.script =
            Some(Script::from_str(&text(2)).map_err(|_| invalid(vm, "script", &text(2)))?);
    }
    if !text(3).is_empty() {
        locale.id.region =
            Some(Region::from_str(&text(3)).map_err(|_| invalid(vm, "region", &text(3)))?);
    }
    if !text(4).is_empty() {
        locale.id.variants = variants(&text(4)).ok_or_else(|| invalid(vm, "variants", &text(4)))?;
    }
    for (offset, key) in SETTABLE.iter().enumerate() {
        let value = text(5 + offset);
        if value.is_empty() {
            continue;
        }
        let keyword = if value == "true" {
            KeywordValue::default()
        } else {
            KeywordValue::try_from_str(&value).map_err(|_| invalid(vm, key, &value))?
        };
        locale
            .extensions
            .unicode
            .keywords
            .set(Key::try_from_str(key).expect("static key"), keyword);
    }
    LocaleCanonicalizer::new_extended().canonicalize(&mut locale);
    alias_calendar(&mut locale);
    Ok(Value::str(&locale.to_string()))
}

/// The canonical `ca` values the canonicalizer data does not map.
fn alias_calendar(locale: &mut Locale) {
    let key = Key::try_from_str("ca").expect("static key");
    let canonical = match locale
        .extensions
        .unicode
        .keywords
        .get(&key)
        .map(|v| v.to_string())
    {
        Some(value) if value == "islamicc" => "islamic-civil",
        Some(value) if value == "ethiopic-amete-alem" => "ethioaa",
        _ => return,
    };
    let value = KeywordValue::try_from_str(canonical).expect("static value");
    locale.extensions.unicode.keywords.set(key, value);
}

/// `tag` -> `baseName, language, script, region, variants, ca, co, hc, kf, kn, nu, fw, rg, sd` joined by
/// `\u{1}`; absent parts are empty and a valueless keyword is `"true"`.
fn parts(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let locale = parse(vm, &string_arg(args, 0))?;
    let id = &locale.id;
    let mut fields = vec![
        id.to_string(),
        id.language.to_string(),
        id.script.map(|s| s.to_string()).unwrap_or_default(),
        id.region.map(|r| r.to_string()).unwrap_or_default(),
        id.variants.to_string(),
    ];
    for key in REPORTED {
        let key = Key::try_from_str(key).expect("static key");
        fields.push(match locale.extensions.unicode.keywords.get(&key) {
            Some(value) if value.is_empty() => "true".to_owned(),
            Some(value) => value.to_string(),
            None => String::new(),
        });
    }
    Ok(Value::str(&fields.join(SEPARATOR)))
}

/// A `-`-separated list of distinct variant subtags, sorted.
fn variants(text: &str) -> Option<Variants> {
    let mut list = Vec::new();
    for part in text.split('-') {
        let variant = Variant::from_str(part).ok()?;
        if list.contains(&variant) {
            return None;
        }
        list.push(variant);
    }
    list.sort();
    Some(Variants::from_vec_unchecked(list))
}

/// `(tag, "max" | "min")` -> the tag with the language id maximized or minimized.
fn expand(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let mut locale = parse(vm, &string_arg(args, 0))?;
    let expander = LocaleExpander::new_extended();
    if string_arg(args, 1) == "max" {
        expander.maximize(&mut locale.id);
    } else {
        expander.minimize(&mut locale.id);
    }
    Ok(Value::str(&locale.to_string()))
}

pub(super) fn ops() -> Vec<(&'static str, u32, NativeFn)> {
    vec![
        ("__intl_locale_canonical", 12, std::rc::Rc::new(canonical)),
        ("__intl_locale_parts", 1, std::rc::Rc::new(parts)),
        ("__intl_locale_expand", 2, std::rc::Rc::new(expand)),
    ]
}
