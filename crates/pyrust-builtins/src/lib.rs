pub mod bound_method;
pub mod dict;
pub mod dict_views;
pub mod file;
pub mod frozenset;
pub mod iter_helpers;
pub mod list;
pub mod mutable_sequence;
pub mod property;
pub mod sequence;
pub mod set;
pub mod string;
pub mod tuple;

/// Look up `BuiltinTypeOps` by stable type-name.  Installed in pyrust-core's
/// registry at interpreter startup so the VM can dispatch operations on
/// built-in objects whose Tier 1 variant has been eliminated.
pub fn lookup_ops(type_name: &str) -> Option<&'static dyn pyrust_core::BuiltinTypeOps> {
    match type_name {
        file::TYPE_NAME => Some(file::FILE_OPS),
        frozenset::TYPE_NAME => Some(frozenset::FROZENSET_OPS),
        iter_helpers::ENUMERATE_TYPE_NAME => Some(iter_helpers::ENUMERATE_OPS),
        iter_helpers::ZIP_TYPE_NAME => Some(iter_helpers::ZIP_OPS),
        iter_helpers::REVERSED_TYPE_NAME => Some(iter_helpers::REVERSED_OPS),
        dict_views::DICT_KEYS_TYPE_NAME => Some(dict_views::DICT_KEYS_OPS),
        dict_views::DICT_VALUES_TYPE_NAME => Some(dict_views::DICT_VALUES_OPS),
        dict_views::DICT_ITEMS_TYPE_NAME => Some(dict_views::DICT_ITEMS_OPS),
        property::TYPE_NAME => Some(property::PROPERTY_OPS),
        bound_method::TYPE_NAME => Some(bound_method::BOUND_METHOD_OPS),
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

    use indexmap::{IndexMap, IndexSet};
    use pyrust_core::{PyError, PyKey, Value};

    fn is_fallback(e: &PyError) -> bool {
        matches!(e, PyError::Runtime(msg) if msg.contains("has no attribute"))
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
            let mut items: Vec<Value> = Vec::new();
            let r = super::list::call(name, &mut items, vec![], &IndexMap::new());
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "list::call({name}) hit fallback: {e:?}");
            }
        }
    }

    #[test]
    fn dict_methods_dispatched() {
        for &name in super::dict::METHODS {
            let mut dict: IndexMap<PyKey, Value> = IndexMap::new();
            let r = super::dict::call(name, &mut dict, vec![]);
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
            let mut set: IndexSet<PyKey> = IndexSet::new();
            let r = super::set::call(name, &mut set, vec![]);
            if let Err(ref e) = r {
                assert!(!is_fallback(e), "set::call({name}) hit fallback: {e:?}");
            }
        }
    }
}
