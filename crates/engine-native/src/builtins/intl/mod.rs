//! ECMA-402 `Intl` (VELYR-114) on ICU4X compiled data. The JS objects (constructors,
//! options processing, internal slots in a WeakMap) live in `prelude.js`; it captures the
//! native ops below, which take and return primitives, and deletes them from the global.
//! `toString` of the JS-defined methods prints `[native code]`, like the iterator helpers.

mod collator;
mod currency;
mod datetime;
mod display;
mod duration;
mod enumeration;
mod list;
mod locale;
mod number;
mod relative;
mod segmenter;

use std::fmt;
use std::rc::Rc;

use icu_locale::{Locale, LocaleCanonicalizer};
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::{Abrupt, ErrorKind, Interpreter, LazyInstall, NativeFn};
use writeable::{Part, PartsWrite};

pub(crate) use datetime::zone_long_name;

use crate::prelude::run_prelude;

const PRELUDE: &str = include_str!("prelude.js");
const PRELUDE_NUMBER: &str = include_str!("prelude_number.js");
const PRELUDE_FORMATS: &str = include_str!("prelude_formats.js");
const EXTRA_PRELUDES: &[(&str, &str)] = &[
    ("velyr:intl_locale", include_str!("prelude_locale.js")),
    ("velyr:intl_enum", include_str!("prelude_enum.js")),
    ("velyr:intl_list", include_str!("prelude_list.js")),
    ("velyr:intl_relative", include_str!("prelude_relative.js")),
    ("velyr:intl_display", include_str!("prelude_display.js")),
    ("velyr:intl_duration", include_str!("prelude_duration.js")),
];

/// Locale used when none of the requested ones is supported.
const DEFAULT_LOCALE: &str = "en-US";

pub(crate) type OpResult = Result<Value, Abrupt>;

/// `args[i]` as a Rust string (`undefined` -> empty).
pub(crate) fn string_arg(args: &[Value], i: usize) -> String {
    match args.get(i) {
        Some(Value::String(s)) => s.to_string_lossy(),
        _ => String::new(),
    }
}

pub(crate) fn number_arg(args: &[Value], i: usize) -> f64 {
    match args.get(i) {
        Some(Value::Number(n)) => *n,
        _ => f64::NAN,
    }
}

pub(crate) fn bool_arg(args: &[Value], i: usize) -> bool {
    matches!(args.get(i), Some(Value::Bool(true)))
}

pub(crate) fn range_error(vm: &mut Interpreter, message: &str) -> Abrupt {
    vm.error(ErrorKind::Range, message)
}

/// A resolved locale tag from the prelude; it was canonicalized there.
pub(crate) fn locale(tag: &str) -> Locale {
    Locale::try_from_str(tag).unwrap_or(Locale::UNKNOWN)
}

fn canonicalize(vm: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let tag = string_arg(args, 0);
    let Ok(mut locale) = Locale::try_from_str(&tag) else {
        return Err(range_error(
            vm,
            &format!("Incorrect locale information provided: {tag}"),
        ));
    };
    LocaleCanonicalizer::new_extended().canonicalize(&mut locale);
    Ok(Value::str(&locale.to_string()))
}

/// Whether `tag` (no extensions) is offered; see the `intl-all-locales` feature.
fn supported(_: &mut Interpreter, _: &Value, args: &[Value]) -> OpResult {
    let tag = string_arg(args, 0);
    let language = tag.split('-').next().unwrap_or_default();
    let offered = cfg!(feature = "intl-all-locales") || matches!(language, "en" | "pt");
    Ok(Value::Bool(offered && Locale::try_from_str(&tag).is_ok()))
}

fn default_locale(_: &mut Interpreter, _: &Value, _: &[Value]) -> OpResult {
    Ok(Value::str(DEFAULT_LOCALE))
}

/// Collects formatted parts as `(type, text)`; text outside a part is `literal`.
pub(crate) struct Parts<F> {
    pub parts: Vec<(&'static str, String)>,
    stack: Vec<&'static str>,
    name: F,
}

impl<F: Fn(Part) -> &'static str> Parts<F> {
    pub(crate) fn new(name: F) -> Self {
        Self {
            parts: Vec::new(),
            stack: Vec::new(),
            name,
        }
    }

