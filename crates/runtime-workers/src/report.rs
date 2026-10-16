//! What a worker reports to its parent's `error` event, with the source location when known.

/// Message plus the position the error was thrown from (`ErrorEvent` fields).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ErrorReport {
    pub(crate) message: String,
    pub(crate) filename: String,
    pub(crate) lineno: u32,
    pub(crate) colno: u32,
    /// The quota class (`heap`, `cpu`) that stopped the worker, when one did.
    pub(crate) quota: Option<&'static str>,
}

impl ErrorReport {
    pub(crate) fn new(message: String, stack: Option<&str>) -> Self {
        let (filename, lineno, colno) = stack.and_then(location).unwrap_or_default();
        Self {
            message,
            filename,
            lineno,
            colno,
            quota: None,
        }
    }

    pub(crate) fn quota(message: String, class: &'static str) -> Self {
        Self {
            quota: Some(class),
            ..Self::new(message, None)
        }
    }
}

/// First `file:///path:line:col` of a stack. shortcut: assumes the V8 frame format (the engine
/// contract says stacks are opaque); frames elsewhere give an empty location, never an error.
pub(crate) fn location(stack: &str) -> Option<(String, u32, u32)> {
    stack.lines().find_map(|line| {
        let start = line.find("file://")?;
        let frame = line[start..].trim_end_matches(')');
        let (rest, col) = frame.rsplit_once(':')?;
        let (file, row) = rest.rsplit_once(':')?;
        Some((file.to_owned(), row.parse().ok()?, col.parse().ok()?))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_first_file_frame() {
        let stack = "Error: boom\n    at f (file:///a/b.js:3:9)\n    at file:///a/c.js:1:1";
        assert_eq!(location(stack), Some(("file:///a/b.js".into(), 3, 9)));
        assert_eq!(location("Error: x\n    at velyr:boot:1:2"), None);
    }
}
