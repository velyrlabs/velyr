//! Rewrites one decorated class: leading `applyDecs2311` static block, element rewrites,
//! initializer chaining and the trailing static block (or the identity wrapper).

use std::collections::HashSet;

use oxc_allocator::{Box as ArenaBox, TakeIn, Vec as ArenaVec};
use oxc_ast::ast::{
    Class, ClassElement, Decorator, Expression, PropertyDefinition, PropertyDefinitionType,
    PropertyKey, StaticBlock,
};

use super::DecoratorPass;
use crate::HELPER_BINDING;
use crate::accessor::private_names;
use crate::template::Template;

/// Output groups in the order `applyDecs2311` fills its `e` array.
pub(super) const STATIC_METHODS: usize = 0;
pub(super) const PROTO_METHODS: usize = 1;
pub(super) const STATIC_FIELDS: usize = 2;
pub(super) const PROTO_FIELDS: usize = 3;

/// What the enclosing statement or expression must do for a class with class decorators.
pub(super) struct ClassResult<'a> {
    /// Temporary holding the decorated class; every class reference is renamed to it.
    pub(super) binding: String,
    /// Class decorator evaluation that must run before the class.
    pub(super) pre: Option<Expression<'a>>,
    /// Static members that must be defined on the decorated class (Babel's identity wrapper).
    pub(super) wrapper: Option<Wrapper<'a>>,
}

pub(super) struct Wrapper<'a> {
    /// Former static fields and blocks, now instance fields of the wrapper.
    pub(super) members: Vec<ClassElement<'a>>,
    /// Calls run after them (static extra initializers, class initializers).
    pub(super) tail: Vec<String>,
}

struct ClassDecorators {
    binding: String,
    init_class: String,
    decs: Vec<String>,
    has_this: bool,
}

pub(super) struct ClassLowering<'p, 'a, 's> {
    pub(super) pass: &'p mut DecoratorPass<'a, 's>,
    pub(super) template: Template<'a>,
    pub(super) private_taken: HashSet<String>,
    pub(super) entries: Vec<String>,
    pub(super) outputs: [Vec<String>; 4],
    pub(super) brand: Option<String>,
    /// Private methods turned into fields; they precede every other element.
    pub(super) front: Vec<String>,
    pub(super) body: Vec<String>,
    /// Calls that must run before the next instance field initializer.
    pub(super) carry: Vec<String>,
    /// Calls that must run before the next static field initializer or static block.
    pub(super) static_carry: Vec<String>,
    /// Restore `name === "default"` of an anonymous default-exported class.
    default_name: bool,
}

fn is_decorated(element: &ClassElement<'_>) -> bool {
    match element {
        ClassElement::MethodDefinition(m) => !m.decorators.is_empty(),
        ClassElement::PropertyDefinition(p) => !p.decorators.is_empty(),
        ClassElement::AccessorProperty(p) => !p.decorators.is_empty(),
        _ => false,
    }
}

fn is_decorated_method(element: &ClassElement<'_>, want_static: bool) -> bool {
    matches!(element, ClassElement::MethodDefinition(m)
        if !m.decorators.is_empty() && m.r#static == want_static)
}

/// True when the lowered element includes a static field or static block.
fn makes_static_field(element: &ClassElement<'_>) -> bool {
    match element {
        ClassElement::StaticBlock(_) => true,
        ClassElement::PropertyDefinition(p) => p.r#static,
        ClassElement::AccessorProperty(p) => p.r#static && !p.decorators.is_empty(),
        ClassElement::MethodDefinition(m) => {
            m.r#static
                && !m.decorators.is_empty()
                && matches!(m.key, PropertyKey::PrivateIdentifier(_))
        }
        _ => false,
    }
}

impl<'a> DecoratorPass<'a, '_> {
    pub(super) fn rewrite_class(
        &mut self,
        class: &mut Class<'a>,
        preset: Option<String>,
        default_name: bool,
    ) -> Option<ClassResult<'a>> {
        let private_taken = private_names(class);
        let mut lowering = ClassLowering {
            template: Template::new(self.allocator),
            pass: self,
            private_taken,
            entries: Vec::new(),
            outputs: Default::default(),
            brand: None,
            front: Vec::new(),
            body: Vec::new(),
            carry: Vec::new(),
            static_carry: Vec::new(),
            default_name,
        };
        lowering.run(class, preset)
    }
}

pub(super) fn needs_rewrite(class: &Class<'_>) -> bool {
    !class.decorators.is_empty() || class.body.body.iter().any(is_decorated)
}

