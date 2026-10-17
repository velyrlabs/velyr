//! Per-element rewrites: each decorated method, field and accessor adds one `memberDecs`
//! entry (`[decorators, flags, name, ...private access]`) and its share of helper outputs.

use oxc_allocator::{Box as ArenaBox, TakeIn, Vec as ArenaVec};
use oxc_ast::ast::{
    AccessorProperty, ClassElement, Decorator, Expression, MethodDefinition, MethodDefinitionKind,
    PropertyDefinition, PropertyKey,
};

use super::class::{ClassLowering, PROTO_FIELDS, PROTO_METHODS, STATIC_FIELDS, STATIC_METHODS};
use crate::accessor::base_name;
use crate::template::js_string;

const KIND_FIELD: u32 = 0;
const KIND_ACCESSOR: u32 = 1;
const KIND_METHOD: u32 = 2;
const KIND_GETTER: u32 = 3;
const KIND_SETTER: u32 = 4;
const FLAG_STATIC: u32 = 8;
const FLAG_DECORATORS_HAVE_THIS: u32 = 16;

/// How a public key is named in the helper entry and re-emitted in the class.
struct PublicKey {
    /// Computed-key expression that evaluates decorators and the key in source order.
    memo: Option<String>,
    /// Key text usable once `memo` ran.
    plain: String,
    /// Name argument of the entry.
    name: String,
}

fn group(is_static: bool, is_field: bool) -> usize {
    match (is_static, is_field) {
        (true, false) => STATIC_METHODS,
        (false, false) => PROTO_METHODS,
        (true, true) => STATIC_FIELDS,
        (false, true) => PROTO_FIELDS,
    }
}

fn private_key(key: &PropertyKey<'_>) -> Option<String> {
    match key {
        PropertyKey::PrivateIdentifier(p) => Some(p.name.as_str().to_string()),
        _ => None,
    }
}

fn private_access(storage: &str) -> [String; 2] {
    [
        format!("o => o.#{storage}"),
        format!("(o, v) => o.#{storage} = v"),
    ]
}

impl<'a> ClassLowering<'_, 'a, '_> {
    pub(super) fn method(&mut self, mut method: ArenaBox<'a, MethodDefinition<'a>>) {
        let kind = match method.kind {
            MethodDefinitionKind::Method => KIND_METHOD,
            MethodDefinitionKind::Get => KIND_GETTER,
            MethodDefinitionKind::Set => KIND_SETTER,
            MethodDefinitionKind::Constructor => unreachable!("constructors cannot be decorated"),
        };
        let is_static = method.r#static;
        let decorators = method.decorators.take_in(&self.pass.allocator);
        let Some(name) = private_key(&method.key) else {
            let (decs, this_flag, setup) = self.decorators(decorators, false);
            let key = self.public_key(&mut method.key, setup);
            if let Some(memo) = &key.memo {
                method.key = PropertyKey::from(self.template.expression(&format!("{memo};")));
                method.computed = true;
            }
            self.push_entry(&decs, kind | this_flag, is_static, &key.name, &[]);
            let text = self
                .template
                .element(ClassElement::MethodDefinition(method));
            self.body.push(text);
            return;
        };
        let (decs, this_flag, _) = self.decorators(decorators, true);
        let call = self.pass.temp(&format!("call_{name}"));
        let function = self
            .template
            .slot(Expression::FunctionExpression(method.unbox().value));
        self.push_entry(
            &decs,
            kind | this_flag,
            is_static,
            &js_string(&name),
            &[function],
        );
        self.outputs[group(is_static, false)].push(call.clone());
        self.set_brand(&name, is_static);
        let st = if is_static { "static " } else { "" };
        match kind {
            KIND_METHOD => self.front.push(format!("{st}#{name} = {call};")),
            KIND_GETTER => self
                .body
                .push(format!("{st}get #{name}() {{ return {call}(this); }}")),
            _ => self
                .body
                .push(format!("{st}set #{name}(v) {{ {call}(this, v); }}")),
        }
    }

    pub(super) fn field(&mut self, mut field: ArenaBox<'a, PropertyDefinition<'a>>) {
        let is_static = field.r#static;
        let base = base_name(&field.key);
        let decorators = field.decorators.take_in(&self.pass.allocator);
        let (decs, this_flag, name, access) = match private_key(&field.key) {
            Some(name) => {
                let (decs, this_flag, _) = self.decorators(decorators, true);
                self.set_brand(&name, is_static);
                (
                    decs,
                    this_flag,
                    js_string(&name),
                    private_access(&name).to_vec(),
                )
            }
            None => {
                let (decs, this_flag, setup) = self.decorators(decorators, false);
                let key = self.public_key(&mut field.key, setup);
                if let Some(memo) = &key.memo {
                    field.key = PropertyKey::from(self.template.expression(&format!("{memo};")));
                    field.computed = true;
                }
                (decs, this_flag, key.name, Vec::new())
            }
        };
        let init = self.pass.temp(&format!("init_{base}"));
        let extra = self.pass.temp(&format!("init_extra_{base}"));
        self.outputs[group(is_static, true)].extend([init.clone(), extra.clone()]);
        self.push_entry(&decs, KIND_FIELD | this_flag, is_static, &name, &access);
        let call = self.init_call(&init, is_static, field.value.take());
        field.value = Some(self.template.expression(&format!("{call};")));
        self.after_initializer(&extra, is_static);
        let text = self
            .template
            .element(ClassElement::PropertyDefinition(field));
        self.body.push(text);
    }

