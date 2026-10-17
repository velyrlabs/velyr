//! `Intl.ListFormat` ops: ICU4X `icu_list` with `intl-experimental`, otherwise a small
//! table for `en` and `pt` (the only locales offered without `intl-all-locales`).

use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Abrupt, Interpreter, NativeFn};
use writeable::Part;

use super::{OpResult, Parts, string_arg};
#[cfg(feature = "intl-experimental")]
use super::{locale, range_error};

type ListParts = Parts<fn(Part) -> &'static str>;

pub(super) fn ops() -> Vec<(&'static str, u32, NativeFn)> {
    vec![("__intl_list_format", 3, std::rc::Rc::new(format))]
}

/// `__intl_list_format(locale, type, style, ...items)` -> encoded `element`/`literal` parts.
fn format(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let (tag, kind, style) = (
        string_arg(args, 0),
        string_arg(args, 1),
        string_arg(args, 2),
    );
    let items: Vec<String> = (3..args.len()).map(|i| string_arg(args, i)).collect();
    let mut out = list_parts(vm, &tag, &kind, &style, &items)?;
    // Empty literals are noise; an empty element is a real (empty) item.
    out.parts
        .retain(|(name, text)| *name == "element" || !text.is_empty());
    Ok(Value::str(&out.encode()))
}

#[cfg(feature = "intl-experimental")]
fn part_name(part: Part) -> &'static str {
    if part == icu_list::parts::ELEMENT {
        "element"
    } else {
        "literal"
    }
}

#[cfg(feature = "intl-experimental")]
fn list_parts(
    vm: &mut Interpreter,
    tag: &str,
    kind: &str,
    style: &str,
    items: &[String],
) -> Result<ListParts, Abrupt> {
    use icu_list::ListFormatter;
    use icu_list::options::{ListFormatterOptions, ListLength};
    use writeable::Writeable;

    let length = match style {
        "short" => ListLength::Short,
        "narrow" => ListLength::Narrow,
        _ => ListLength::Wide,
    };
    let options = ListFormatterOptions::default().with_length(length);
    let prefs = (&locale(tag)).into();
    let formatter = match kind {
        "disjunction" => ListFormatter::try_new_or(prefs, options),
        "unit" => ListFormatter::try_new_unit(prefs, options),
        _ => ListFormatter::try_new_and(prefs, options),
    };
    let Ok(formatter) = formatter else {
        return Err(range_error(vm, "No list data for this locale"));
    };
    let mut out = Parts::new(part_name as fn(Part) -> &'static str);
    let _ = formatter
        .format(items.iter().map(String::as_str))
        .write_to_parts(&mut out);
    Ok(out)
}

/// Separators `(pair, middle, last)` of the fallback table.
#[cfg(not(feature = "intl-experimental"))]
fn separators(tag: &str, kind: &str, style: &str) -> (&'static str, &'static str, &'static str) {
    let pt = tag.starts_with("pt");
    match (pt, kind, style) {
        (false, "disjunction", _) => (" or ", ", ", ", or "),
        (false, "unit", "narrow") => (" ", " ", " "),
        (false, "unit", _) => (", ", ", ", ", "),
        (false, _, "narrow") => (", ", ", ", ", "),
        (false, _, "short") => (" & ", ", ", ", & "),
        (false, _, _) => (" and ", ", ", ", and "),
        (true, "disjunction", _) => (" ou ", ", ", " ou "),
        (true, "unit", "narrow") => (" ", " ", " "),
        (true, "unit", "short") => (", ", ", ", " e "),
        (true, _, _) => (" e ", ", ", " e "),
    }
}

#[cfg(not(feature = "intl-experimental"))]
fn list_parts(
    _: &mut Interpreter,
    tag: &str,
    kind: &str,
    style: &str,
    items: &[String],
) -> Result<ListParts, Abrupt> {
    let (pair, middle, last) = separators(tag, kind, style);
    let mut out = Parts::new((|_| "literal") as fn(Part) -> &'static str);
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            let separator = match (items.len(), i) {
                (2, _) => pair,
                (n, i) if i == n - 1 => last,
                _ => middle,
            };
            out.parts.push(("literal", separator.to_owned()));
        }
        out.parts.push(("element", item.clone()));
    }
    Ok(out)
}
