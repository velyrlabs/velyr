use std::io::Write;

use velyr_engine_api::{CallContext, HostError, HostFunctionDef, Value};

pub const OP_CONSOLE_WRITE: HostFunctionDef = HostFunctionDef {
    name: "op_console_write",
    arity: 2,
    callback: op_console_write,
};

/// `op_console_write(level, text)`: stdout for log/info/debug, stderr for warn/error.
fn op_console_write(_: &mut dyn CallContext, args: &[Value]) -> Result<Value, HostError> {
    let [Value::String(level), Value::String(text)] = args else {
        return Err(HostError::TypeError(
            "op_console_write expects (level: string, text: string)".into(),
        ));
    };
    let level = level.to_string_lossy();
    let text = text.to_string_lossy();
    match level.as_str() {
        "log" | "info" | "debug" => write_line(std::io::stdout().lock(), &text),
        "warn" | "error" => write_line(std::io::stderr().lock(), &text),
        other => {
            return Err(HostError::TypeError(format!(
                "unknown console level `{other}`"
            )));
        }
    }
    Ok(Value::Undefined)
}

/// Output failures (closed pipe) must not turn into JS exceptions.
fn write_line(mut out: impl Write, text: &str) {
    let _ = writeln!(out, "{text}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_line_appends_newline() {
        let mut buf = Vec::new();
        write_line(&mut buf, "hi");
        assert_eq!(buf, b"hi\n");
    }
}
