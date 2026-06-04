pub mod bound_method;
pub mod bytearray;
pub mod bytes;
pub mod cached_property;
pub mod classmethod;
pub mod code;
pub mod complex;
pub mod dict;
pub mod dict_views;
pub mod file;
pub mod float;
pub mod frame;
pub mod frozenset;
pub mod generic_alias;
pub mod instance_dict;
pub mod int;
pub mod iter_helpers;
pub mod list;
pub mod mapping_proxy;
pub mod member_descriptor;
pub mod mutable_sequence;
pub mod numeric_attrs_descriptor;
pub mod property;
pub mod sequence;
pub mod set;
pub mod slice;
pub mod string;
pub mod super_bound_builtin;
pub mod traceback;
pub mod tuple;
pub mod unicode_data;
pub mod union_type;

/// Look up `BuiltinTypeOps` by stable type-name.  Installed in pyrust-core's
/// registry at interpreter startup so the VM can dispatch operations on
/// built-in objects whose Tier 1 variant has been eliminated.
pub fn lookup_ops(type_name: &str) -> Option<&'static dyn pyrust_core::BuiltinTypeOps> {
    match type_name {
        bytearray::TYPE_NAME => Some(bytearray::BYTEARRAY_OPS),
        file::TYPE_NAME => Some(file::FILE_OPS),
        frozenset::TYPE_NAME => Some(frozenset::FROZENSET_OPS),
        generic_alias::TYPE_NAME => Some(generic_alias::GENERIC_ALIAS_OPS),
        iter_helpers::ENUMERATE_TYPE_NAME => Some(iter_helpers::ENUMERATE_OPS),
        iter_helpers::ZIP_TYPE_NAME => Some(iter_helpers::ZIP_OPS),
        iter_helpers::REVERSED_TYPE_NAME => Some(iter_helpers::REVERSED_OPS),
        iter_helpers::CHAIN_TYPE_NAME => Some(iter_helpers::CHAIN_OPS),
        dict_views::DICT_KEYS_TYPE_NAME => Some(dict_views::DICT_KEYS_OPS),
        dict_views::DICT_VALUES_TYPE_NAME => Some(dict_views::DICT_VALUES_OPS),
        dict_views::DICT_ITEMS_TYPE_NAME => Some(dict_views::DICT_ITEMS_OPS),
        property::TYPE_NAME => Some(property::PROPERTY_OPS),
        cached_property::TYPE_NAME => Some(cached_property::CACHED_PROPERTY_OPS),
        bound_method::TYPE_NAME => Some(bound_method::BOUND_METHOD_OPS),
        instance_dict::TYPE_NAME => Some(instance_dict::INSTANCE_DICT_OPS),
        mapping_proxy::TYPE_NAME => Some(mapping_proxy::MAPPING_PROXY_OPS),
        slice::TYPE_NAME => Some(slice::SLICE_OPS),
        classmethod::CLASS_TYPE_NAME => Some(classmethod::CLASS_METHOD_ANY_OPS),
        classmethod::STATIC_TYPE_NAME => Some(classmethod::STATIC_METHOD_ANY_OPS),
        classmethod::CLASS_BINDER_TYPE_NAME => Some(classmethod::CLASS_METHOD_GET_BINDER_OPS),
        classmethod::STATIC_BINDER_TYPE_NAME => Some(classmethod::STATIC_METHOD_GET_BINDER_OPS),
        code::TYPE_NAME => Some(code::CODE_OPS),
        traceback::TYPE_NAME => Some(traceback::TRACEBACK_OPS),
        union_type::TYPE_NAME => Some(union_type::UNION_TYPE_OPS),
        numeric_attrs_descriptor::GETSET_TYPE_NAME => {
            Some(numeric_attrs_descriptor::GETSET_DESCRIPTOR_OPS)
        }
        numeric_attrs_descriptor::METHOD_DESCRIPTOR_TYPE_NAME => {
            Some(numeric_attrs_descriptor::METHOD_DESCRIPTOR_OPS)
        }
        _ => None,
    }
}

/// Install [`lookup_ops`] in pyrust-core's registry.  Idempotent — safe to
/// call from `Interpreter::default()`.
pub fn install() {
    pyrust_core::install_builtin_registry(lookup_ops);
}

#[cfg(test)]
mod method_table_drift_guard {
    //! Guards against the `METHODS` const slice and the `call()` match arm in
    //! each builtin module drifting apart. If `METHODS` lists a name that
    //! `call()` doesn't dispatch, `getattr`/`hasattr` lie to users; if `call()`
    //! dispatches a name that isn't in `METHODS`, `hasattr` lies the other way.
    //!
    //! For every name in each `METHODS` slice we invoke `call()` with empty
    //! args and assert the error (if any) is *not* the "has no attribute"
    //! fallback — argument-related errors are fine, they prove the dispatch
    //! reached the right arm.

