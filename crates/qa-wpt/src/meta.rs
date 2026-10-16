//! WPT `// META:` header of a `.any.js` file, and the reasons a test cannot run without wptserve.

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// `META: script=` values, in order.
    pub scripts: Vec<String>,
    /// `META: variant=` values (`?a=b`); empty means one run without a query string.
    pub variants: Vec<String>,
}

/// Reads the leading comment block; the first line that is neither blank nor a comment ends it.
pub fn parse(source: &str) -> Meta {
    let mut meta = Meta::default();
    for line in source.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        let Some(comment) = line.strip_prefix("//") else {
            break;
        };
        let Some((key, value)) = comment
            .trim()
            .strip_prefix("META:")
            .and_then(|rest| rest.trim().split_once('='))
        else {
            continue;
        };
        match key.trim() {
            "script" => meta.scripts.push(value.trim().to_owned()),
            "variant" => meta.variants.push(value.trim().to_owned()),
            _ => {}
        }
    }
    meta
}

/// Markers of tests that need the WPT server (substitution, host info, testdriver, IDL fetch).
const SERVER_MARKERS: [(&str, &str); 5] = [
    ("get-host-info", "needs wptserve host info"),
    ("get_host_info", "needs wptserve host info"),
    ("testdriver", "needs the testdriver/webdriver bridge"),
    (
        "idlharness",
        "idlharness fetches /interfaces/*.idl from wptserve",
    ),
    (".sub.", "needs wptserve substitution"),
];

pub fn skip_reason(path: &str, source: &str, meta: &Meta) -> Option<&'static str> {
    let in_scripts = meta.scripts.iter().map(String::as_str);
    SERVER_MARKERS.iter().find_map(|(marker, reason)| {
        let hit = path.contains(marker)
            || source.contains(marker)
            || in_scripts.clone().any(|script| script.contains(marker));
        hit.then_some(*reason)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scripts_and_variants_until_code() {
        let source = "// META: global=window,worker\n// META: script=/resources/a.js\n\n// META: script=b.js\n// META: variant=?x\n// META: variant=\ntest(() => {});\n// META: script=late.js\n";
        let meta = parse(source);
        assert_eq!(meta.scripts, ["/resources/a.js", "b.js"]);
        assert_eq!(meta.variants, ["?x", ""]);
    }

    #[test]
    fn ignores_plain_comments() {
        let meta = parse("// copyright\n// META: timeout=long\ntest();");
        assert_eq!(meta, Meta::default());
    }

    #[test]
    fn skips_server_dependent_tests() {
        let plain = parse("test();");
        assert_eq!(skip_reason("a/b.any.js", "test();", &plain), None);
        assert!(skip_reason("a/b.sub.any.js", "test();", &plain).is_some());
        assert!(skip_reason("a/b.any.js", "get_host_info()", &plain).is_some());
        let idl = parse("// META: script=/resources/idlharness.js\n");
        assert!(skip_reason("a/idl.any.js", "", &idl).is_some());
    }
}
