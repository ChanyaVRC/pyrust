use pyrust_core::{PyKey, ValueKind};

use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: set([iterable]) — set constructor.
    /// <https://docs.python.org/3/library/functions.html#func-set>
    ///
    /// Migrated to the typed-signature dialect (#400): `OptValueRef` keeps
    /// omitted `set()` distinct from `set(None)` while enabling the vectorcall
    /// fast entry.
    ///
    /// This can run user code by dispatching `__hash__` via
    /// `value_to_pykey` when building the set, and `__eq__` via `set_insert`.
    #[arity_style(expected_got)]
    fn set(
        #[positional_only] #[default(OptionalArg::Omitted)] iterable: OptValueRef,
    ) -> Result<Value> {
        match iterable.as_value() {
            None => Ok(Value::set(PySet::default())),
            Some(arg) => {
                if let ValueKind::Set(source) = arg.kind() {
                    return Ok(Value::set(PySet::cpython_merged_copy(&source)));
                }
                if let Some(source) = pyrust_builtins::frozenset::as_items(arg) {
                    return Ok(Value::set(PySet::cpython_merged_copy(&source)));
                }
                let dict_keys = match arg.kind() {
                    ValueKind::Dict(source) => {
                        Some(source.keys().cloned().collect::<Vec<PyKey>>())
                    }
                    _ => None,
                };
                if let Some(keys) = dict_keys {
                    let mut set = PySet::with_cpython_dict_capacity(keys.len());
                    for key in keys {
                        _interp.set_insert(&mut set, key)?;
                    }
                    return Ok(Value::set(set));
                }
                let items = _interp.collect_iterable(arg)?;
                let mut set: PySet = PySet::default();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                Ok(Value::set(set))
            }
        }
    }

    /// CPython: frozenset([iterable]) — frozenset constructor.
    /// <https://docs.python.org/3/library/functions.html#func-frozenset>
    ///
    /// Migrated to the typed-signature dialect (#400) — same `OptValueRef`
    /// pattern as `set`.
    ///
    /// This can run user code by dispatching `__hash__` via
    /// `value_to_pykey` when building the set, and `__eq__` via `set_insert`.
    #[arity_style(expected_got)]
    fn frozenset(
        #[positional_only] #[default(OptionalArg::Omitted)] iterable: OptValueRef,
    ) -> Result<Value> {
        match iterable.as_value() {
            None => Ok(pyrust_builtins::frozenset::frozenset(PySet::default())),
            Some(arg) => {
                // frozenset(frozenset_instance) returns the same object (per CPython).
                if pyrust_builtins::frozenset::as_items(arg).is_some() {
                    return Ok(arg.clone());
                }
                if let ValueKind::Set(source) = arg.kind() {
                    return Ok(pyrust_builtins::frozenset::frozenset(
                        PySet::cpython_merged_copy(&source),
                    ));
                }
                let dict_keys = match arg.kind() {
                    ValueKind::Dict(source) => {
                        Some(source.keys().cloned().collect::<Vec<PyKey>>())
                    }
                    _ => None,
                };
                if let Some(keys) = dict_keys {
                    let mut set = PySet::with_cpython_dict_capacity(keys.len());
                    for key in keys {
                        _interp.set_insert(&mut set, key)?;
                    }
                    return Ok(pyrust_builtins::frozenset::frozenset(set));
                }
                let items = _interp.collect_iterable(arg)?;
                let mut set: PySet = PySet::default();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                Ok(pyrust_builtins::frozenset::frozenset(set))
            }
        }
    }

}