    pub(super) fn accessor(&mut self, mut accessor: ArenaBox<'a, AccessorProperty<'a>>) {
        let is_static = accessor.r#static;
        let st = if is_static { "static " } else { "" };
        let base = base_name(&accessor.key);
        let storage = self.private_name(&format!("{base}_accessor_storage"));
        let decorators = accessor.decorators.take_in(&self.pass.allocator);
        let init = self.pass.temp(&format!("init_{base}"));
        let extra = self.pass.temp(&format!("init_extra_{base}"));
        let (getter, setter) = match private_key(&accessor.key) {
            Some(name) => {
                let (decs, this_flag, _) = self.decorators(decorators, true);
                let get = self.pass.temp(&format!("get_{name}"));
                let set = self.pass.temp(&format!("set_{name}"));
                self.outputs[group(is_static, false)].extend([
                    init.clone(),
                    get.clone(),
                    set.clone(),
                    extra.clone(),
                ]);
                let access = private_access(&storage);
                self.push_entry(
                    &decs,
                    KIND_ACCESSOR | this_flag,
                    is_static,
                    &js_string(&name),
                    &access,
                );
                self.set_brand(&name, is_static);
                (
                    format!("{st}get #{name}() {{ return {get}(this); }}"),
                    format!("{st}set #{name}(v) {{ {set}(this, v); }}"),
                )
            }
            None => {
                let (decs, this_flag, setup) = self.decorators(decorators, false);
                let key = self.public_key(&mut accessor.key, setup);
                self.outputs[group(is_static, false)].extend([init.clone(), extra.clone()]);
                self.push_entry(&decs, KIND_ACCESSOR | this_flag, is_static, &key.name, &[]);
                let getter_key = key
                    .memo
                    .map_or_else(|| key.plain.clone(), |m| format!("[{m}]"));
                (
                    format!("{st}get {getter_key}() {{ return this.#{storage}; }}"),
                    format!("{st}set {}(v) {{ this.#{storage} = v; }}", key.plain),
                )
            }
        };
        let call = self.init_call(&init, is_static, accessor.value.take());
        self.body.push(format!("{st}#{storage} = {call};"));
        self.body.push(getter);
        self.body.push(setter);
        self.after_initializer(&extra, is_static);
    }

    /// Decorator text of an entry, its `this` flag and (public members only) the setup
    /// expressions that must run in the member's computed key.
    fn decorators(
        &mut self,
        decorators: ArenaVec<'a, Decorator<'a>>,
        is_private: bool,
    ) -> (String, u32, Vec<String>) {
        let mut setup = (!is_private).then(Vec::new);
        let (parts, has_this) = self.decorator_list(decorators, &mut setup);
        let decs = if has_this || parts.len() != 1 {
            format!("[{}]", parts.join(", "))
        } else {
            parts.concat()
        };
        let flag = if has_this {
            FLAG_DECORATORS_HAVE_THIS
        } else {
            0
        };
        (decs, flag, setup.unwrap_or_default())
    }

    fn public_key(&mut self, key: &mut PropertyKey<'a>, mut setup: Vec<String>) -> PublicKey {
        if let Some(name) = key.static_name() {
            let literal = js_string(&name);
            let plain = match key {
                PropertyKey::StaticIdentifier(ident) => ident.name.as_str().to_string(),
                _ => literal.clone(),
            };
            let memo = (!setup.is_empty()).then(|| {
                setup.push(literal.clone());
                format!("({})", setup.join(", "))
            });
            return PublicKey {
                memo,
                plain,
                name: literal,
            };
        }
        let temp = self.pass.temp("computedKey");
        let expression = key.take_in(&self.pass.allocator).into_expression();
        setup.push(format!("{temp} = {}", self.template.slot(expression)));
        PublicKey {
            memo: Some(format!("({})", setup.join(", "))),
            plain: format!("[{temp}]"),
            name: temp,
        }
    }

    fn push_entry(
        &mut self,
        decs: &str,
        flags: u32,
        is_static: bool,
        name: &str,
        extra: &[String],
    ) {
        let flags = flags | if is_static { FLAG_STATIC } else { 0 };
        let mut parts = vec![decs.to_string(), flags.to_string(), name.to_string()];
        parts.extend_from_slice(extra);
        self.entries.push(format!("[{}]", parts.join(", ")));
    }

    fn set_brand(&mut self, name: &str, is_static: bool) {
        if !is_static && self.brand.is_none() {
            self.brand = Some(format!("_ => #{name} in _"));
        }
    }

    /// `_init_x(this, value)` (instance) or `_init_x(value)` (static), after pending calls.
    fn init_call(&mut self, init: &str, is_static: bool, value: Option<Expression<'a>>) -> String {
        let value = value.map(|v| self.template.slot(v));
        let (call, carry) = match (is_static, value) {
            (true, value) => (
                format!("{init}({})", value.unwrap_or_default()),
                &mut self.static_carry,
            ),
            (false, Some(value)) => (format!("{init}(this, {value})"), &mut self.carry),
            (false, None) => (format!("{init}(this)"), &mut self.carry),
        };
        if carry.is_empty() {
            return call;
        }
        format!("({}, {call})", std::mem::take(carry).join(", "))
    }

    fn after_initializer(&mut self, extra: &str, is_static: bool) {
        if is_static {
            self.static_carry.push(format!("{extra}()"));
        } else {
            self.carry.push(format!("{extra}(this)"));
        }
    }
}
