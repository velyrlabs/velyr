//! Source locations and frame lines for V8-format stack traces
//! (`    at name (file:line:column)`).

/// The script a `FunctionCode` was compiled from: its name, text and line starts.
#[derive(Debug)]
pub struct Script {
    pub name: String,
    /// Byte offset of every line start; the first is 0.
    line_starts: Vec<u32>,
    /// Source text, for `Function.prototype.toString`.
    text: Box<str>,
}

impl Script {
    pub fn new(name: &str, text: &str) -> Self {
        let breaks = text.match_indices('\n').map(|(index, _)| index as u32 + 1);
        Self {
            name: name.to_owned(),
            line_starts: std::iter::once(0).chain(breaks).collect(),
            text: text.into(),
        }
    }

    /// The source text between two byte offsets (empty when out of range).
    pub fn slice(&self, start: u32, end: u32) -> &str {
        self.text.get(start as usize..end as usize).unwrap_or("")
    }

    /// 1-based line and column of a byte offset.
    /// shortcut: columns count UTF-8 bytes, V8 counts UTF-16 units; they differ only on
    /// non-ASCII lines, upgrade when a stack consumer needs exact columns there.
    pub fn location(&self, offset: u32) -> (u32, u32) {
        let line = self.line_starts.partition_point(|&start| start <= offset);
        let start = self.line_starts[line - 1];
        (line as u32, offset - start + 1)
    }
}

/// One stack line: `at new name (file:line:col)`, or `at file:line:col` for anonymous
/// functions and script code.
pub(crate) fn frame_line(name: &str, is_construct: bool, location: &str) -> String {
    match (name.is_empty(), is_construct) {
        (true, false) => format!("\n    at {location}"),
        (true, true) => format!("\n    at new <anonymous> ({location})"),
        (false, false) => format!("\n    at {name} ({location})"),
        (false, true) => format!("\n    at new {name} ({location})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_are_one_based() {
        let script = Script::new("a.js", "ab\ncd\n");
        assert_eq!(script.location(0), (1, 1));
        assert_eq!(script.location(1), (1, 2));
        assert_eq!(script.location(3), (2, 1));
        assert_eq!(script.location(6), (3, 1));
    }
}
