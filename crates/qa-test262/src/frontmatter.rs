//! Test262 frontmatter: the YAML block between `/*---` and `---*/`.
//! Only the keys the runner needs are read; everything else is ignored.

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub includes: Vec<String>,
    pub only_strict: bool,
    pub no_strict: bool,
    pub module: bool,
    pub is_async: bool,
    pub raw: bool,
    pub negative: Option<Negative>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Negative {
    pub phase: String,
    pub error_type: String,
}

pub fn parse(source: &str) -> Meta {
    let mut meta = Meta::default();
    let Some(block) = source
        .split_once("/*---")
        .and_then(|(_, rest)| rest.split_once("---*/"))
        .map(|(block, _)| block)
    else {
        return meta;
    };
    let mut key = "";
    for line in block.lines() {
        if line.starts_with(|c: char| c.is_whitespace()) {
            apply_nested(&mut meta, key, line.trim());
        } else if let Some((k, value)) = line.split_once(':') {
            key = k.trim();
            if let Some(inner) = value.trim().strip_prefix('[') {
                for item in inner.trim_end_matches(']').split(',') {
                    apply_item(&mut meta, key, item.trim());
                }
            }
        }
    }
    meta
}

fn apply_nested(meta: &mut Meta, key: &str, line: &str) {
    if let Some(item) = line.strip_prefix("- ") {
        return apply_item(meta, key, item.trim());
    }
    if key != "negative" {
        return;
    }
    let negative = meta.negative.get_or_insert_with(Negative::default);
    match line.split_once(':') {
        Some(("phase", value)) => negative.phase = value.trim().to_owned(),
        Some(("type", value)) => negative.error_type = value.trim().to_owned(),
        _ => {}
    }
}

fn apply_item(meta: &mut Meta, key: &str, item: &str) {
    if item.is_empty() {
        return;
    }
    match (key, item) {
        ("includes", _) => meta.includes.push(item.to_owned()),
        ("flags", "onlyStrict") => meta.only_strict = true,
        ("flags", "noStrict") => meta.no_strict = true,
        ("flags", "module") => meta.module = true,
        ("flags", "async") => meta.is_async = true,
        ("flags", "raw") => meta.raw = true,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(body: &str) -> String {
        format!("// header\n/*---\n{body}\n---*/\nvar x;")
    }

    #[test]
    fn no_frontmatter_is_default() {
        assert_eq!(parse("var x;"), Meta::default());
    }

    #[test]
    fn inline_lists() {
        let meta = parse(&wrap(
            "description: d\nincludes: [propertyHelper.js, compareArray.js]\nflags: [onlyStrict, async]",
        ));
        assert_eq!(meta.includes, ["propertyHelper.js", "compareArray.js"]);
        assert!(meta.only_strict && meta.is_async);
        assert!(!meta.no_strict && !meta.module && !meta.raw);
    }

    #[test]
    fn block_lists() {
        let meta = parse(&wrap(
            "includes:\n  - a.js\n  - b.js\nflags:\n  - raw\n  - noStrict",
        ));
        assert_eq!(meta.includes, ["a.js", "b.js"]);
        assert!(meta.raw && meta.no_strict);
    }

    #[test]
    fn negative_block() {
        let meta = parse(&wrap(
            "negative:\n  phase: parse\n  type: SyntaxError\nflags: [module]",
        ));
        assert!(meta.module);
        assert_eq!(
            meta.negative,
            Some(Negative {
                phase: "parse".into(),
                error_type: "SyntaxError".into()
            })
        );
    }

    #[test]
    fn multiline_scalars_do_not_leak_keys() {
        let meta = parse(&wrap(
            "info: |\n  flags: [raw]\n  includes: [x.js]\ndescription: ok\nflags: []",
        ));
        assert_eq!(meta, Meta::default());
    }

    #[test]
    fn empty_inline_list() {
        assert_eq!(parse(&wrap("includes: []")).includes.len(), 0);
    }
}