    use indexmap::IndexMap;
    use pyrust_core::{PyDict, PyError, PySet, Value};

    fn is_fallback(e: &PyError) -> bool {
        matches!(e, PyError::Runtime(msg) if msg.contains("has no attribute"))
    }

    #[test]
    fn float_methods_dispatched() {
        for &name in super::float::METHODS {
            let r = super::float::call(name, 1.0_f64, &[]);
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "float::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn int_methods_dispatched() {
        let receiver = Value::int(5);
        for &name in super::int::METHODS {
            let r = super::int::call(name, &receiver, &[], &PyDict::default());
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "int::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn string_methods_dispatched() {
        let src = Value::string("");
        for &name in super::string::METHODS {
            let r = super::string::call(name, &src, vec![]);
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "string::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn list_methods_dispatched() {
        for &name in super::list::METHODS {
            let receiver = Value::list(Vec::new());
            let r = super::list::call(name, &receiver, vec![], &PyDict::default());
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "list::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn dict_methods_dispatched() {
        for &name in super::dict::METHODS {
            let receiver = Value::dict(PyDict::default());
            let r = super::dict::call(name, &receiver, vec![], &PyDict::default());
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "dict::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn tuple_methods_dispatched() {
        let items: Vec<Value> = Vec::new();
        for &name in super::tuple::METHODS {
            let r = super::tuple::call(name, &items, vec![]);
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "tuple::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn set_methods_dispatched() {
        for &name in super::set::METHODS {
            let receiver = Value::set(PySet::default());
            let r = super::set::call(name, &receiver, vec![]);
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "set::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn complex_methods_dispatched() {
        let receiver = Value::complex(0.0, 0.0);
        for &name in super::complex::METHODS {
            let r = super::complex::call(name, &receiver, vec![]);
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "complex::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn frozenset_methods_dispatched() {
        let receiver = super::frozenset::frozenset(PySet::default());
        for &name in super::frozenset::METHODS {
            let r = super::frozenset::call(name, &receiver, vec![]);
            if let Err(ref e) = r {
                assert!(
                    !is_fallback(e),
                    "frozenset::call({name}) hit fallback: {e:?}"
                );
            }
        }
    }

    #[test]
    fn bytes_methods_dispatched() {
        let receiver = Value::bytes(vec![]);
        for &name in super::bytes::METHODS {
            let r = super::bytes::call(name, &receiver, &[], &PyDict::default());
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "bytes::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn bytearray_methods_dispatched() {
        use pyrust_core::BuiltinTypeOps;
        let receiver = super::bytearray::bytearray(vec![]);
        for &name in super::bytearray::METHODS {
            if name == "__iter__" || name == "fromhex" {
                // __iter__ is handled at the interpreter level; fromhex is a classmethod.
                continue;
            }
            let r = super::bytearray::BYTEARRAY_OPS.call_method(
                match receiver.kind() {
                    pyrust_core::ValueKind::BuiltinObject { state, .. } => state,
                    _ => panic!("expected BuiltinObject"),
                },
                name,
                vec![],
                &IndexMap::new(),
            );
            if let Err(ref e) = r {
                assert!(
                    !is_fallback(e),
                    "bytearray::call_method({name}) hit fallback: {e:?}"
                );
            }
        }
    }
}

#[cfg(test)]
mod cross_dispatch_tests {
    //! Regression tests for the BuiltinTypeOps dispatch paths added in #291.

    use pyrust_core::{PyKey, PySet, Value};

    #[test]
    fn set_eq_frozenset_dispatches_through_ops() {
        // `set == frozenset` must route through `BuiltinTypeOps::eq` on the
        // frozenset side so pyrust-core never needs to name the frozenset type.
        let mut s: PySet = PySet::default();
        s.insert(PyKey::Int(1));
        s.insert(PyKey::Int(2));

        let mut fs_items: PySet = PySet::default();
        fs_items.insert(PyKey::Int(2));
        fs_items.insert(PyKey::Int(1));

        let set_val = Value::set(s.clone());
        let frozen_val = super::frozenset::frozenset(fs_items);

        assert_eq!(set_val, frozen_val);
        assert_eq!(frozen_val, set_val);
    }

    #[test]
    fn frozenset_eq_frozenset_uses_rc_fastpath() {
        // Two frozensets sharing the same backing Rc should compare equal
        // via the Rc::ptr_eq fast path inside FrozenSetOps::eq.
        let mut items: PySet = PySet::default();
        items.insert(PyKey::Int(42));
        let rc = std::rc::Rc::new(items);

        let a = super::frozenset::frozenset_rc(rc.clone());
        let b = super::frozenset::frozenset_rc(rc);

        assert_eq!(a, b);
    }
}
