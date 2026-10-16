//! VELYR-102: object model (shapes, descriptors, prototypes, Array exotic, inline cache).

use velyr_engine_vm::{
    InlineCache, InternalMethods, ObjectError, ObjectKey, ObjectRef, ObjectResult, Property,
    PropertyDescriptor, PropertyValue, Realm, SymbolId, Value, same_value,
};

fn object(realm: &mut Realm) -> ObjectRef {
    realm.create_object(None, &()).unwrap()
}

fn object_with_proto(realm: &mut Realm, proto: ObjectRef) -> ObjectRef {
    realm.create_object(Some(proto), &()).unwrap()
}

fn array(realm: &mut Realm, length: u32) -> ObjectRef {
    realm.create_array(None, length, &()).unwrap()
}

fn num(n: f64) -> Value {
    Value::Number(n)
}

fn obj_value(o: ObjectRef) -> Value {
    Value::Object(o.raw())
}

fn data(value: Value, writable: bool, enumerable: bool, configurable: bool) -> PropertyDescriptor {
    PropertyDescriptor::data(value, writable, enumerable, configurable)
}

fn accessor(get: Value, set: Value, enumerable: bool, configurable: bool) -> PropertyDescriptor {
    PropertyDescriptor::accessor(get, set, enumerable, configurable)
}

fn define(realm: &mut Realm, o: ObjectRef, name: &str, desc: PropertyDescriptor) -> bool {
    let key = realm.key_str(name);
    realm.define_own_property(o, key, &desc).unwrap()
}

fn own(realm: &mut Realm, o: ObjectRef, name: &str) -> Option<Property> {
    let key = realm.key_str(name);
    realm.get_own_property(o, key).unwrap()
}

fn get(realm: &mut Realm, o: ObjectRef, name: &str) -> Value {
    let key = realm.key_str(name);
    realm.get(o, key, &obj_value(o)).unwrap()
}

fn set(realm: &mut Realm, o: ObjectRef, name: &str, value: Value) -> bool {
    let key = realm.key_str(name);
    realm.set(o, key, value, &obj_value(o)).unwrap()
}

fn length(realm: &mut Realm, o: ObjectRef) -> Value {
    get(realm, o, "length")
}

fn assert_same(actual: &Value, expected: &Value) {
    assert!(
        same_value(actual, expected),
        "expected {expected:?}, got {actual:?}"
    );
}

fn assert_data(
    prop: Option<Property>,
    value: Value,
    writable: bool,
    enumerable: bool,
    configurable: bool,
) {
    let prop = prop.expect("property exists");
    let PropertyValue::Data {
        value: v,
        writable: w,
    } = &prop.value
    else {
        panic!("expected a data property, got {prop:?}");
    };
    assert_same(v, &value);
    assert_eq!(
        (*w, prop.enumerable, prop.configurable),
        (writable, enumerable, configurable)
    );
}

fn assert_accessor(
    prop: Option<Property>,
    get: Value,
    set: Value,
    enumerable: bool,
    configurable: bool,
) {
    let prop = prop.expect("property exists");
    let PropertyValue::Accessor { get: g, set: s } = &prop.value else {
        panic!("expected an accessor property, got {prop:?}");
    };
    assert_same(g, &get);
    assert_same(s, &set);
    assert_eq!(
        (prop.enumerable, prop.configurable),
        (enumerable, configurable)
    );
}

// ---------------------------------------------------------------- OwnPropertyKeys order

#[test]
fn own_keys_are_indices_ascending_then_strings_then_symbols_in_creation_order() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    let any = || data(num(0.0), true, true, true);
    let sym = |n| ObjectKey::Symbol(SymbolId(n));
    for name in [
        "b",
        "a",
        "10",
        "4294967295",
        "2",
        "-1",
        "01",
        "0",
        "4294967294",
    ] {
        assert!(define(&mut realm, o, name, any()));
    }
    assert!(realm.define_own_property(o, sym(7), &any()).unwrap());
    assert!(realm.define_own_property(o, sym(3), &any()).unwrap());
    assert!(define(&mut realm, o, "c", any()));

    let expected = vec![
        ObjectKey::Index(0),
        ObjectKey::Index(2),
        ObjectKey::Index(10),
        ObjectKey::Index(4_294_967_294),
        realm.key_str("b"),
        realm.key_str("a"),
        realm.key_str("4294967295"),
        realm.key_str("-1"),
        realm.key_str("01"),
        realm.key_str("c"),
        sym(7),
        sym(3),
    ];
    assert_eq!(realm.own_property_keys(o).unwrap(), expected);
}

