// Python-source injection (`namedtuple`, `OrderedDict`, `ChainMap`, `UserDict`,
// `UserList`, `UserString`), public class tagging, OrderedDict relinking bridge,
// and native deque iterator constructor dispatch.

const COLLECTIONS_PY_SOURCE: &str = include_str!("../collections_py.py");

const COLLECTIONS_PY_EXPORTS: [&str; 6] = [
    "namedtuple",
    "OrderedDict",
    "ChainMap",
    "UserDict",
    "UserList",
    "UserString",
];

/// Public container classes tagged after module construction and the immutable
/// registry sentinel used for each class's PEP 585 adapter.
const COLLECTION_CLASS_GETITEM_DISPATCH: [(&str, &str); 8] = [
    ("Counter", "Counter.__class_getitem__"),
    ("defaultdict", "defaultdict.__class_getitem__"),
    ("deque", "deque.__class_getitem__"),
    ("OrderedDict", "OrderedDict.__class_getitem__"),
    ("ChainMap", "ChainMap.__class_getitem__"),
    ("UserDict", "UserDict.__class_getitem__"),
    ("UserList", "UserList.__class_getitem__"),
    ("UserString", "UserString.__class_getitem__"),
];

#[cfg(test)]
mod ownership_tests {
    use super::*;

    fn test_class(name: &str, base: Option<Rc<RefCell<PyClass>>>) -> Rc<RefCell<PyClass>> {
        Rc::new(RefCell::new(PyClass {
            name: name.to_string(),
            qualname: name.to_string(),
            base,
            ..PyClass::default()
        }))
    }

    #[test]
    fn class_getitem_dispatch_names_are_static_and_complete() {
        assert_eq!(
            COLLECTION_CLASS_GETITEM_DISPATCH,
            [
                ("Counter", "Counter.__class_getitem__"),
                ("defaultdict", "defaultdict.__class_getitem__"),
                ("deque", "deque.__class_getitem__"),
                ("OrderedDict", "OrderedDict.__class_getitem__"),
                ("ChainMap", "ChainMap.__class_getitem__"),
                ("UserDict", "UserDict.__class_getitem__"),
                ("UserList", "UserList.__class_getitem__"),
                ("UserString", "UserString.__class_getitem__"),
            ]
        );

        let owner = include_str!("../collections.rs");
        let setup = include_str!("module_setup.rs");
        let forbidden = concat!("Box", "::leak");
        assert!(
            !owner.contains(forbidden) && !setup.contains(forbidden),
            "re-importable collections generations must not leak dispatch names"
        );
    }

    #[test]
    fn retained_counter_resolves_base_from_its_own_generation() {
        let old_counter = test_class("old Counter", None);
        register_canonical_collection_class(CanonicalCollectionKind::Counter, &old_counter);

        let new_counter = test_class("new Counter", None);
        register_canonical_collection_class(CanonicalCollectionKind::Counter, &new_counter);

        let old_counter_subclass = test_class("old Counter child", Some(Rc::clone(&old_counter)));
        assert!(
            Rc::ptr_eq(
                &canonical_collection_base_for_receiver(
                    &old_counter,
                    CanonicalCollectionKind::Counter,
                )
                .unwrap(),
                &old_counter,
            )
        );
        assert!(Rc::ptr_eq(
            &canonical_collection_base_for_receiver(
                &old_counter_subclass,
                CanonicalCollectionKind::Counter,
            )
            .unwrap(),
            &old_counter,
        ));
        assert!(
            Rc::ptr_eq(
                &canonical_collection_base_for_receiver(
                    &new_counter,
                    CanonicalCollectionKind::Counter,
                )
                .unwrap(),
                &new_counter,
            )
        );
    }
}

/// Execute `COLLECTIONS_PY_SOURCE` once and copy its public names onto the
/// `collections` module's attribute map.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let itertools = interp.load_module("itertools")?;
    let chain = match itertools.kind() {
        ValueKind::PyModule(module) => module.borrow().get_attr_value("chain"),
        _ => None,
    };
    let chain_class = match chain.as_ref().map(Value::kind) {
        Some(ValueKind::PyClass(class)) => Some(Rc::clone(class)),
        _ => None,
    };
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    // Match CPython 3.12: `collections` imports `_collections_abc`, not
    // `collections.abc`, so plain `import collections` does not bind
    // `collections.abc` or populate `sys.modules["collections.abc"]`.
    let abc_module = interp.load_module("_collections_abc")?;
    if let ValueKind::PyModule(abc_mod) = abc_module.kind() {
        let abc_attrs = abc_mod.borrow();
        for (src_name, dst_name) in [
            ("Mapping", "_Mapping"),
            ("MutableMapping", "_MutableMapping"),
            ("MutableSequence", "_MutableSequence"),
            ("Sequence", "_Sequence"),
        ] {
            if let Some(val) = abc_attrs.attrs.get(src_name).cloned() {
                ns.dict_insert(PyKey::str_from(dst_name), val)?;
            }
        }
    }
    if let Some(helper) = module
        .borrow()
        .attrs
        .get("_ordered_dict_move_to_end")
        .cloned()
    {
        ns.dict_insert(PyKey::str_from("_ordered_dict_move_to_end"), helper)?;
    }
    interp.exec_source(COLLECTIONS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("collections: exec namespace not a dict".into()))?;
    for name in COLLECTIONS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    tag_public_classes(module, chain_class.as_ref());
    // `_collections_abc.MutableSequence.register(deque)`.
    let deque = module.borrow().attrs.get("deque").cloned();
    if let Some(deque) = deque
        && let ValueKind::PyClass(deque) = deque.kind()
    {
        super::collections_abc::register_virtual_mutable_sequence(interp, deque)?;
    }
    Ok(Some(ns.clone()))
}

