//! Module code (VELYR-110): parses a module, collects its import/export entries and
//! compiles its two functions, `init` (InitializeEnvironment: `var`s and hoisted
//! functions, run at link time) and `body`. Top-level bindings live in the module
//! context (see `ScopeAnalysis::module`); a body using top-level await is an async function.
//!
//! Compiled as a child of the bytecode compiler (`bytecode_compiler::module`).

use std::collections::HashMap;
use std::rc::Rc;

use oxc_ast::ast::{
    Declaration, ExportDefaultDeclarationKind, Function, ImportDeclarationSpecifier,
    ModuleExportName, Program, Statement,
};
use oxc_semantic::{SymbolFlags, SymbolId};
use oxc_span::GetSpan;
use velyr_engine_vm::JsStr;
use velyr_engine_vm::bytecode::{CoroutineKind, FunctionCode, FunctionKind, Instruction as I};
use velyr_engine_vm::exception::Script;
use velyr_engine_vm::interpreter::{ExportTarget, ImportName, ModuleCode, ModuleImport};

use super::function::FunctionCompiler;
use super::scope::{self, DEFAULT_SLOT, Location, META_SLOT, ScopeAnalysis};
use super::unsupported;
use super::using::using_kind;
use crate::compiler::{JsResult, throw};

pub fn compile_module(name: &str, text: &str) -> JsResult<ModuleCode> {
    super::analyze(text, true, true, |program, _, analysis| {
        module_code(name, text, program, analysis)
    })
}

fn module_code(
    name: &str,
    text: &str,
    program: &Program,
    analysis: &ScopeAnalysis,
) -> JsResult<ModuleCode> {
    let script = Rc::new(Script::new(name, text));
    let entries = Entries::collect(program, analysis)?;
    let init = init_code(program, analysis, script.clone())?;
    // The body is an async function only if it awaits at the top level.
    let body = match body_code(program, analysis, script.clone(), None) {
        Err(error) if error.message == unsupported(AWAIT_OUTSIDE_ASYNC).message => {
            body_code(program, analysis, script, Some(CoroutineKind::Async))?
        }
        result => result?,
    };
    Ok(ModuleCode {
        url: name.to_owned(),
        init: Rc::new(init),
        body: Rc::new(body),
        context_size: analysis.context_size(program.scope_id()),
        requests: entries.requests,
        imports: entries.imports,
        exports: entries.exports,
        star_exports: entries.star_exports,
    })
}

/// Error of `emit_await` outside an async function.
const AWAIT_OUTSIDE_ASYNC: &str = "await outside an async function";

#[derive(Default)]
struct Entries {
    requests: Vec<String>,
    imports: Vec<ModuleImport>,
    exports: Vec<(JsStr, ExportTarget)>,
    star_exports: Vec<u32>,
    /// Request and import name of each import binding, for `export { imported }`.
    imported: HashMap<SymbolId, (u32, ImportName)>,
}

fn export_name(name: &ModuleExportName) -> JsStr {
    JsStr::from_str(name.name().as_str())
}

fn context_slot(analysis: &ScopeAnalysis, symbol: SymbolId) -> JsResult<u32> {
    match analysis.location(symbol) {
        Location::Context { slot, .. } | Location::Import { slot } => Ok(slot),
        _ => Err(throw(
            "SyntaxError",
            "internal compiler error: module binding",
        )),
    }
}

impl Entries {
    fn collect(program: &Program, analysis: &ScopeAnalysis) -> JsResult<Self> {
        let mut entries = Self::default();
        for statement in &program.body {
            entries.statement(statement, analysis)?;
        }
        // `export { x }` may name an import declared later in the module.
        for statement in &program.body {
            if let Statement::ExportNamedDeclaration(d) = statement {
                for specifier in &d.specifiers {
                    let symbol = match &specifier.local {
                        ModuleExportName::IdentifierReference(r) => {
                            analysis.symbol_of(r.reference_id())
                        }
                        _ => None,
                    };
                    let Some(symbol) = symbol else {
                        let local = specifier.local.name();
                        let message = format!("Export '{local}' is not defined in module");
                        return Err(throw("SyntaxError", message));
                    };
                    let target = match entries.imported.get(&symbol) {
                        Some((request, name)) => ExportTarget::Indirect {
                            request: *request,
                            name: name.clone(),
                        },
                        None => ExportTarget::Local(context_slot(analysis, symbol)?),
                    };
                    entries
                        .exports
                        .push((export_name(&specifier.exported), target));
                }
            }
        }
        Ok(entries)
    }