#[test]
fn key_classification_only_treats_canonical_array_indices_as_indices() {
    let mut realm = Realm::default();
    assert_eq!(realm.key_str("0"), ObjectKey::Index(0));
    assert_eq!(realm.key_str("4294967294"), ObjectKey::Index(4_294_967_294));
    for name in ["4294967295", "01", "-0", "1.0", "", " 1", "1e3"] {
        assert!(
            matches!(realm.key_str(name), ObjectKey::Name(_)),
            "{name:?}"
        );
    }
}

#[test]
fn deleting_and_redefining_a_key_moves_it_to_the_end() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    for name in ["a", "b", "c"] {
        assert!(set(&mut realm, o, name, num(1.0)));
    }
    let a = realm.key_str("a");
    assert!(realm.delete(o, a).unwrap());
    assert!(set(&mut realm, o, "a", num(2.0)));
    let expected = vec![realm.key_str("b"), realm.key_str("c"), a];
    assert_eq!(realm.own_property_keys(o).unwrap(), expected);
}

#[test]
fn array_keys_put_length_before_other_strings() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    assert!(set(&mut realm, arr, "x", num(1.0)));
    assert!(set(&mut realm, arr, "1", num(1.0)));
    assert!(set(&mut realm, arr, "0", num(1.0)));
    let expected = vec![
        ObjectKey::Index(0),
        ObjectKey::Index(1),
        realm.key_str("length"),
        realm.key_str("x"),
    ];
    assert_eq!(realm.own_property_keys(arr).unwrap(), expected);
}

// ---------------------------------------------------------------- ValidateAndApplyPropertyDescriptor

/// Defines `initial` on a fresh object, then attempts `change`; returns the result and the final property.
fn redefine(initial: PropertyDescriptor, change: PropertyDescriptor) -> (bool, Option<Property>) {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    assert!(define(&mut realm, o, "p", initial));
    let accepted = define(&mut realm, o, "p", change);
    (accepted, own(&mut realm, o, "p"))
}

fn getter_pair() -> (Realm, Value, Value) {
    let mut realm = Realm::default();
    let g1 = obj_value(object(&mut realm));
    let g2 = obj_value(object(&mut realm));
    (realm, g1, g2)
}

#[test]
fn rejects_adding_to_a_non_extensible_object() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    assert!(realm.prevent_extensions(o).unwrap());
    assert!(!define(
        &mut realm,
        o,
        "p",
        data(num(1.0), true, true, true)
    ));
    assert!(!define(
        &mut realm,
        o,
        "0",
        data(num(1.0), true, true, true)
    ));
    assert!(own(&mut realm, o, "p").is_none());
}

#[test]
fn rejects_making_a_non_configurable_property_configurable() {
    let change = PropertyDescriptor {
        configurable: Some(true),
        ..Default::default()
    };
    let (accepted, prop) = redefine(data(num(1.0), true, true, false), change);
    assert!(!accepted);
    assert_data(prop, num(1.0), true, true, false);
}

#[test]
fn rejects_changing_enumerable_on_a_non_configurable_property() {
    let change = PropertyDescriptor {
        enumerable: Some(false),
        ..Default::default()
    };
    let (accepted, prop) = redefine(data(num(1.0), true, true, false), change);
    assert!(!accepted);
    assert_data(prop, num(1.0), true, true, false);
}

#[test]
fn accepts_restating_enumerable_on_a_non_configurable_property() {
    let change = PropertyDescriptor {
        enumerable: Some(true),
        configurable: Some(false),
        ..Default::default()
    };
    assert!(redefine(data(num(1.0), true, true, false), change).0);
}

#[test]
fn rejects_data_to_accessor_on_a_non_configurable_property() {
    let (_, g1, _) = getter_pair();
    let change = PropertyDescriptor {
        get: Some(g1),
        ..Default::default()
    };
    let (accepted, prop) = redefine(data(num(1.0), true, true, false), change);
    assert!(!accepted);
    assert_data(prop, num(1.0), true, true, false);
}

#[test]
fn rejects_accessor_to_data_on_a_non_configurable_property() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    let g = obj_value(object(&mut realm));
    assert!(define(
        &mut realm,
        o,
        "p",
        accessor(g.clone(), Value::Undefined, false, false)
    ));
    let change = PropertyDescriptor {
        value: Some(num(1.0)),
        ..Default::default()
    };
    assert!(!define(&mut realm, o, "p", change));
    let change = PropertyDescriptor {
        writable: Some(false),
        ..Default::default()
    };
    assert!(!define(&mut realm, o, "p", change));
    assert_accessor(own(&mut realm, o, "p"), g, Value::Undefined, false, false);
}

