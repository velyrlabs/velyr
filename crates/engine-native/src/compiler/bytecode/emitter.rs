//! Instruction buffer, constant pool, labels, registers and line table of one function.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oxc_span::Span;
use velyr_engine_gc::external::{Charge, Kind};
use velyr_engine_vm::JsStr;
use velyr_engine_vm::bytecode::{
    ConstIndex, Constant, FunctionCode, HandlerEntry, Instruction, Reg, RegRange, SourcePosition,
    Target,
};
use velyr_engine_vm::exception::Script;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label(u32);

/// The string of an oxc literal value. With `lone_surrogates`, oxc encodes each lone
/// surrogate as U+FFFD followed by its 4 hex digits (and U+FFFD itself as `\u{FFFD}fffd`).
pub fn literal_str(text: &str, lone_surrogates: bool) -> JsStr {
    if !lone_surrogates {
        return JsStr::from_str(text);
    }
    let mut units = Vec::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{FFFD}' {
            let hex: String = chars.by_ref().take(4).collect();
            units.push(u16::from_str_radix(&hex, 16).unwrap_or(0xFFFD));
        } else {
            let mut buffer = [0u16; 2];
            units.extend_from_slice(c.encode_utf16(&mut buffer));
        }
    }
    JsStr::from_utf16(units)
}

/// Everything about a function that is not its code.
pub struct Header {
    pub name: JsStr,
    pub kind: velyr_engine_vm::bytecode::FunctionKind,
    pub is_strict: bool,
    pub parameter_count: u32,
    pub length: u32,
    pub span: Span,
    pub script: Option<Rc<Script>>,
}

struct PendingHandler {
    start: Label,
    end: Label,
    target: Label,
    context: Option<Reg>,
}

#[derive(Default)]
pub struct Emitter {
    instructions: Vec<Instruction>,
    constants: Vec<Constant>,
    numbers: HashMap<u64, ConstIndex>,
    strings: HashMap<String, ConstIndex>,
    labels: Vec<Option<u32>>,
    handlers: Vec<PendingHandler>,
    positions: Vec<SourcePosition>,
    span: Option<Span>,
    next_register: u32,
    register_count: u32,
    /// Inline caches handed out so far.
    caches: u32,
    /// String constants of the whole compilation (a script or module): the functions of a
    /// module repeat the same property names, and each copy would be its own allocation.
    pub(super) pool: Rc<RefCell<HashMap<Box<str>, JsStr>>>,
    /// Added (wrapping) to AST offsets: a lazy body is parsed out of its script's text.
    pub(super) shift: u32,
}

impl Emitter {
    /// Appends `instruction`; a named property access gets the next inline cache index.
    pub fn emit(&mut self, mut instruction: Instruction) {
        if let Some(cache) = instruction.cache_mut() {
            *cache = self.caches;
            self.caches += 1;
        }
        if let Some(span) = self.span {
            let is_new = self
                .positions
                .last()
                .is_none_or(|last| (last.start, last.end) != (span.start, span.end));
            if is_new {
                self.positions.push(SourcePosition {
                    offset: self.instructions.len() as u32,
                    start: span.start,
                    end: span.end,
                });
            }
        }
        self.instructions.push(instruction);
    }

    /// Following instructions belong to `span` in the line table.
    pub fn position(&mut self, span: Span) {
        self.span = Some(self.shifted(span));
    }

    /// `span` of the AST being compiled as offsets into the script's text.
    pub fn shifted(&self, span: Span) -> Span {
        let start = span.start.wrapping_add(self.shift);
        Span::new(start, span.end.wrapping_add(self.shift))
    }

    pub fn emit_at(&mut self, span: Span, instruction: Instruction) {
        self.position(span);
        self.emit(instruction);
    }

    /// Emits a jump-like instruction; the label is patched to an offset by `finish`.
    pub fn jump(&mut self, make: impl FnOnce(Target) -> Instruction, label: Label) {
        self.emit(make(Target(label.0)));
    }

