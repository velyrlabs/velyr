//! Annex B `String.prototype` HTML methods (`anchor`, `big`, ..., `sup`), built on
//! CreateHTML (ECMA-262 §B.2.2.2.1).

use super::super::{Interpreter, JsResult, arg};
use super::{string_value, this_units};
use crate::{ObjectRef, Value};

/// CreateHTML(string, tag, attribute, value).
fn create_html(
    vm: &mut Interpreter,
    this: &Value,
    method: &str,
    tag: &str,
    attribute: Option<(&str, &Value)>,
) -> JsResult<Value> {
    let text = this_units(vm, this, method)?;
    let mut out: Vec<u16> = format!("<{tag}").encode_utf16().collect();
    if let Some((name, value)) = attribute {
        let value = vm.coerce_string(value)?.to_utf16();
        out.extend(format!(" {name}=\"").encode_utf16());
        for unit in value {
            match unit == u16::from(b'"') {
                true => out.extend("&quot;".encode_utf16()),
                false => out.push(unit),
            }
        }
        out.push(u16::from(b'"'));
    }
    out.push(u16::from(b'>'));
    out.extend(text);
    out.extend(format!("</{tag}>").encode_utf16());
    Ok(string_value(out))
}

macro_rules! html_methods {
    ($($f:ident: $method:literal, $tag:literal, $attr:expr;)*) => {
        $(
            fn $f(vm: &mut Interpreter, this: &Value, args: &[Value]) -> JsResult<Value> {
                let value = arg(args, 0);
                let attribute: Option<&str> = $attr;
                create_html(vm, this, $method, $tag, attribute.map(|name| (name, &value)))
            }
        )*

        pub(super) fn install(vm: &mut Interpreter, proto: ObjectRef) -> JsResult<()> {
            $(
                let arity = if Option::<&str>::is_some(&$attr) { 1 } else { 0 };
                vm.method(proto, $method, arity, $f)?;
            )*
            Ok(())
        }
    };
}

html_methods! {
    string_anchor: "anchor", "a", Some("name");
    string_big: "big", "big", None;
    string_blink: "blink", "blink", None;
    string_bold: "bold", "b", None;
    string_fixed: "fixed", "tt", None;
    string_fontcolor: "fontcolor", "font", Some("color");
    string_fontsize: "fontsize", "font", Some("size");
    string_italics: "italics", "i", None;
    string_link: "link", "a", Some("href");
    string_small: "small", "small", None;
    string_strike: "strike", "strike", None;
    string_sub: "sub", "sub", None;
    string_sup: "sup", "sup", None;
}