#[test]
fn rejects_writable_false_to_true_on_a_non_configurable_property() {
    let change = PropertyDescriptor {
        writable: Some(true),
        ..Default::default()
    };
    let (accepted, prop) = redefine(data(num(1.0), false, false, false), change);
    assert!(!accepted);
    assert_data(prop, num(1.0), false, false, false);
}

#[test]
fn accepts_writable_true_to_false_on_a_non_configurable_property() {
    let change = PropertyDescriptor {
        writable: Some(false),
        ..Default::default()
    };
    let (accepted, prop) = redefine(data(num(1.0), true, false, false), change);
    assert!(accepted);
    assert_data(prop, num(1.0), false, false, false);
}

#[test]
fn rejects_value_change_on_a_non_writable_non_configurable_property() {
    let frozen = || data(num(1.0), false, false, false);
    let value = |v| PropertyDescriptor {
        value: Some(v),
        ..Default::default()
    };
    assert!(!redefine(frozen(), value(num(2.0))).0);
    assert!(!redefine(data(num(0.0), false, false, false), value(num(-0.0))).0);
    assert!(redefine(frozen(), value(num(1.0))).0);
    assert!(
        redefine(
            data(num(f64::NAN), false, false, false),
            value(num(f64::NAN))
        )
        .0
    );
}

#[test]
fn accepts_value_change_on_a_writable_non_configurable_property() {
    let change = PropertyDescriptor {
        value: Some(num(2.0)),
        ..Default::default()
    };
    let (accepted, prop) = redefine(data(num(1.0), true, false, false), change);
    assert!(accepted);
    assert_data(prop, num(2.0), true, false, false);
}

#[test]
fn rejects_getter_or_setter_change_on_a_non_configurable_accessor() {
    let (mut realm, g1, g2) = getter_pair();
    let o = object(&mut realm);
    assert!(define(
        &mut realm,
        o,
        "p",
        accessor(g1.clone(), g1.clone(), true, false)
    ));
    let get = PropertyDescriptor {
        get: Some(g2.clone()),
        ..Default::default()
    };
    assert!(!define(&mut realm, o, "p", get));
    let set_ = PropertyDescriptor {
        set: Some(Value::Undefined),
        ..Default::default()
    };
    assert!(!define(&mut realm, o, "p", set_));
    let same = PropertyDescriptor {
        get: Some(g1.clone()),
        set: Some(g1.clone()),
        ..Default::default()
    };
    assert!(define(&mut realm, o, "p", same));
    assert_accessor(own(&mut realm, o, "p"), g1.clone(), g1, true, false);
}

#[test]
fn empty_descriptor_is_accepted_on_a_non_configurable_property() {
    let (accepted, prop) = redefine(
        data(num(1.0), false, false, false),
        PropertyDescriptor::default(),
    );
    assert!(accepted);
    assert_data(prop, num(1.0), false, false, false);
}

#[test]
fn new_property_fields_default_to_false_and_undefined() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    assert!(define(
        &mut realm,
        o,
        "d",
        PropertyDescriptor {
            value: Some(num(1.0)),
            ..Default::default()
        }
    ));
    assert_data(own(&mut realm, o, "d"), num(1.0), false, false, false);
    assert!(define(
        &mut realm,
        o,
        "g",
        PropertyDescriptor {
            enumerable: Some(true),
            ..Default::default()
        }
    ));
    assert_data(
        own(&mut realm, o, "g"),
        Value::Undefined,
        false,
        true,
        false,
    );
    let getter = obj_value(object(&mut realm));
    assert!(define(
        &mut realm,
        o,
        "a",
        PropertyDescriptor {
            get: Some(getter.clone()),
            ..Default::default()
        }
    ));
    assert_accessor(
        own(&mut realm, o, "a"),
        getter,
        Value::Undefined,
        false,
        false,
    );
}

#[test]
fn configurable_data_converts_to_accessor_keeping_enumerable_and_configurable() {
    let (_, g1, _) = getter_pair();
    let change = PropertyDescriptor {
        get: Some(g1.clone()),
        ..Default::default()
    };
    let (accepted, prop) = redefine(data(num(1.0), true, true, true), change);
    assert!(accepted);
    assert_accessor(prop, g1, Value::Undefined, true, true);
}

#[test]
fn configurable_accessor_converts_to_data_with_defaults() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    let g = obj_value(object(&mut realm));
    assert!(define(
        &mut realm,
        o,
        "p",
        accessor(g.clone(), g, false, true)
    ));
    assert!(define(
        &mut realm,
        o,
        "p",
        PropertyDescriptor {
            writable: Some(true),
            ..Default::default()
        }
    ));
    assert_data(own(&mut realm, o, "p"), Value::Undefined, true, false, true);
}