/// Tag each public `collections` class with `__module__ = "collections"`,
/// PEP 585 `__class_getitem__`, canonical identities, and native descriptors.
fn tag_public_classes(
    module: &Rc<RefCell<crate::value::PyModule>>,
    chain_class: Option<&Rc<RefCell<PyClass>>>,
) {
    for (cls_name, class_getitem_dispatch) in COLLECTION_CLASS_GETITEM_DISPATCH {
        let cls = module.borrow().attrs.get(cls_name).cloned();
        if let Some(cls_val) = cls
            && let ValueKind::PyClass(cls_rc) = cls_val.kind()
        {
            let mut class = cls_rc.borrow_mut();
            class
                .attrs
                .insert("__module__".to_string(), Value::string("collections"));
            class.attrs.insert(
                "__class_getitem__".to_string(),
                Value::builtin_function(class_getitem_dispatch),
            );
            class.error_name = match cls_name {
                "OrderedDict" => Some("collections.OrderedDict"),
                "defaultdict" => Some("collections.defaultdict"),
                "deque" => Some("collections.deque"),
                _ => None,
            };
            drop(class);
            if cls_name == "OrderedDict" {
                pyrust_builtins::ordered_mapping::register_class(cls_rc);
            }
            match cls_name {
                "Counter" => {
                    let fromkeys = pyrust_builtins::classmethod::native_class_method_descriptor(
                        Value::builtin_function("collections._counter_fromkeys"),
                        cls_rc,
                        "fromkeys",
                    );
                    cls_rc
                        .borrow_mut()
                        .attrs
                        .insert("fromkeys".to_string(), fromkeys);
                    register_canonical_collection_class(CanonicalCollectionKind::Counter, cls_rc);
                    if let Some(chain) = chain_class {
                        register_counter_chain_class(cls_rc, chain);
                    }
                }
                "deque" => {
                    {
                        let mut class = cls_rc.borrow_mut();
                        class.slots = Some(indexmap::IndexSet::new());
                        class.attrs.insert(
                            "maxlen".to_string(),
                            pyrust_builtins::property::property(
                                Value::builtin_function("collections.deque._get_maxlen"),
                                Value::builtin_function("collections._deque_readonly_maxlen"),
                                Value::builtin_function("collections._deque_readonly_maxlen"),
                            ),
                        );
                    }
                    register_canonical_collection_class(CanonicalCollectionKind::Deque, cls_rc);
                }
                "defaultdict" => {
                    {
                        let mut class = cls_rc.borrow_mut();
                        class.slots = Some(indexmap::IndexSet::new());
                        class.attrs.insert(
                            "default_factory".to_string(),
                            pyrust_builtins::property::property(
                                Value::builtin_function(
                                    "collections.defaultdict._get_default_factory",
                                ),
                                Value::builtin_function(
                                    "collections.defaultdict._set_default_factory",
                                ),
                                Value::builtin_function(
                                    "collections.defaultdict._del_default_factory",
                                ),
                            ),
                        );
                    }
                    register_canonical_collection_class(
                        CanonicalCollectionKind::DefaultDict,
                        cls_rc,
                    );
                }
                _ => {}
            }
        }
    }
}

fn native_deque_iterator_new(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    kind: NativeIteratorClass,
) -> Result<Value> {
    let class = args
        .first()
        .filter(|arg| arg.name.is_none())
        .and_then(|arg| match arg.value.kind() {
            ValueKind::PyClass(class) => Some(class),
            _ => None,
        });
    if class.and_then(NativeIteratorClass::from_class) != Some(kind) {
        return Err(pyrust_core::type_err!("invalid native iterator class"));
    }
    deque_iterator_constructor(
        interp,
        args.get(1..).unwrap_or_default(),
        kind == NativeIteratorClass::DequeReverse,
    )
}

fn ordered_dict_move_to_end(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    if args.len() != 3 || args.iter().any(|arg| arg.name.is_some()) {
        return Err(PyError::runtime(
            "internal: OrderedDict.move_to_end bridge expected 3 positional arguments".to_string(),
        ));
    }

    let last = interp.truthy_value(&args[2].value)?;
    let key_value = args[1].value.clone();
    let backing = coerce_subclass_backing(&args[0].value, &[])
        .filter(Value::is_dict)
        .ok_or_else(|| {
            PyError::runtime(
                "internal: OrderedDict.move_to_end receiver has no dict backing".to_string(),
            )
        })?;
    let Some((from, _)) = interp.dict_lookup_value(&backing, &key_value)? else {
        return Err(PyError::key_error(key_value));
    };
    let len = backing.dict_len().ok_or_else(|| {
        PyError::runtime("internal: OrderedDict.move_to_end backing is not a dict".to_string())
    })?;
    let to = if last { len - 1 } else { 0 };
    backing.dict_move_index(from, to)?;
    Ok(Value::none())
}