impl<'a> ClassLowering<'_, 'a, '_> {
    fn run(&mut self, class: &mut Class<'a>, preset: Option<String>) -> Option<ClassResult<'a>> {
        let elements = class.body.body.take_in(&self.pass.allocator);
        let proto_init = elements
            .iter()
            .any(|e| is_decorated_method(e, false))
            .then(|| self.pass.temp("initProto"));
        let static_init = elements
            .iter()
            .any(|e| is_decorated_method(e, true))
            .then(|| self.pass.temp("initStatic"));
        self.carry
            .extend(proto_init.iter().map(|init| format!("{init}(this)")));
        let parent = self.memoize_heritage(class);
        let class_decorators = class.decorators.take_in(&self.pass.allocator);
        let wrap = !class_decorators.is_empty() && elements.iter().any(makes_static_field);
        let (decorated, pre) = match class_decorators.is_empty() {
            true => (None, None),
            false => {
                let (decorated, pre) = self.class_decorators(class, class_decorators, preset);
                (Some(decorated), pre)
            }
        };
        for element in elements {
            self.element(element);
        }
        self.flush_carry();

        let mut text = self.first_block(decorated.as_ref(), parent, proto_init, static_init);
        for part in self.front.iter().chain(&self.body) {
            text.push('\n');
            text.push_str(part);
        }
        let mut tail = std::mem::take(&mut self.static_carry);
        tail.extend(decorated.as_ref().map(|d| format!("{}()", d.init_class)));
        if !wrap && !tail.is_empty() {
            text.push_str(&format!("\nstatic {{ {}; }}", tail.join("; ")));
        }
        class.body.body = self.template.class_elements(&text);
        let wrapper = wrap.then(|| Wrapper {
            members: self.move_static_members(&mut class.body.body),
            tail,
        });
        decorated.map(|d| ClassResult {
            binding: d.binding,
            pre,
            wrapper,
        })
    }

    fn first_block(
        &mut self,
        decorated: Option<&ClassDecorators>,
        parent: Option<String>,
        proto_init: Option<String>,
        static_init: Option<String>,
    ) -> String {
        let class_decs = decorated.map(|d| d.decs.join(", ")).unwrap_or_default();
        let class_this = decorated.is_some_and(|d| d.has_this);
        let mut args = format!("this, [{class_decs}], [{}]", self.entries.join(", "));
        let has_this = if class_this { "1" } else { "0" };
        match (parent, &self.brand) {
            (Some(parent), brand) => {
                let brand = brand.as_deref().unwrap_or("void 0");
                args.push_str(&format!(", {has_this}, {brand}, {parent}"));
            }
            (None, Some(brand)) => args.push_str(&format!(", {has_this}, {brand}")),
            (None, None) if class_this => args.push_str(", 1"),
            (None, None) => {}
        }
        let call = format!("{HELPER_BINDING}({args})");
        let mut e_list: Vec<String> = self.outputs.concat();
        e_list.extend(proto_init);
        e_list.extend(static_init.clone());
        let e = e_list.join(", ");
        let destructure = match decorated {
            Some(d) if !e_list.is_empty() => {
                format!(
                    "({{ e: [{e}], c: [{}, {}] }} = {call});",
                    d.binding, d.init_class
                )
            }
            Some(d) => format!("[{}, {}] = {call}.c;", d.binding, d.init_class),
            None => format!("[{e}] = {call}.e;"),
        };
        let static_call = static_init
            .map(|s| format!(" {s}(this);"))
            .unwrap_or_default();
        let name = match self.default_name {
            true => {
                "Object.defineProperty(this, \"name\", { value: \"default\", configurable: true }); "
            }
            false => "",
        };
        format!("static {{ {name}{destructure}{static_call} }}")
    }

    /// `class A extends (_Base = Base)`, so the helper sees the same parent for metadata.
    fn memoize_heritage(&mut self, class: &mut Class<'a>) -> Option<String> {
        let heritage = class.heritage.as_mut()?;
        let base = match &heritage.expression {
            Expression::Identifier(ident) => ident.name.as_str().to_string(),
            _ => "superClass".to_string(),
        };
        let temp = self.pass.temp(&base);
        let slot = self
            .template
            .slot(heritage.expression.take_in(&self.pass.allocator));
        heritage.expression = self.template.expression(&format!("({temp} = {slot});"));
        Some(temp)
    }

    /// Identifier decorators stay inline; others are evaluated before the class.
    fn class_decorators(
        &mut self,
        class: &Class<'a>,
        decorators: ArenaVec<'a, Decorator<'a>>,
        preset: Option<String>,
    ) -> (ClassDecorators, Option<Expression<'a>>) {
        let binding = preset.unwrap_or_else(|| {
            let name = class.id.as_ref().map_or("Class", |id| id.name.as_str());
            self.pass.temp(name)
        });
        if let Some(symbol) = class.id.as_ref().and_then(|id| id.symbol_id.get()) {
            self.pass.rename_symbol(symbol, &binding);
        }
        let all_identifiers = decorators
            .iter()
            .all(|d| matches!(d.expression, Expression::Identifier(_)));
        let mut setup = (!all_identifiers).then(Vec::new);
        let (decs, has_this) = self.decorator_list(decorators, &mut setup);
        let pre = setup
            .filter(|s| !s.is_empty())
            .map(|s| self.template.expression(&format!("({});", s.join(", "))));
        let init_class = self.pass.temp("initClass");
        let decorated = ClassDecorators {
            binding,
            init_class,
            decs,
            has_this,
        };
        (decorated, pre)
    }

    fn element(&mut self, element: ClassElement<'a>) {
        match element {
            ClassElement::MethodDefinition(method) if !method.decorators.is_empty() => {
                self.method(method);
            }
            ClassElement::PropertyDefinition(field) if !field.decorators.is_empty() => {
                self.field(field);
            }
            ClassElement::AccessorProperty(accessor) if !accessor.decorators.is_empty() => {
                self.accessor(accessor);
            }
            ClassElement::PropertyDefinition(mut field) if is_plain_field(&field) => {
                let is_static = field.r#static;
                self.prefix_value(&mut field.value, is_static);
                let text = self
                    .template
                    .element(ClassElement::PropertyDefinition(field));
                self.body.push(text);
            }
            other => {
                if matches!(other, ClassElement::StaticBlock(_)) && !self.static_carry.is_empty() {
                    let carry = std::mem::take(&mut self.static_carry).join("; ");
                    self.body.push(format!("static {{ {carry}; }}"));
                }
                let text = self.template.element(other);
                self.body.push(text);
            }
        }
    }

    /// Runs the pending initializer calls right before `value`.
    fn prefix_value(&mut self, value: &mut Option<Expression<'a>>, is_static: bool) {
        let carry = if is_static {
            &mut self.static_carry
        } else {
            &mut self.carry
        };
        if carry.is_empty() {
            return;
        }
        let carry = std::mem::take(carry).join(", ");
        let slot = self.optional_slot(value.take());
        *value = Some(self.template.expression(&format!("({carry}, {slot});")));
    }

    pub(super) fn optional_slot(&mut self, value: Option<Expression<'a>>) -> String {
        value.map_or_else(|| "void 0".to_string(), |v| self.template.slot(v))
    }

    /// Instance initializer calls left after the last field run in a synthetic private field.
    fn flush_carry(&mut self) {
        if self.carry.is_empty() {
            return;
        }
        let name = self.private_name("initFields");
        let carry = std::mem::take(&mut self.carry).join(", ");
        self.body.push(format!("#{name} = ({carry}, void 0);"));
    }

    pub(super) fn private_name(&mut self, base: &str) -> String {
        let mut name = base.to_string();
        while self.private_taken.contains(&name) {
            name.push('_');
        }
        self.private_taken.insert(name.clone());
        name
    }

    /// Takes every static field and static block (except the leading helper block) out of
    /// `elements`, as wrapper instance fields: `this` there is the decorated class.
    fn move_static_members(
        &mut self,
        elements: &mut ArenaVec<'a, ClassElement<'a>>,
    ) -> Vec<ClassElement<'a>> {
        let old = elements.take_in(&self.pass.allocator);
        let mut moved = Vec::new();
        let mut keys = String::new();
        for (index, element) in old.into_iter().enumerate() {
            match element {
                ClassElement::PropertyDefinition(mut field) if field.r#static => {
                    field.r#static = false;
                    if field.computed {
                        keys.push_str(&self.hoist_moved_key(&mut field.key));
                    }
                    moved.push(ClassElement::PropertyDefinition(field));
                }
                ClassElement::StaticBlock(block) if index > 0 => {
                    moved.push(self.block_field(block))
                }
                other => elements.push(other),
            }
        }
        if let Some(ClassElement::StaticBlock(first)) = elements.first_mut() {
            let mut body = self.template.statements(&keys);
            body.extend(first.body.take_in(&self.pass.allocator));
            first.body = body;
        }
        moved
    }

    /// The wrapper evaluates its keys after the class body, too late for the helper call:
    /// evaluate the key at the start of the leading static block and reuse the value.
    fn hoist_moved_key(&mut self, key: &mut PropertyKey<'a>) -> String {
        let temp = self.pass.temp("staticKey");
        let expression = key.take_in(&self.pass.allocator).into_expression();
        let slot = self.template.slot(expression);
        *key = PropertyKey::from(self.template.expression(&format!("{temp};")));
        format!("{temp} = {slot};")
    }

    fn block_field(&mut self, block: ArenaBox<'a, StaticBlock<'a>>) -> ClassElement<'a> {
        let name = self.private_name("staticBlock");
        let body = self.template.statement_list(block.unbox().body);
        let text = format!("#{name} = (() => {{ {body} }})();");
        self.template
            .class_elements(&text)
            .pop()
            .expect("template has one element")
    }
}

fn is_plain_field(field: &PropertyDefinition<'_>) -> bool {
    !field.declare && field.r#type == PropertyDefinitionType::PropertyDefinition
}