#[test]
fn generic_descriptor_on_accessor_keeps_the_accessor() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    let g = obj_value(object(&mut realm));
    assert!(define(
        &mut realm,
        o,
        "p",
        accessor(g.clone(), Value::Undefined, true, true)
    ));
    assert!(define(
        &mut realm,
        o,
        "p",
        PropertyDescriptor {
            enumerable: Some(false),
            ..Default::default()
        }
    ));
    assert_accessor(own(&mut realm, o, "p"), g, Value::Undefined, false, true);
}

#[test]
fn configurable_non_writable_value_can_change() {
    let change = PropertyDescriptor {
        value: Some(num(5.0)),
        ..Default::default()
    };
    let (accepted, prop) = redefine(data(num(1.0), false, true, true), change);
    assert!(accepted);
    assert_data(prop, num(5.0), false, true, true);
}

#[test]
fn element_descriptors_follow_the_same_validation() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    assert!(define(
        &mut realm,
        arr,
        "0",
        data(num(1.0), false, true, false)
    ));
    assert!(!define(
        &mut realm,
        arr,
        "0",
        PropertyDescriptor {
            value: Some(num(2.0)),
            ..Default::default()
        }
    ));
    assert!(!define(
        &mut realm,
        arr,
        "0",
        PropertyDescriptor {
            configurable: Some(true),
            ..Default::default()
        }
    ));
    assert!(!set(&mut realm, arr, "0", num(3.0)));
    assert_data(own(&mut realm, arr, "0"), num(1.0), false, true, false);
    let key = ObjectKey::Index(0);
    assert!(!realm.delete(arr, key).unwrap());
}

// ---------------------------------------------------------------- [[Get]] / [[Set]] / [[Delete]] / [[HasProperty]]

/// Test stand-in for function calls: a getter returns `this.tag`, a setter stores its argument in `this.stored`.
fn test_call(
    realm: &mut Realm,
    _function: &Value,
    this: &Value,
    args: &[Value],
) -> ObjectResult<Value> {
    let Value::Object(this) = this else {
        return Err(ObjectError::Type("receiver is not an object".into()));
    };
    let this = ObjectRef::from_raw(*this);
    match args.first() {
        None => {
            let key = realm.key_str("tag");
            realm.get(this, key, &obj_value(this))
        }
        Some(value) => {
            let key = realm.key_str("stored");
            realm.set(this, key, value.clone(), &obj_value(this))?;
            Ok(Value::Undefined)
        }
    }
}

#[test]
fn get_walks_the_prototype_chain() {
    let mut realm = Realm::default();
    let proto = object(&mut realm);
    let child = object_with_proto(&mut realm, proto);
    assert!(set(&mut realm, proto, "x", num(1.0)));
    assert_same(&get(&mut realm, child, "x"), &num(1.0));
    assert_same(&get(&mut realm, child, "missing"), &Value::Undefined);
    let x = realm.key_str("x");
    assert!(realm.has_property(child, x).unwrap());
    assert!(realm.get_own_property(child, x).unwrap().is_none());
}

#[test]
fn inherited_getter_and_setter_receive_the_receiver() {
    let mut realm = Realm::default();
    realm.call = test_call;
    let proto = object(&mut realm);
    let child = object_with_proto(&mut realm, proto);
    let f = obj_value(object(&mut realm));
    assert!(define(
        &mut realm,
        proto,
        "p",
        accessor(f.clone(), f, true, true)
    ));
    assert!(set(&mut realm, child, "tag", num(42.0)));
    assert_same(&get(&mut realm, child, "p"), &num(42.0));
    assert!(set(&mut realm, child, "p", num(7.0)));
    assert_same(&get(&mut realm, child, "stored"), &num(7.0));
    assert!(own(&mut realm, child, "p").is_none());
}

#[test]
fn accessor_without_getter_reads_undefined_and_without_setter_fails() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    assert!(define(
        &mut realm,
        o,
        "p",
        accessor(Value::Undefined, Value::Undefined, true, true)
    ));
    assert_same(&get(&mut realm, o, "p"), &Value::Undefined);
    assert!(!set(&mut realm, o, "p", num(1.0)));
}

#[test]
fn getter_errors_propagate_from_the_call_hook() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    let f = obj_value(object(&mut realm));
    assert!(define(
        &mut realm,
        o,
        "p",
        accessor(f, Value::Undefined, true, true)
    ));
    let key = realm.key_str("p");
    assert!(matches!(
        realm.get(o, key, &obj_value(o)),
        Err(ObjectError::Type(_))
    ));
}

