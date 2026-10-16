//! `Intl.Collator` comparison over ICU4X collation. Collators are cached per resolved
//! configuration, so `array.sort(collator.compare)` builds one.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;

use icu_collator::options::{AlternateHandling, CaseLevel, CollatorOptions, Strength};
use icu_collator::preferences::{CollationCaseFirst, CollationNumericOrdering};
use icu_collator::{Collator, CollatorBorrowed, CollatorPreferences};
use velyr_engine_vm::Value;
use velyr_engine_vm::interpreter::NativeFn;

use super::{bool_arg, locale, range_error, string_arg};

fn options(sensitivity: &str, ignore_punctuation: bool) -> CollatorOptions {
    let mut options = CollatorOptions::default();
    (options.strength, options.case_level) = match sensitivity {
        "base" => (Some(Strength::Primary), None),
        "accent" => (Some(Strength::Secondary), None),
        "case" => (Some(Strength::Primary), Some(CaseLevel::On)),
        _ => (Some(Strength::Tertiary), None),
    };
    if ignore_punctuation {
        options.alternate_handling = Some(AlternateHandling::Shifted);
    }
    options
}

fn preferences(tag: &str, numeric: bool, case_first: &str) -> CollatorPreferences {
    let mut prefs = CollatorPreferences::from(&locale(tag));
    if numeric {
        prefs.numeric_ordering = Some(CollationNumericOrdering::True);
    }
    prefs.case_first = match case_first {
        "upper" => Some(CollationCaseFirst::Upper),
        "lower" => Some(CollationCaseFirst::Lower),
        "false" => Some(CollationCaseFirst::False),
        _ => prefs.case_first,
    };
    prefs
}

/// `(locale, sensitivity, numeric, caseFirst, ignorePunctuation, x, y)` -> -1, 0 or 1.
/// shortcut: the cache never shrinks; it holds one entry per distinct configuration.
pub(crate) fn compare() -> NativeFn {
    let cache: Rc<RefCell<HashMap<String, CollatorBorrowed<'static>>>> = Rc::default();
    Rc::new(move |vm, _, args| {
        let key: Vec<String> = (0..5).map(|i| format!("{:?}", args.get(i))).collect();
        let key = key.join("|");
        let mut cache = cache.borrow_mut();
        if !cache.contains_key(&key) {
            let prefs = preferences(
                &string_arg(args, 0),
                bool_arg(args, 2),
                &string_arg(args, 3),
            );
            let options = options(&string_arg(args, 1), bool_arg(args, 4));
            let Ok(collator) = Collator::try_new(prefs, options) else {
                return Err(range_error(vm, "No collation data for this locale"));
            };
            cache.insert(key.clone(), collator);
        }
        let units = |i: usize| match args.get(i) {
            Some(Value::String(s)) => s.to_utf16(),
            _ => Vec::new(),
        };
        let order = cache[&key].compare_utf16(&units(5), &units(6));
        Ok(Value::Number(match order {
            Ordering::Less => -1.0,
            Ordering::Equal => 0.0,
            Ordering::Greater => 1.0,
        }))
    })
}
