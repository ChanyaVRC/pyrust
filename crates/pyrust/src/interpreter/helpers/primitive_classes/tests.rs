#[test]
fn primitive_provider_metadata_matches_installed_class_attributes() {
    use pyrust_builtins::primitive_class_attrs::{PrimitiveClassAttrKind, PrimitiveClassAttrs};

    fn assert_plain_builtin(owner: &str, name: &str, value: &Value, expected_dispatch_key: &str) {
        assert!(
            matches!(
                value.kind(),
                ValueKind::BuiltinFunction(actual) if actual == expected_dispatch_key
            ),
            "{owner}.{name} must be the ordinary built-in sentinel \
             {expected_dispatch_key:?}, got {value:?}"
        );
    }

    fn assert_class_attrs(class: &Rc<RefCell<PyClass>>, spec: &'static PrimitiveClassAttrs) {
        assert_eq!(
            class.borrow().name,
            spec.type_name,
            "provider metadata attached to the wrong primitive class"
        );

        let mut seen = std::collections::HashSet::new();
        for attr in spec.iter() {
            assert!(
                seen.insert(attr.name),
                "{} metadata installs {:?} more than once",
                spec.type_name,
                attr.name
            );
            let value = class
                .borrow()
                .attrs
                .get(attr.name)
                .unwrap_or_else(|| {
                    panic!(
                        "{} provider metadata declared missing attribute {:?}",
                        spec.type_name, attr.name
                    )
                })
                .clone();
            let expected_dispatch_key = format!("{}.{}", spec.type_name, attr.name);

            match attr.kind {
                PrimitiveClassAttrKind::NativeClassMethod => {
                    let plan =
                        pyrust_builtins::classmethod::native_class_method_cache_plan(&value, class)
                            .unwrap_or_else(|| {
                                panic!(
                                    "{}.{} must be a native classmethod descriptor",
                                    spec.type_name, attr.name
                                )
                            });
                    let (wrapped, receiver) =
                        pyrust_builtins::classmethod::cached_native_class_method_call(&plan, class)
                            .expect("provider-owned classmethod must bind its defining class");
                    assert_plain_builtin(
                        spec.type_name,
                        attr.name,
                        &wrapped,
                        &expected_dispatch_key,
                    );
                    assert!(
                        matches!(
                            receiver.kind(),
                            ValueKind::PyClass(owner) if Rc::ptr_eq(owner, class)
                        ),
                        "{}.{} must bind the defining class",
                        spec.type_name,
                        attr.name
                    );
                }
                PrimitiveClassAttrKind::NativeStaticMethod => {
                    let wrapped = pyrust_builtins::classmethod::as_static_method_any(&value)
                        .unwrap_or_else(|| {
                            panic!(
                                "{}.{} must be a staticmethod descriptor",
                                spec.type_name, attr.name
                            )
                        });
                    let call = pyrust_builtins::native_builtin_callable::as_native_static_builtin(
                        &wrapped,
                    )
                    .unwrap_or_else(|| {
                        panic!(
                            "{}.{} must wrap a stable native callable",
                            spec.type_name, attr.name
                        )
                    });
                    assert_plain_builtin(
                        spec.type_name,
                        attr.name,
                        &call.wrapped,
                        &expected_dispatch_key,
                    );
                    assert!(
                        call.receiver.is_none(),
                        "{}.{} staticmethod must not capture a receiver",
                        spec.type_name,
                        attr.name
                    );
                }
                PrimitiveClassAttrKind::InstanceMethod
                | PrimitiveClassAttrKind::Init
                | PrimitiveClassAttrKind::New
                | PrimitiveClassAttrKind::ClassGetItem
                | PrimitiveClassAttrKind::OwnedSlot => {
                    assert_plain_builtin(spec.type_name, attr.name, &value, &expected_dispatch_key)
                }
            }
        }
    }

    let classes = build_primitive_classes();
    for (class, spec) in [
        (
            &classes.bool_class,
            &pyrust_builtins::primitive_class_attrs::BOOL,
        ),
        (
            &classes.bytearray_class,
            &pyrust_builtins::bytearray::CLASS_ATTRS,
        ),
        (&classes.bytes_class, &pyrust_builtins::bytes::CLASS_ATTRS),
        (
            &classes.complex_class,
            &pyrust_builtins::complex::CLASS_ATTRS,
        ),
        (&classes.dict_class, &pyrust_builtins::dict::CLASS_ATTRS),
        (&classes.float_class, &pyrust_builtins::float::CLASS_ATTRS),
        (
            &classes.frozenset_class,
            &pyrust_builtins::frozenset::CLASS_ATTRS,
        ),
        (&classes.int_class, &pyrust_builtins::int::CLASS_ATTRS),
        (&classes.list_class, &pyrust_builtins::list::CLASS_ATTRS),
        (&classes.set_class, &pyrust_builtins::set::CLASS_ATTRS),
        (&classes.str_class, &pyrust_builtins::string::CLASS_ATTRS),
        (&classes.tuple_class, &pyrust_builtins::tuple::CLASS_ATTRS),
    ] {
        assert_class_attrs(class, spec);
    }

    // A derived primitive must not acquire fresh descriptors merely because
    // its runtime dispatch table accepts the inherited protocol.  The class
    // dictionary owns only bool's seven real overrides; common numeric slots
    // continue to resolve from int and therefore retain int as __objclass__.
    for inherited in ["__add__", "__eq__", "__float__", "__index__", "__round__"] {
        assert!(
            !classes.bool_class.borrow().attrs.contains_key(inherited),
            "bool.{inherited} must be inherited from int, not re-materialized"
        );
        assert!(
            classes.int_class.borrow().attrs.contains_key(inherited),
            "int must own the inherited bool slot {inherited}"
        );
    }
}