#[test]
fn set_on_inherited_writable_data_creates_an_own_property() {
    let mut realm = Realm::default();
    let proto = object(&mut realm);
    let child = object_with_proto(&mut realm, proto);
    assert!(set(&mut realm, proto, "x", num(1.0)));
    assert!(set(&mut realm, child, "x", num(2.0)));
    assert_data(own(&mut realm, child, "x"), num(2.0), true, true, true);
    assert_same(&get(&mut realm, proto, "x"), &num(1.0));
}

#[test]
fn set_on_inherited_non_writable_data_fails() {
    let mut realm = Realm::default();
    let proto = object(&mut realm);
    let child = object_with_proto(&mut realm, proto);
    assert!(define(
        &mut realm,
        proto,
        "x",
        data(num(1.0), false, true, true)
    ));
    assert!(!set(&mut realm, child, "x", num(2.0)));
    assert!(own(&mut realm, child, "x").is_none());
}

#[test]
fn set_with_a_distinct_receiver_writes_to_the_receiver() {
    let mut realm = Realm::default();
    let target = object(&mut realm);
    let receiver = object(&mut realm);
    assert!(set(&mut realm, target, "x", num(1.0)));
    let x = realm.key_str("x");
    assert!(
        realm
            .set(target, x, num(9.0), &obj_value(receiver))
            .unwrap()
    );
    assert_same(&get(&mut realm, target, "x"), &num(1.0));
    assert_data(own(&mut realm, receiver, "x"), num(9.0), true, true, true);
}

#[test]
fn set_with_an_existing_receiver_property_only_updates_its_value() {
    let mut realm = Realm::default();
    let target = object(&mut realm);
    let receiver = object(&mut realm);
    let x = realm.key_str("x");
    assert!(define(
        &mut realm,
        receiver,
        "x",
        data(num(1.0), true, false, false)
    ));
    assert!(
        realm
            .set(target, x, num(2.0), &obj_value(receiver))
            .unwrap()
    );
    assert_data(own(&mut realm, receiver, "x"), num(2.0), true, false, false);

    assert!(define(
        &mut realm,
        receiver,
        "y",
        accessor(Value::Undefined, Value::Undefined, true, true)
    ));
    let y = realm.key_str("y");
    assert!(
        !realm
            .set(target, y, num(2.0), &obj_value(receiver))
            .unwrap()
    );
    assert!(define(
        &mut realm,
        receiver,
        "z",
        data(num(1.0), false, true, true)
    ));
    let z = realm.key_str("z");
    assert!(
        !realm
            .set(target, z, num(2.0), &obj_value(receiver))
            .unwrap()
    );
}

#[test]
fn set_with_a_primitive_receiver_fails() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    let x = realm.key_str("x");
    assert!(!realm.set(o, x, num(1.0), &num(5.0)).unwrap());
}

#[test]
fn delete_removes_configurable_and_keeps_non_configurable() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    assert!(define(&mut realm, o, "c", data(num(1.0), true, true, true)));
    assert!(define(
        &mut realm,
        o,
        "n",
        data(num(1.0), true, true, false)
    ));
    let (c, n, missing) = (
        realm.key_str("c"),
        realm.key_str("n"),
        realm.key_str("missing"),
    );
    assert!(realm.delete(o, c).unwrap());
    assert!(!realm.delete(o, n).unwrap());
    assert!(realm.delete(o, missing).unwrap());
    assert!(own(&mut realm, o, "c").is_none());
    assert!(own(&mut realm, o, "n").is_some());
}

#[test]
fn prevent_extensions_blocks_new_keys_but_not_writes() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    assert!(set(&mut realm, o, "x", num(1.0)));
    assert!(realm.is_extensible(o).unwrap());
    assert!(realm.prevent_extensions(o).unwrap());
    assert!(!realm.is_extensible(o).unwrap());
    assert!(set(&mut realm, o, "x", num(2.0)));
    assert!(!set(&mut realm, o, "y", num(2.0)));
    assert_same(&get(&mut realm, o, "x"), &num(2.0));
}

#[test]
fn set_prototype_of_rejects_cycles_and_non_extensible_targets() {
    let mut realm = Realm::default();
    let a = object(&mut realm);
    let b = object_with_proto(&mut realm, a);
    assert!(!realm.set_prototype_of(a, Some(b)).unwrap());
    assert!(!realm.set_prototype_of(a, Some(a)).unwrap());
    assert!(realm.set_prototype_of(b, None).unwrap());
    assert_eq!(realm.get_prototype_of(b).unwrap(), None);
    assert!(realm.prevent_extensions(b).unwrap());
    assert!(realm.set_prototype_of(b, None).unwrap());
    assert!(!realm.set_prototype_of(b, Some(a)).unwrap());
}

