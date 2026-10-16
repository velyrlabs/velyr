//! The compiled function: constants, handler table, line table and the disassembly used by
//! snapshot tests. Re-exported from `bytecode`.

use std::fmt;
use std::rc::Rc;

use velyr_engine_api::JsBigInt;
use velyr_engine_gc::external::Charge;

use super::{ConstIndex, FunctionKind, Instruction, Reg, RegRange, Target};
use crate::string::JsStr;

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateObject {
    pub cooked: Vec<Option<JsStr>>,
    pub raw: Vec<JsStr>,
}

/// Script-level bindings, declared in one step so conflicts throw before any is created.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GlobalDeclarations {
    pub functions: Vec<JsStr>,
    pub vars: Vec<JsStr>,
    pub lets: Vec<JsStr>,
    pub consts: Vec<JsStr>,
}

#[derive(Clone)]
pub enum Constant {
    Number(f64),
    String(JsStr),
    BigInt(Rc<JsBigInt>),
    Function(Rc<FunctionCode>),
    TemplateObject(Rc<TemplateObject>),
    GlobalDeclarations(Rc<GlobalDeclarations>),
}

impl Constant {
    /// From the base-10 digits Oxc stores for a BigInt literal.
    pub fn bigint_from_decimal(digits: &str) -> Option<Constant> {
        crate::bigint::parse(digits).map(|value| Constant::BigInt(Rc::new(value)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandlerEntry {
    pub start: Target,
    pub end: Target,
    pub target: Target,
    /// Register holding the context to restore before jumping to `target`.
    pub context: Option<Reg>,
}

/// Instructions from `offset` on belong to the source range `start..end` (byte offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    pub offset: u32,
    pub start: u32,
    pub end: u32,
}

/// The body of a lazily compiled function, compiled by its engine on the first call.
pub trait LazyBody {
    /// The complete code of `stub` (same header); `Err` is a SyntaxError message.
    fn compile(&self, stub: &FunctionCode) -> Result<FunctionCode, String>;
    /// Generator or async kind, needed before the body exists (prototype setup).
    fn coroutine(&self) -> Option<super::CoroutineKind>;
}

#[derive(Clone)]
pub struct FunctionCode {
    pub name: JsStr,
    pub kind: FunctionKind,
    pub is_strict: bool,
    pub parameter_count: u32,
    /// The `length` property: parameters before the first default or rest.
    pub length: u32,
    pub register_count: u32,
    pub instructions: Vec<Instruction>,
    pub constants: Vec<Constant>,
    pub handlers: Vec<HandlerEntry>,
    pub positions: Vec<SourcePosition>,
    /// Source range of the whole function, for stack traces and `toString`.
    pub source_start: u32,
    pub source_end: u32,
    /// Where the code came from, for stack traces; `None` for synthesized code.
    pub script: Option<Rc<crate::exception::Script>>,
    /// `Kind::Code` bytes for the memory breakdown; the compiler sets it from `owned_bytes`.
    pub charge: Charge,
    /// Set on a stub: header only, the body compiles on the first call.
    pub lazy: Option<Rc<dyn LazyBody>>,
}

impl FunctionCode {
    /// The compiled code: `self`, or a lazy stub's body compiled now.
    pub fn compiled(&self) -> Result<std::borrow::Cow<'_, FunctionCode>, String> {
        match &self.lazy {
            Some(lazy) => lazy.compile(self).map(std::borrow::Cow::Owned),
            None => Ok(std::borrow::Cow::Borrowed(self)),
        }
    }

    /// Bytes this code owns besides its strings and nested functions (charged on their own).
    pub fn owned_bytes(&self) -> usize {
        size_of::<Self>()
            + self.instructions.capacity() * size_of::<Instruction>()
            + self.constants.capacity() * size_of::<Constant>()
            + self.handlers.capacity() * size_of::<HandlerEntry>()
            + self.positions.capacity() * size_of::<SourcePosition>()
    }

    /// Source range of the instruction at `offset` (line table lookup).
    pub fn span_at(&self, offset: u32) -> Option<(u32, u32)> {
        let index = self.positions.partition_point(|p| p.offset <= offset);
        let position = self.positions.get(index.checked_sub(1)?)?;
        Some((position.start, position.end))
    }
}

impl fmt::Debug for Reg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

impl fmt::Debug for ConstIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "c{}", self.0)
    }
}

impl fmt::Debug for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

impl fmt::Debug for RegRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.count {
            0 => write!(f, "[]"),
            1 => write!(f, "[r{}]", self.first.0),
            n => write!(f, "[r{}..r{}]", self.first.0, self.first.0 + n - 1),
        }
    }
}

impl fmt::Debug for Constant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Constant::Number(n) => write!(f, "{n:?}"),
            Constant::String(s) => write!(f, "{:?}", s.to_string_lossy()),
            Constant::BigInt(n) => write!(f, "{}n", crate::bigint::to_decimal_string(n)),
            Constant::Function(code) => write!(f, "function {:?}", code.name.to_string_lossy()),
            Constant::TemplateObject(t) => {
                let raw: Vec<String> = t.raw.iter().map(JsStr::to_string_lossy).collect();
                write!(f, "template {raw:?}")
            }
            Constant::GlobalDeclarations(g) => {
                let names = |list: &[JsStr]| -> Vec<String> {
                    list.iter().map(JsStr::to_string_lossy).collect()
                };
                write!(
                    f,
                    "globals functions={:?} vars={:?} lets={:?} consts={:?}",
                    names(&g.functions),
                    names(&g.vars),
                    names(&g.lets),
                    names(&g.consts)
                )
            }
        }
    }
}

/// Disassembly used by snapshot tests; nested functions follow their parent.
impl fmt::Display for FunctionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.lazy.is_some() {
            return match self.compiled() {
                Ok(code) => write!(f, "{code}"),
                Err(message) => writeln!(f, "== lazy body fails: {message}"),
            };
        }
        let mode = if self.is_strict { "strict" } else { "sloppy" };
        writeln!(
            f,
            "== {:?} {:?} {mode} params={} length={} registers={}",
            self.name.to_string_lossy(),
            self.kind,
            self.parameter_count,
            self.length,
            self.register_count
        )?;
        for (offset, instruction) in self.instructions.iter().enumerate() {
            writeln!(f, "{offset:>4}: {instruction:?}")?;
        }
        for h in &self.handlers {
            writeln!(
                f,
                "handler [{:?}, {:?}) -> {:?} context={:?}",
                h.start, h.end, h.target, h.context
            )?;
        }
        for (index, constant) in self.constants.iter().enumerate() {
            writeln!(f, "c{index}: {constant:?}")?;
        }
        for constant in &self.constants {
            if let Constant::Function(code) = constant {
                write!(f, "{code}")?;
            }
        }
        Ok(())
    }
}