    pub fn new_label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() as u32 - 1)
    }

    pub fn bind(&mut self, label: Label) {
        self.labels[label.0 as usize] = Some(self.instructions.len() as u32);
    }

    pub fn bound_label(&mut self) -> Label {
        let label = self.new_label();
        self.bind(label);
        label
    }

    pub fn add_handler(&mut self, start: Label, end: Label, target: Label, context: Option<Reg>) {
        self.handlers.push(PendingHandler {
            start,
            end,
            target,
            context,
        });
    }

    pub fn alloc(&mut self) -> Reg {
        self.alloc_range(1).first
    }

    pub fn alloc_range(&mut self, count: u32) -> RegRange {
        let first = Reg(self.next_register);
        self.next_register += count;
        self.register_count = self.register_count.max(self.next_register);
        RegRange { first, count }
    }

    /// Register stack height, to `release` every temporary allocated after it.
    pub fn mark(&self) -> u32 {
        self.next_register
    }

    pub fn release(&mut self, mark: u32) {
        self.next_register = mark;
    }

    pub fn constant(&mut self, constant: Constant) -> ConstIndex {
        self.constants.push(constant);
        ConstIndex(self.constants.len() as u32 - 1)
    }

    /// One `JsStr` per distinct text across every function sharing this pool (see `pool`).
    pub fn intern(&self, text: &str) -> JsStr {
        let mut pool = self.pool.borrow_mut();
        if let Some(s) = pool.get(text) {
            return s.clone();
        }
        let s = JsStr::from_str(text);
        pool.insert(text.into(), s.clone());
        s
    }

    pub fn string(&mut self, text: &str) -> ConstIndex {
        if let Some(&index) = self.strings.get(text) {
            return index;
        }
        let s = self.intern(text);
        let index = self.constant(Constant::String(s));
        self.strings.insert(text.to_owned(), index);
        index
    }

    pub fn number(&mut self, value: f64) -> ConstIndex {
        if let Some(&index) = self.numbers.get(&value.to_bits()) {
            return index;
        }
        let index = self.constant(Constant::Number(value));
        self.numbers.insert(value.to_bits(), index);
        index
    }

    pub fn load_number(&mut self, value: f64) {
        let small = value as i32;
        let is_small = f64::from(small) == value && !(value == 0.0 && value.is_sign_negative());
        let instruction = if is_small {
            Instruction::LdaSmi(small)
        } else {
            Instruction::LdaConstant(self.number(value))
        };
        self.emit(instruction);
    }

    pub fn load_string(&mut self, text: &str) {
        let index = self.string(text);
        self.emit(Instruction::LdaConstant(index));
    }

    /// Loads an oxc literal value; `lone_surrogates` marks oxc's `\u{FFFD}XXXX` escapes.
    pub fn load_literal(&mut self, text: &str, lone_surrogates: bool) {
        if !lone_surrogates {
            return self.load_string(text);
        }
        let index = self.constant(Constant::String(literal_str(text, true)));
        self.emit(Instruction::LdaConstant(index));
    }

    pub fn finish(mut self, header: Header) -> FunctionCode {
        let labels = std::mem::take(&mut self.labels);
        let resolve = |label: Label| Target(labels[label.0 as usize].expect("label bound"));
        for instruction in &mut self.instructions {
            if let Some(target) = instruction.target_mut() {
                *target = resolve(Label(target.0));
            }
        }
        let handlers = self
            .handlers
            .iter()
            .map(|h| HandlerEntry {
                start: resolve(h.start),
                end: resolve(h.end),
                target: resolve(h.target),
                context: h.context,
            })
            .collect();
        // Code lives as long as its module: drop the growth slack (up to half of each Vec).
        self.instructions.shrink_to_fit();
        self.constants.shrink_to_fit();
        self.positions.shrink_to_fit();
        let span = self.shifted(header.span);
        let mut code = FunctionCode {
            name: header.name,
            kind: header.kind,
            is_strict: header.is_strict,
            parameter_count: header.parameter_count,
            length: header.length,
            register_count: self.register_count,
            instructions: self.instructions,
            constants: self.constants,
            handlers,
            positions: self.positions,
            source_start: span.start,
            source_end: span.end,
            script: header.script,
            charge: Charge::new(Kind::Code, 0),
            lazy: None,
        };
        let bytes = code.owned_bytes();
        code.charge.set(bytes);
        code
    }
}