// ---------------------------------------------------------------- Array exotic objects

#[test]
fn writing_indices_grows_length() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    assert_same(&length(&mut realm, arr), &num(0.0));
    assert!(set(&mut realm, arr, "0", num(1.0)));
    assert!(set(&mut realm, arr, "10", num(2.0)));
    assert_same(&length(&mut realm, arr), &num(11.0));
    assert!(own(&mut realm, arr, "5").is_none());
    assert_data(
        own(&mut realm, arr, "length"),
        num(11.0),
        true,
        false,
        false,
    );
    assert_same(&get(&mut realm, arr, "10"), &num(2.0));
}

#[test]
fn created_array_has_holes_up_to_its_length() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 3);
    assert_same(&length(&mut realm, arr), &num(3.0));
    assert_eq!(
        realm.own_property_keys(arr).unwrap(),
        vec![realm.key_str("length")]
    );
}

#[test]
fn sparse_indices_keep_ascending_order_and_length() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    assert!(set(&mut realm, arr, "1000000", num(1.0)));
    assert!(set(&mut realm, arr, "5", num(1.0)));
    assert!(set(&mut realm, arr, "4294967294", num(1.0)));
    assert_same(&length(&mut realm, arr), &num(4_294_967_295.0));
    let keys = realm.own_property_keys(arr).unwrap();
    assert_eq!(
        &keys[..3],
        &[
            ObjectKey::Index(5),
            ObjectKey::Index(1_000_000),
            ObjectKey::Index(4_294_967_294)
        ]
    );
}

#[test]
fn names_past_the_index_range_do_not_touch_length() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    assert!(set(&mut realm, arr, "4294967295", num(1.0)));
    assert_same(&length(&mut realm, arr), &num(0.0));
}

#[test]
fn shrinking_length_deletes_elements() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    for i in ["0", "1", "2"] {
        assert!(set(&mut realm, arr, i, num(1.0)));
    }
    assert!(set(&mut realm, arr, "length", num(1.0)));
    assert!(own(&mut realm, arr, "1").is_none());
    assert_eq!(
        realm.own_property_keys(arr).unwrap()[0],
        ObjectKey::Index(0)
    );
    assert!(set(&mut realm, arr, "length", num(5.0)));
    assert_same(&length(&mut realm, arr), &num(5.0));
    assert!(own(&mut realm, arr, "1").is_none());
}

#[test]
fn invalid_length_throws_range_error() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    let key = realm.key_str("length");
    for bad in [num(1.5), num(-1.0), num(4_294_967_296.0), num(f64::NAN)] {
        let desc = PropertyDescriptor {
            value: Some(bad),
            ..Default::default()
        };
        assert!(matches!(
            realm.define_own_property(arr, key, &desc),
            Err(ObjectError::Range(_))
        ));
    }
}

#[test]
fn non_configurable_element_stops_truncation() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    for i in ["0", "1", "2"] {
        assert!(set(&mut realm, arr, i, num(1.0)));
    }
    assert!(define(
        &mut realm,
        arr,
        "1",
        data(num(1.0), true, true, false)
    ));
    assert!(!set(&mut realm, arr, "length", num(0.0)));
    assert_data(own(&mut realm, arr, "length"), num(2.0), true, false, false);
    assert!(own(&mut realm, arr, "2").is_none());
    assert!(own(&mut realm, arr, "0").is_some());
}

#[test]
fn failed_truncation_still_applies_writable_false() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    assert!(define(
        &mut realm,
        arr,
        "3",
        data(num(1.0), true, true, false)
    ));
    assert!(set(&mut realm, arr, "5", num(1.0)));
    let key = realm.key_str("length");
    let desc = PropertyDescriptor {
        value: Some(num(0.0)),
        writable: Some(false),
        ..Default::default()
    };
    assert!(!realm.define_own_property(arr, key, &desc).unwrap());
    assert_data(
        own(&mut realm, arr, "length"),
        num(4.0),
        false,
        false,
        false,
    );
}

