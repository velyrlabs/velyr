//! V8-format `stack` on error objects: captured at creation and by
//! `Error.captureStackTrace`, bounded by `Error.stackTraceLimit`.

use super::state::Frame;
use super::{ErrorKind, Interpreter, JsResult, object_value};
use crate::bytecode::FunctionKind;
use crate::exception::frame_line;
use crate::{JsStr, ObjectRef, PropertyDescriptor, Value, strict_equals};

pub(crate) const DEFAULT_STACK_TRACE_LIMIT: f64 = 10.0;

impl Interpreter {
    /// Defines `object.stack` as its error header plus the running frames, omitting the
    /// frames above (and including) the topmost call of `skip_through`. Nothing is
    /// installed when `Error.stackTraceLimit` is not a number, like V8.
    pub(crate) fn capture_stack(
        &mut self,
        object: ObjectRef,
        skip_through: Option<&Value>,
    ) -> JsResult<()> {
        let Some(limit) = self.stack_trace_limit()? else {
            return Ok(());
        };
        let mut text = super::intrinsics::describe_error(self, object, &object_value(object))?;
        let frames = &self.state.frames;
        let skip = skip_through
            .and_then(|f| {
                frames
                    .iter()
                    .rev()
                    .position(|frame| strict_equals(&frame.function, f))
            })
            .map_or(0, |index| index + 1);
        for frame in frames.iter().rev().skip(skip).take(limit) {
            text += &describe_frame(frame);
        }
        let stack = Value::String(JsStr::from_str(&text));
        let key = self.realm.key_str("stack");
        self.define_property(
            object,
            key,
            &PropertyDescriptor::data(stack, true, false, true),
        )?;
        Ok(())
    }

    /// `Error.stackTraceLimit` as a frame count; `None` when it is not a number.
    fn stack_trace_limit(&mut self) -> JsResult<Option<usize>> {
        let constructor = self.state.intrinsics.error_constructor;
        let key = self.realm.key_str("stackTraceLimit");
        Ok(
            match self.get(constructor, key, &object_value(constructor))? {
                Value::Number(n) if n.is_nan() || n <= 0.0 => Some(0),
                Value::Number(n) => Some(n.min(usize::MAX as f64) as usize),
                _ => None,
            },
        )
    }

    /// The `stack` an uncaught exception reports, when it is an object with a string one.
    pub fn exception_stack(&mut self, value: &Value) -> Option<String> {
        let object = self.as_object(value)?;
        let key = self.realm.key_str("stack");
        match self.get(object, key, value) {
            Ok(Value::String(stack)) => Some(stack.to_string_lossy()),
            _ => None,
        }
    }
}

fn describe_frame(frame: &Frame) -> String {
    let code = &frame.info.code;
    let location = code
        .script
        .as_ref()
        .zip(code.span_at(frame.pc.saturating_sub(1)))
        .map_or_else(
            || "<anonymous>".to_owned(),
            |(script, (start, _))| {
                let (line, column) = script.location(start);
                format!("{}:{line}:{column}", script.name)
            },
        );
    let is_construct =
        code.kind != FunctionKind::Arrow && !matches!(frame.env.new_target, Value::Undefined);
    frame_line(&code.name.to_string_lossy(), is_construct, &location)
}

/// `Error.captureStackTrace(object, constructorOpt)`.
pub(crate) fn capture_stack_trace(
    vm: &mut Interpreter,
    _: &Value,
    args: &[Value],
) -> JsResult<Value> {
    let target = args.first().cloned().unwrap_or(Value::Undefined);
    let Some(object) = vm.as_object(&target) else {
        return Err(vm.error(
            ErrorKind::Type,
            "Invalid argument: captureStackTrace target",
        ));
    };
    let skip_through = args.get(1).filter(|f| vm.callable(f).is_some());
    vm.capture_stack(object, skip_through)?;
    Ok(Value::Undefined)
}