    /// `type\u{1}text\u{2}…`, decoded by the prelude.
    pub(crate) fn encode(&self) -> String {
        let parts = self
            .parts
            .iter()
            .map(|(kind, text)| format!("{kind}\u{1}{text}"));
        parts.collect::<Vec<_>>().join("\u{2}")
    }
}

impl<F> fmt::Write for Parts<F> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let kind = self.stack.last().copied().unwrap_or("literal");
        match self.parts.last_mut() {
            Some((last, buffer)) if *last == kind => buffer.push_str(text),
            _ => self.parts.push((kind, text.to_owned())),
        }
        Ok(())
    }
}

impl<F: Fn(Part) -> &'static str> PartsWrite for Parts<F> {
    type SubPartsWrite = Self;

    fn with_part(
        &mut self,
        part: Part,
        mut f: impl FnMut(&mut Self::SubPartsWrite) -> fmt::Result,
    ) -> fmt::Result {
        // An unknown nested part (the decimal digits of an hour) takes the outer type.
        let kind = match ((self.name)(part), self.stack.last()) {
            ("literal", Some(outer)) => *outer,
            (kind, _) => kind,
        };
        self.stack.push(kind);
        // A nested part of the same kind starts a new entry (e.g. integer, group, integer).
        self.parts.push((kind, String::new()));
        let result = f(self);
        self.stack.pop();
        result
    }
}

fn op(vm: &mut Interpreter, name: &str, arity: u32, f: NativeFn) -> Result<(), Abrupt> {
    vm.define_global_function(name, arity, f)
}

/// Defines the lazy `Intl` namespace on `vm`'s realm; the first operation on it defines the
/// ops and runs the preludes, which also replace the locale-sensitive built-in stubs.
pub(crate) fn install(vm: &mut Interpreter) -> Result<(), String> {
    let init: LazyInstall =
        Rc::new(|vm| materialize(vm).map_err(|message| vm.error(ErrorKind::Error, &message)));
    vm.define_lazy_namespace("Intl", init)
        .map_err(|_| "cannot define Intl".to_owned())
}

fn materialize(vm: &mut Interpreter) -> Result<(), String> {
    let mut ops: Vec<(&str, u32, NativeFn)> = vec![
        ("__intl_canonicalize", 1, Rc::new(canonicalize)),
        ("__intl_supported", 1, Rc::new(supported)),
        ("__intl_default_locale", 0, Rc::new(default_locale)),
        ("__intl_number_format", 4, Rc::new(number::format)),
        (
            "__intl_currency_digits",
            1,
            Rc::new(number::currency_digits),
        ),
        ("__intl_plural_select", 5, Rc::new(number::plural_select)),
        (
            "__intl_plural_categories",
            2,
            Rc::new(number::plural_categories),
        ),
        ("__intl_collator_compare", 7, collator::compare()),
        ("__intl_date_time_format", 5, Rc::new(datetime::format)),
        (
            "__intl_canonical_zone",
            1,
            Rc::new(datetime::canonical_zone),
        ),
        ("__intl_default_zone", 0, Rc::new(datetime::default_zone)),
        ("__intl_segment", 2, Rc::new(segmenter::segment)),
    ];
    for extra in [
        locale::ops(),
        enumeration::ops(),
        list::ops(),
        relative::ops(),
        display::ops(),
        duration::ops(),
    ] {
        ops.extend(extra);
    }
    for (name, arity, f) in ops {
        op(vm, name, arity, f).map_err(|_| format!("cannot define {name}"))?;
    }
    for (name, source) in [
        ("velyr:intl", PRELUDE),
        ("velyr:intl_number", PRELUDE_NUMBER),
        ("velyr:intl_formats", PRELUDE_FORMATS),
    ]
    .into_iter()
    .chain(EXTRA_PRELUDES.iter().copied())
    {
        run_prelude(vm, name, source)?;
    }
    // The extra preludes read the shared helpers; nothing may see them afterwards.
    run_prelude(vm, "velyr:intl_cleanup", "delete globalThis.__intl_shared;")

}