#[test]
fn non_writable_length_blocks_growth_and_writes() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    assert!(set(&mut realm, arr, "0", num(1.0)));
    let desc = PropertyDescriptor {
        writable: Some(false),
        ..Default::default()
    };
    assert!(define(&mut realm, arr, "length", desc));
    assert!(!set(&mut realm, arr, "1", num(1.0)));
    assert!(!set(&mut realm, arr, "length", num(0.0)));
    assert!(set(&mut realm, arr, "0", num(2.0)));
    assert!(!define(
        &mut realm,
        arr,
        "length",
        PropertyDescriptor {
            writable: Some(true),
            ..Default::default()
        }
    ));
    assert!(
        !set(&mut realm, arr, "length", num(1.0)),
        "[[Set]] fails on non-writable even with the same value"
    );
    let same = PropertyDescriptor {
        value: Some(num(1.0)),
        ..Default::default()
    };
    assert!(
        define(&mut realm, arr, "length", same),
        "defining the same value is accepted"
    );
    assert_data(
        own(&mut realm, arr, "length"),
        num(1.0),
        false,
        false,
        false,
    );
    let length = realm.key_str("length");
    assert!(!realm.delete(arr, length).unwrap());
}

#[test]
fn length_attributes_cannot_be_reconfigured() {
    let mut realm = Realm::default();
    let arr = array(&mut realm, 0);
    assert!(!define(
        &mut realm,
        arr,
        "length",
        PropertyDescriptor {
            enumerable: Some(true),
            ..Default::default()
        }
    ));
    assert!(!define(
        &mut realm,
        arr,
        "length",
        PropertyDescriptor {
            configurable: Some(true),
            ..Default::default()
        }
    ));
    assert!(!define(
        &mut realm,
        arr,
        "length",
        accessor(Value::Undefined, Value::Undefined, false, false)
    ));
}

// ---------------------------------------------------------------- Shapes and dictionary mode

#[test]
fn same_insertion_order_shares_a_shape() {
    let mut realm = Realm::default();
    let (a, b, c) = (object(&mut realm), object(&mut realm), object(&mut realm));
    for o in [a, b] {
        assert!(set(&mut realm, o, "x", num(1.0)));
        assert!(set(&mut realm, o, "y", num(2.0)));
    }
    assert!(set(&mut realm, c, "y", num(1.0)));
    assert!(set(&mut realm, c, "x", num(2.0)));
    assert_eq!(realm.shape_id(a), realm.shape_id(b));
    assert!(realm.shape_id(a).is_some());
    assert_ne!(realm.shape_id(a), realm.shape_id(c));
}

#[test]
fn reconfiguring_attributes_transitions_to_a_shared_shape() {
    let mut realm = Realm::default();
    let (a, b) = (object(&mut realm), object(&mut realm));
    for o in [a, b] {
        assert!(set(&mut realm, o, "x", num(1.0)));
    }
    let before = realm.shape_id(a);
    for o in [a, b] {
        assert!(define(
            &mut realm,
            o,
            "x",
            PropertyDescriptor {
                writable: Some(false),
                ..Default::default()
            }
        ));
    }
    assert_ne!(realm.shape_id(a), before);
    assert_eq!(realm.shape_id(a), realm.shape_id(b));
    assert!(!set(&mut realm, a, "x", num(2.0)));
}

#[test]
fn many_deletions_switch_to_dictionary_mode_preserving_order_and_values() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    let names: Vec<String> = (0..40).map(|i| format!("k{i}")).collect();
    for (i, name) in names.iter().enumerate() {
        assert!(set(&mut realm, o, name, num(i as f64)));
    }
    for name in names.iter().step_by(2) {
        let key = realm.key_str(name);
        assert!(realm.delete(o, key).unwrap());
    }
    assert_eq!(realm.shape_id(o), None, "object is in dictionary mode");
    assert!(set(&mut realm, o, "tail", num(-1.0)));
    let mut expected: Vec<ObjectKey> = names
        .iter()
        .skip(1)
        .step_by(2)
        .map(|n| realm.key_str(n))
        .collect();
    expected.push(realm.key_str("tail"));
    assert_eq!(realm.own_property_keys(o).unwrap(), expected);
    assert_same(&get(&mut realm, o, "k39"), &num(39.0));
    assert_same(&get(&mut realm, o, "k38"), &Value::Undefined);
}

#[test]
fn few_deletions_stay_in_shape_mode() {
    let mut realm = Realm::default();
    let o = object(&mut realm);
    for name in ["a", "b", "c"] {
        assert!(set(&mut realm, o, name, num(1.0)));
    }
    let b = realm.key_str("b");
    assert!(realm.delete(o, b).unwrap());
    assert!(realm.shape_id(o).is_some());
    assert_same(&get(&mut realm, o, "c"), &num(1.0));
    let expected = vec![realm.key_str("a"), realm.key_str("c")];
    assert_eq!(realm.own_property_keys(o).unwrap(), expected);
}

