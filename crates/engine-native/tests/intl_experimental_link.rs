//! Compile gate: the optional ICU4X crates behind `intl-experimental` link.

#[cfg(feature = "intl-experimental")]
#[test]
fn experimental_crates_link() {
    let _ = icu_list::ListFormatter::try_new_and(
        Default::default(),
        icu_list::options::ListFormatterOptions::default(),
    );
}