    fn request(&mut self, specifier: &str) -> u32 {
        let index = self.requests.iter().position(|r| r == specifier);
        let index = index.unwrap_or_else(|| {
            self.requests.push(specifier.to_owned());
            self.requests.len() - 1
        });
        index as u32
    }

    fn statement(&mut self, statement: &Statement, analysis: &ScopeAnalysis) -> JsResult<()> {
        match statement {
            Statement::ImportDeclaration(d) => {
                let request = self.request(d.source.value.as_str());
                for specifier in d.specifiers.iter().flatten() {
                    let (name, local) = match specifier {
                        ImportDeclarationSpecifier::ImportSpecifier(s) => {
                            (ImportName::Named(export_name(&s.imported)), &s.local)
                        }
                        ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                            (ImportName::Named(JsStr::from_str("default")), &s.local)
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                            (ImportName::Namespace, &s.local)
                        }
                    };
                    let symbol = local.symbol_id();
                    let slot = context_slot(analysis, symbol)?;
                    self.imported.insert(symbol, (request, name.clone()));
                    self.imports.push(ModuleImport {
                        request,
                        name,
                        slot,
                    });
                }
            }
            Statement::ExportFromDeclaration(d) => {
                let request = self.request(d.source.value.as_str());
                for specifier in &d.specifiers {
                    let name = ImportName::Named(export_name(&specifier.local));
                    let target = ExportTarget::Indirect { request, name };
                    self.exports
                        .push((export_name(&specifier.exported), target));
                }
            }
            Statement::ExportDeclaration(d) => {
                let span = d.declaration.span();
                for symbol in analysis.bindings(analysis.scoping.root_scope_id()) {
                    let start = analysis.scoping.symbol_span(symbol).start;
                    if span.start <= start && start < span.end {
                        let name = JsStr::from_str(analysis.name(symbol));
                        let slot = context_slot(analysis, symbol)?;
                        self.exports.push((name, ExportTarget::Local(slot)));
                    }
                }
            }
            Statement::ExportDefaultDeclaration(d) => {
                let id = match &d.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(f) => f.id.as_ref(),
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => c.id.as_ref(),
                    _ => None,
                };
                let slot = match id {
                    Some(id) => context_slot(analysis, id.symbol_id())?,
                    None => DEFAULT_SLOT,
                };
                let target = ExportTarget::Local(slot);
                self.exports.push((JsStr::from_str("default"), target));
            }
            Statement::ExportAllDeclaration(d) => {
                let request = self.request(d.source.value.as_str());
                match &d.exported {
                    None => self.star_exports.push(request),
                    Some(name) => {
                        let target = ExportTarget::Indirect {
                            request,
                            name: ImportName::Namespace,
                        };
                        self.exports.push((export_name(name), target));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Function declarations of the module top level, exported or not.
fn hoisted_function<'b, 'a>(statement: &'b Statement<'a>) -> Option<&'b Function<'a>> {
    match statement {
        Statement::FunctionDeclaration(f) => Some(&**f),
        Statement::ExportDeclaration(d) => match &d.declaration {
            Declaration::FunctionDeclaration(f) => Some(&**f),
            _ => None,
        },
        Statement::ExportDefaultDeclaration(d) => match &d.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(f) => Some(&**f),
            _ => None,
        },
        _ => None,
    }
}

fn module_compiler<'s>(
    program: &Program,
    analysis: &'s ScopeAnalysis<'s>,
    script: Rc<Script>,
) -> FunctionCompiler<'s> {
    let root = program.scope_id();
    let mut compiler = FunctionCompiler::new(analysis, root, FunctionKind::Script, script);
    // The module context is created with the record and is current when the code runs.
    compiler.contexts.push(root);
    compiler
}

/// InitializeEnvironment: `var` bindings start as `undefined`, functions are created.
fn init_code(
    program: &Program,
    analysis: &ScopeAnalysis,
    script: Rc<Script>,
) -> JsResult<FunctionCode> {
    let mut compiler = module_compiler(program, analysis, script);
    for symbol in analysis.bindings(program.scope_id()) {
        let flags = analysis.flags(symbol);
        let is_var = !scope::is_lexical(flags)
            && !flags.intersects(SymbolFlags::Function | SymbolFlags::Import);
        if let (true, Location::Context { slot, .. }) = (is_var, analysis.location(symbol)) {
            compiler.e.emit(I::LdaUndefined);
            compiler.e.emit(I::StaContextSlot { depth: 0, slot });
        }
    }
    for function in program.body.iter().filter_map(hoisted_function) {
        match &function.id {
            Some(id) => {
                compiler.closure(function, id.name.as_str(), FunctionKind::Normal)?;
                compiler.initialize_binding(id.symbol_id())?;
            }
            None => {
                compiler.closure(function, "default", FunctionKind::Normal)?;
                compiler.e.emit(I::StaContextSlot {
                    depth: 0,
                    slot: DEFAULT_SLOT,
                });
            }
        }
    }
    compiler.e.emit(I::LdaUndefined);
    compiler.e.emit(I::Return);
    let header = compiler.header("", 0, 0, program.span);
    Ok(compiler.e.finish(header))
}

fn body_code(
    program: &Program,
    analysis: &ScopeAnalysis,
    script: Rc<Script>,
    coroutine: Option<CoroutineKind>,
) -> JsResult<FunctionCode> {
    let mut compiler = module_compiler(program, analysis, script);
    compiler.assign_local_registers();
    let async_body = compiler.start_coroutine(coroutine, true);
    let body = |this: &mut FunctionCompiler| {
        let mut statements = program.body.iter();
        statements.try_for_each(|statement| this.module_statement(statement))
    };
    match using_kind(&program.body) {
        Some(is_async) => compiler.dispose_scope(is_async, body)?,
        None => body(&mut compiler)?,
    }
    compiler.e.emit(I::LdaUndefined);
    compiler.e.emit(I::Return);
    compiler.finish_async_function(async_body);
    let header = compiler.header("", 0, 0, program.span);
    Ok(compiler.e.finish(header))
}

impl FunctionCompiler<'_> {
    /// `import.meta`: slot `META_SLOT` of the module context.
    pub fn import_meta(&mut self) -> JsResult<()> {
        if let Some(depth) = self.outer_module() {
            let slot = META_SLOT;
            self.e.emit(I::LdaContextSlot { depth, slot });
            return Ok(());
        }
        if !self.analysis.module {
            return Err(unsupported("import.meta outside a module"));
        }
        let depth = self.context_depth(self.analysis.scoping.root_scope_id())?;
        self.e.emit(I::LdaContextSlot {
            depth,
            slot: META_SLOT,
        });
        Ok(())
    }

