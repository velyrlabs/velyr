//! Best-effort list of the static import specifiers in a module's text, so the loader can read a
//! graph before the engine links it. A miss or a false hit is harmless: the link still asks for
//! every module it needs and reads whatever the scan missed; reads of false hits are discarded.

/// Keywords after which a `/` starts a regular expression.
const REGEX_AFTER: [&str; 14] = [
    "return",
    "typeof",
    "case",
    "do",
    "else",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "instanceof",
    "yield",
    "await",
];

/// Specifiers of `import ... from "x"`, `import "x"` and `export ... from "x"`, in source order.
/// Specifiers with escapes are skipped (the engine sees them decoded).
pub(super) fn static_imports(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    // Whether a `/` here starts a regular expression rather than a division.
    let mut regex_allowed = true;
    // Last significant byte, to tell `x.import` from `import`.
    let mut previous = 0u8;
    while i < b.len() {
        let c = b[i];
        if let Some(end) = skip_comment(b, i) {
            i = end;
            continue;
        }
        let (end, allowed) = match c {
            b'\'' | b'"' => (skip_string(b, i), false),
            b'`' => (skip_template(b, i), false),
            b'/' if regex_allowed => (skip_regex(b, i), false),
            b')' | b']' | b'}' => (i + 1, false),
            _ if c.is_ascii_digit() => (i + 1, false),
            _ if c.is_ascii_whitespace() => {
                i += 1;
                continue;
            }
            _ if is_ident_start(c) => {
                let mut end = ident_end(b, i);
                let word = &text[i..end];
                if matches!(word, "import" | "export")
                    && previous != b'.'
                    && let Some((specifier, clause_end)) = clause(text, end, word == "export")
                {
                    found.extend(specifier);
                    end = clause_end;
                }
                (end, REGEX_AFTER.contains(&word))
            }
            _ => (i + 1, true),
        };
        i = end;
        regex_allowed = allowed;
        previous = b[end - 1];
    }
    found
}

/// After `import`/`export` ending at `i`: the specifier of the statement (`None` inside when it
/// has escapes) and where it ends. `None` when this is not a static import or re-export.
fn clause(text: &str, mut i: usize, export: bool) -> Option<(Option<&str>, usize)> {
    let b = text.as_bytes();
    i = skip_blank(b, i);
    if export && !matches!(b.get(i), Some(b'{' | b'*')) {
        return None;
    }
    // `import "x"` takes the string right away; otherwise only after `from`.
    let mut after_from = !export;
    loop {
        i = skip_blank(b, i);
        let c = *b.get(i)?;
        if c == b'\'' || c == b'"' {
            if !after_from {
                return None;
            }
            let end = skip_string(b, i);
            let closed = end > i + 1 && b[end - 1] == c;
            let specifier = &text[i + 1..if closed { end - 1 } else { end }];
            let plain = closed && !specifier.contains('\\');
            return Some((plain.then_some(specifier), end));
        }
        if matches!(c, b'{' | b'}' | b',' | b'*') {
            i += 1;
            after_from = false;
        } else if is_ident_start(c) {
            let end = ident_end(b, i);
            after_from = &text[i..end] == "from";
            i = end;
        } else {
            return None;
        }
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80
}

fn ident_end(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && (is_ident_start(b[i]) || b[i].is_ascii_digit()) {
        i += 1;
    }
    i
}

fn skip_blank(b: &[u8], mut i: usize) -> usize {
    loop {
        if let Some(end) = skip_comment(b, i) {
            i = end;
        } else if b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        } else {
            return i;
        }
    }
}

fn skip_comment(b: &[u8], i: usize) -> Option<usize> {
    match (b.get(i), b.get(i + 1)) {
        (Some(b'/'), Some(b'/')) => Some(find(b, i + 2, b"\n").map_or(b.len(), |n| n + 1)),
        (Some(b'/'), Some(b'*')) => Some(find(b, i + 2, b"*/").map_or(b.len(), |n| n + 2)),
        _ => None,
    }
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b[from.min(b.len())..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Past the closing quote of the string at `i` (or the end of its line when unterminated).
fn skip_string(b: &[u8], i: usize) -> usize {
    let quote = b[i];
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'\n' => return j,
            c if c == quote => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// Past the closing backtick of the template at `i`, `${...}` parts included.
fn skip_template(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'`' => return j + 1,
            b'$' if b.get(j + 1) == Some(&b'{') => j = skip_braces(b, j + 1),
            _ => j += 1,
        }
    }
    b.len()
}

/// Past the `}` matching the `{` at `i`, skipping strings, templates and comments inside.
fn skip_braces(b: &[u8], i: usize) -> usize {
    let mut depth = 0usize;
    let mut j = i;
    while j < b.len() {
        if let Some(end) = skip_comment(b, j) {
            j = end;
            continue;
        }
        match b[j] {
            b'\'' | b'"' => j = skip_string(b, j),
            b'`' => j = skip_template(b, j),
            b'{' => {
                depth += 1;
                j += 1;
            }
            b'}' => {
                depth -= 1;
                j += 1;
                if depth == 0 {
                    return j;
                }
            }
            _ => j += 1,
        }
    }
    b.len()
}

/// Past the regular expression literal at `i` and its flags.
fn skip_regex(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    let mut in_class = false;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 1,
            b'\n' => return j,
            b'[' => in_class = true,
            b']' => in_class = false,
            b'/' if !in_class => return ident_end(b, j + 1),
            _ => {}
        }
        j += 1;
    }
    b.len()
}

#[cfg(test)]
mod tests {
    use super::static_imports;

    #[test]
    fn finds_static_imports_and_reexports() {
        let text = r#"
            import a from "./a.js";
            import { b, c as d } from './b.js';
            import * as e from "./e.js"
            import "./side.js";
            import f, { g } from "pkg/g";
            import from from "./from.js";
            export { h } from "./h.js";
            export * from "./star.js";
            export * as ns from "./ns.js";
            import/* c */{x}/* d */from/* e */"./comments.js";
        "#;
        assert_eq!(
            static_imports(text),
            [
                "./a.js",
                "./b.js",
                "./e.js",
                "./side.js",
                "pkg/g",
                "./from.js",
                "./h.js",
                "./star.js",
                "./ns.js",
                "./comments.js"
            ]
        );
    }

    #[test]
    fn ignores_what_is_not_a_static_import() {
        let text = r#"
            // import "./line.js";
            /* import "./block.js"; */
            const s = 'import "./string.js"';
            const t = `import "./template.js" ${ `nested ${"}"}` } import "./t2.js"`;
            const r = /import "\.\/regex.js"[/]/g;
            const n = 4 / 2; import "./after-division.js";
            await import("./dynamic.js");
            console.log(import.meta.url, x.import, { import: 1 });
            export const v = 1;
            export default function () {}
            import { y } from "./esc\x2ejs";
        "#;
        assert_eq!(static_imports(text), ["./after-division.js"]);
    }
}
