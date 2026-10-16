//! Classes: ClassDefinitionEvaluation around `CreateClass`. Methods and accessors are
//! defined in source order, instance fields and private methods are recorded on the class
//! (`AddInstanceElement`), and static fields/blocks run, as closures called with the class
//! as `this`, after the class binding is initialized. Private names live in the class
//! scope's context, one fresh name per evaluation.
//! shortcut: inside a class declaration the class name resolves to the outer binding
//! (Oxc has no inner binding); it is initialized before static elements run, so the only
//! difference is that reassigning the outer name is visible to the class body.

use std::rc::Rc;

use oxc_ast::AstKind;
use oxc_ast::ast::{
    Class, ClassElement, MethodDefinition, MethodDefinitionKind, PropertyDefinition, PropertyKey,
};
use oxc_semantic::AstNodes;
use oxc_span::Span;
use velyr_engine_vm::JsStr;
use velyr_engine_vm::bytecode::{Constant, FunctionCode, FunctionKind, Instruction as I, Reg};
use velyr_engine_vm::exception::Script;

use super::emitter::{Emitter, Header};
use super::function::{FunctionCompiler, uses_super_property};
use super::literal::static_key;
use super::scope::ClassInfo;
use super::unsupported;
use crate::compiler::JsResult;

/// Every class of the script with the private names it declares.
pub fn collect_classes(nodes: &AstNodes) -> Vec<ClassInfo> {
    nodes
        .iter()
        .filter_map(|node| match node.kind() {
            AstKind::Class(class) => Some(ClassInfo {
                scope: class.scope_id(),
                private_names: private_names(class),
            }),
            _ => None,
        })
        .collect()
}

fn private_names(class: &Class) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for element in &class.body.body {
        let key = match element {
            ClassElement::MethodDefinition(method) => &method.key,
            ClassElement::PropertyDefinition(field) => &field.key,
            _ => continue,
        };
        if let PropertyKey::PrivateIdentifier(id) = key
            && !names.iter().any(|name| name == id.name.as_str())
        {
            names.push(id.name.to_string());
        }
    }
    names
}

/// Static field or block, run after the class binding exists.
struct StaticElement {
    /// Field key register; `None` for a static block.
    key: Option<Reg>,
    /// Closure register; `None` for a field without initializer.
    closure: Option<Reg>,
}