// ---------------------------------------------------------------- Inline cache

#[test]
fn load_cache_hits_on_the_same_shape_and_misses_after_a_transition() {
    let mut realm = Realm::default();
    let (a, b) = (object(&mut realm), object(&mut realm));
    for (o, v) in [(a, 1.0), (b, 2.0)] {
        assert!(set(&mut realm, o, "x", num(v)));
    }
    let x = realm.key_str("x");
    let mut ic = InlineCache::default();
    assert_same(&ic.load(&mut realm, a, x).unwrap(), &num(1.0));
    assert_eq!((ic.misses(), ic.cached_shape()), (1, realm.shape_id(a)));
    assert_same(&ic.load(&mut realm, b, x).unwrap(), &num(2.0));
    assert_eq!(ic.misses(), 1, "same shape hits");

    assert!(set(&mut realm, b, "y", num(0.0)));
    assert_same(&ic.load(&mut realm, b, x).unwrap(), &num(2.0));
    assert_eq!((ic.misses(), ic.cached_shape()), (2, realm.shape_id(b)));
}

#[test]
fn load_cache_ignores_inherited_and_dictionary_properties() {
    let mut realm = Realm::default();
    let proto = object(&mut realm);
    let child = object_with_proto(&mut realm, proto);
    assert!(set(&mut realm, proto, "x", num(1.0)));
    let x = realm.key_str("x");
    let mut ic = InlineCache::default();
    assert_same(&ic.load(&mut realm, child, x).unwrap(), &num(1.0));
    assert_same(&ic.load(&mut realm, child, x).unwrap(), &num(1.0));
    assert_eq!((ic.misses(), ic.cached_shape()), (2, None));
}

#[test]
fn load_cache_falls_back_when_a_same_shape_slot_holds_an_accessor() {
    let mut realm = Realm::default();
    realm.call = test_call;
    let (a, b) = (object(&mut realm), object(&mut realm));
    let f = obj_value(object(&mut realm));
    assert!(define(
        &mut realm,
        a,
        "x",
        data(num(1.0), false, true, true)
    ));
    assert!(define(
        &mut realm,
        b,
        "x",
        accessor(f, Value::Undefined, true, true)
    ));
    assert!(define(
        &mut realm,
        b,
        "tag",
        data(num(5.0), true, true, true)
    ));
    let x = realm.key_str("x");
    let mut ic = InlineCache::default();
    assert_same(&ic.load(&mut realm, a, x).unwrap(), &num(1.0));
    assert_same(&ic.load(&mut realm, b, x).unwrap(), &num(5.0));
}

#[test]
fn store_cache_writes_writable_slots_and_rejects_read_only_ones() {
    let mut realm = Realm::default();
    let (a, b) = (object(&mut realm), object(&mut realm));
    for o in [a, b] {
        assert!(set(&mut realm, o, "x", num(0.0)));
    }
    let x = realm.key_str("x");
    let mut ic = InlineCache::default();
    assert!(ic.store(&mut realm, a, x, num(1.0)).unwrap());
    assert!(ic.store(&mut realm, b, x, num(2.0)).unwrap());
    assert_eq!(ic.misses(), 1);
    assert_same(&get(&mut realm, b, "x"), &num(2.0));

    assert!(define(
        &mut realm,
        b,
        "x",
        PropertyDescriptor {
            writable: Some(false),
            ..Default::default()
        }
    ));
    assert!(!ic.store(&mut realm, b, x, num(3.0)).unwrap());
    assert_same(&get(&mut realm, b, "x"), &num(2.0));
    assert_eq!(ic.misses(), 2);
}

// ---------------------------------------------------------------- GC integration

#[test]
fn objects_trace_their_prototype_and_property_values() {
    let mut realm = Realm::default();
    let proto = object(&mut realm);
    let child = object_with_proto(&mut realm, proto);
    let value = object(&mut realm);
    let getter = object(&mut realm);
    let element = object(&mut realm);
    let garbage = object(&mut realm);
    assert!(set(&mut realm, child, "v", obj_value(value)));
    assert!(define(
        &mut realm,
        child,
        "a",
        accessor(obj_value(getter), Value::Undefined, true, true)
    ));
    assert!(set(&mut realm, child, "0", obj_value(element)));

    realm.heap.collect(&child);
    for alive in [child, proto, value, getter, element] {
        assert!(realm.heap.is_alive(alive.raw()));
    }
    assert!(!realm.heap.is_alive(garbage.raw()));
}