    fn module_statement(&mut self, statement: &Statement) -> JsResult<()> {
        let mark = self.e.mark();
        self.e.position(statement.span());
        let result = match statement {
            Statement::ExportDeclaration(d) => match &d.declaration {
                Declaration::VariableDeclaration(v) => self.variable_declaration(v),
                Declaration::ClassDeclaration(class) => self.class(class, ""),
                // Functions are hoisted by `init`; the rest is TypeScript.
                _ => Ok(()),
            },
            Statement::ExportDefaultDeclaration(d) => self.export_default(&d.declaration),
            Statement::ImportDeclaration(_)
            | Statement::ExportNamedDeclaration(_)
            | Statement::ExportFromDeclaration(_)
            | Statement::ExportAllDeclaration(_) => Ok(()),
            other => self.statement(other),
        };
        self.e.release(mark);
        result
    }

    fn export_default(&mut self, declaration: &ExportDefaultDeclarationKind) -> JsResult<()> {
        let store = I::StaContextSlot {
            depth: 0,
            slot: DEFAULT_SLOT,
        };
        match declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(_) => {}
            ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                self.class(class, "default")?;
                if class.id.is_none() {
                    self.e.emit(store);
                }
            }
            kind => {
                let Some(expression) = kind.as_expression() else {
                    return Err(unsupported("TypeScript syntax"));
                };
                self.expression_named(expression, Some("default"))?;
                self.e.emit(store);
            }
        }
        Ok(())
    }
}