impl FunctionCompiler<'_> {
    /// Leaves the class constructor in `acc`; `name` names an anonymous class.
    pub fn class(&mut self, class: &Class, name: &str) -> JsResult<()> {
        if !class.decorators.is_empty() {
            return Err(unsupported("decorator"));
        }
        let mark = self.e.mark();
        let saved = self.enter_scope(class.scope_id());
        self.classes.push(class.scope_id());
        let result = self.class_definition(class, name);
        self.classes.pop();
        self.exit_scope(saved);
        self.e.release(mark);
        result
    }

    fn class_definition(&mut self, class: &Class, name: &str) -> JsResult<()> {
        for slot in self.analysis.private_names(class.scope_id()) {
            self.e.emit(I::CreatePrivateName);
            self.e.emit(I::StaContextSlot { depth: 0, slot });
        }
        let registers = self.e.alloc_range(2);
        let (constructor, proto) = (registers.first, Reg(registers.first.0 + 1));
        match &class.heritage {
            Some(heritage) => self.expression(&heritage.expression)?,
            None => self.e.emit(I::LdaTheHole),
        }
        self.e.emit(I::Star(constructor));
        let name = class.id.as_ref().map_or(name, |id| id.name.as_str());
        let code = self.constructor_code(class, name)?;
        let index = self.e.constant(Constant::Function(Rc::new(code)));
        self.e
            .emit_at(class.span, I::CreateClass(index, constructor));
        let mut statics = Vec::new();
        for element in &class.body.body {
            let mark = self.e.mark();
            if !self.class_element(element, constructor, proto, &mut statics)? {
                self.e.release(mark);
            }
        }
        if let Some(id) = &class.id {
            let symbol = id.symbol_id();
            let is_referenced = !self
                .analysis
                .scoping
                .get_resolved_reference_ids(symbol)
                .is_empty();
            if class.is_declaration() || is_referenced {
                self.e.emit(I::Ldar(constructor));
                self.initialize_binding(symbol)?;
            }
        }
        for element in statics {
            match element.closure {
                Some(callee) => {
                    let args = self.e.alloc_range(0);
                    let this = constructor;
                    self.e.emit(I::Call { callee, this, args });
                }
                None => self.e.emit(I::LdaUndefined),
            }
            if let Some(key) = element.key {
                let field = FunctionKind::ClassField;
                self.e.emit(I::DefineElement(constructor, key, field));
            }
        }
        self.e.emit(I::Ldar(constructor));
        Ok(())
    }

    fn constructor_code(&mut self, class: &Class, name: &str) -> JsResult<FunctionCode> {
        let kind = match class.heritage {
            Some(_) => FunctionKind::DerivedConstructor,
            None => FunctionKind::ClassConstructor,
        };
        let explicit = class.body.body.iter().find_map(|element| match element {
            ClassElement::MethodDefinition(method)
                if method.kind == MethodDefinitionKind::Constructor =>
            {
                Some(method)
            }
            _ => None,
        });
        match explicit {
            Some(method) => self.function_code(&method.value, name, kind, Some(class.span)),
            None => Ok(default_constructor(kind, name, class.span, &self.script)),
        }
    }

    /// Compiles one element; returns whether its registers must outlive it (statics).
    fn class_element(
        &mut self,
        element: &ClassElement,
        constructor: Reg,
        proto: Reg,
        statics: &mut Vec<StaticElement>,
    ) -> JsResult<bool> {
        match element {
            ClassElement::MethodDefinition(method) => {
                if method.kind != MethodDefinitionKind::Constructor {
                    self.class_method(method, constructor, proto)?;
                }
                Ok(false)
            }
            ClassElement::PropertyDefinition(field) => {
                self.class_field(field, constructor, proto, statics)
            }
            ClassElement::StaticBlock(block) => {
                let code = self.block_code(block.scope_id(), &block.body, block.span)?;
                let closure = self.element_closure(code, constructor);
                statics.push(StaticElement {
                    key: None,
                    closure: Some(closure),
                });
                Ok(true)
            }
            ClassElement::AccessorProperty(_) => Err(unsupported("auto-accessor")),
            ClassElement::TSIndexSignature(_) => Err(unsupported("TypeScript syntax")),
        }
    }

    fn class_method(
        &mut self,
        method: &MethodDefinition,
        constructor: Reg,
        proto: Reg,
    ) -> JsResult<()> {
        let target = if method.r#static { constructor } else { proto };
        let key = self.element_key(&method.key, method.computed)?;
        let kind = match method.kind {
            MethodDefinitionKind::Get => FunctionKind::Getter,
            MethodDefinitionKind::Set => FunctionKind::Setter,
            _ => FunctionKind::Method,
        };
        let is_private = matches!(method.key, PropertyKey::PrivateIdentifier(_));
        let name = match (&method.key, kind) {
            (PropertyKey::PrivateIdentifier(id), _) => format!("#{}", id.name),
            (key, FunctionKind::Method) => static_key(key, method.computed)?.unwrap_or_default(),
            _ => String::new(),
        };
        let source = self.method_source(method);
        if self.method_closure(&method.value, &name, kind, Some(source))? {
            self.e.emit(I::SetHomeObject(target));
        }
        let instruction = match is_private && !method.r#static {
            true => I::AddInstanceElement(constructor, key, kind),
            false => I::DefineElement(target, key, kind),
        };
        self.e.emit_at(method.span, instruction);
        Ok(())
    }

    /// `toString` span of a method: its definition without a leading `static` keyword.
    fn method_source(&self, method: &MethodDefinition) -> Span {
        let span = method.span;
        if !method.r#static {
            return span;
        }
        let text = self.script.slice(span.start, span.end);
        let mut rest = text.strip_prefix("static").unwrap_or(text);
        loop {
            let trimmed = rest.trim_start();
            rest = if let Some(after) = trimmed.strip_prefix("//") {
                after
                    .find(['\n', '\r', '\u{2028}', '\u{2029}'])
                    .map_or("", |i| &after[i..])
            } else if let Some(after) = trimmed.strip_prefix("/*") {
                after.find("*/").map_or("", |i| &after[i + 2..])
            } else {
                break Span::new(span.end - trimmed.len() as u32, span.end);
            };
        }
    }

    fn class_field(
        &mut self,
        field: &PropertyDefinition,
        constructor: Reg,
        proto: Reg,
        statics: &mut Vec<StaticElement>,
    ) -> JsResult<bool> {
        let key = self.element_key(&field.key, field.computed)?;
        let home = if field.r#static { constructor } else { proto };
        let closure = match &field.value {
            Some(value) => {
                let name = match &field.key {
                    PropertyKey::PrivateIdentifier(id) => Some(format!("#{}", id.name)),
                    key => static_key(key, field.computed)?,
                };
                let class_scope = *self.classes.last().expect("fields are inside a class");
                let code = self.initializer_code(class_scope, value, name.as_deref())?;
                Some(self.element_closure(code, home))
            }
            None => None,
        };
        if field.r#static {
            statics.push(StaticElement {
                key: Some(key),
                closure,
            });
            return Ok(true);
        }
        match closure {
            Some(closure) => self.e.emit(I::Ldar(closure)),
            None => self.e.emit(I::LdaUndefined),
        }
        let kind = FunctionKind::ClassField;
        self.e
            .emit_at(field.span, I::AddInstanceElement(constructor, key, kind));
        Ok(false)
    }

    /// `CreateClosure` for an element's code (with `home` as [[HomeObject]] when it uses
    /// `super`) into a fresh register.
    fn element_closure(&mut self, code: FunctionCode, home: Reg) -> Reg {
        let needs_home = uses_super_property(&code);
        let index = self.e.constant(Constant::Function(Rc::new(code)));
        self.e.emit(I::CreateClosure(index));
        if needs_home {
            self.e.emit(I::SetHomeObject(home));
        }
        let register = self.e.alloc();
        self.e.emit(I::Star(register));
        register
    }

    /// The element key (string, private name or ToPropertyKey'd computed value) in a
    /// register.
    fn element_key(&mut self, key: &PropertyKey, computed: bool) -> JsResult<Reg> {
        if let PropertyKey::PrivateIdentifier(id) = key {
            self.load_private_name(&id.name)?;
        } else if let Some(name) = static_key(key, computed)? {
            self.e.load_string(&name);
        } else {
            return self.computed_key(key);
        }
        let register = self.e.alloc();
        self.e.emit(I::Star(register));
        Ok(register)
    }

    /// Loads the private name `#name` of the innermost class declaring it.
    pub fn load_private_name(&mut self, name: &str) -> JsResult<()> {
        for index in (0..self.classes.len()).rev() {
            let class = self.classes[index];
            if let Some(slot) = self.analysis.private_slot(class, name) {
                let depth = self.context_depth(class)?;
                self.e.emit(I::LdaContextSlot { depth, slot });
                return Ok(());
            }
        }
        Err(unsupported("undeclared private name"))
    }
}

/// `constructor() {}`, or `constructor(...args) { super(...args); }` without touching the
/// array iterator.
fn default_constructor(
    kind: FunctionKind,
    name: &str,
    span: Span,
    script: &Rc<Script>,
) -> FunctionCode {
    let mut e = Emitter::default();
    if kind == FunctionKind::DerivedConstructor {
        let args = e.alloc();
        e.emit(I::CreateRestParameter(0));
        e.emit(I::Star(args));
        e.emit(I::SuperCall(args));
    }
    e.emit(I::LdaUndefined);
    e.emit(I::Return);
    e.finish(Header {
        name: JsStr::from_str(name),
        kind,
        is_strict: true,
        parameter_count: 0,
        length: 0,
        span,
        script: Some(script.clone()),
    })
}
