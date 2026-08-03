use pyrust_core::{PyKey, ValueKind};

use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: set([iterable]) — set constructor.
    /// <https://docs.python.org/3/library/functions.html#func-set>
    ///
    /// Not migrated in this batch — same arity-split constraint as `list`.
    ///
    /// This can run user code by dispatching `__hash__` via
    /// `value_to_pykey` when building the set, and `__eq__` via `set_insert`.
    fn set(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::set(PySet::default())),
            1 => {
                if let ValueKind::Set(source) = args[0].value.kind() {
                    return Ok(Value::set(PySet::cpython_merged_copy(&source)));
                }
                if let Some(source) = pyrust_builtins::frozenset::as_items(&args[0].value) {
                    return Ok(Value::set(PySet::cpython_merged_copy(&source)));
                }
                let dict_keys = match args[0].value.kind() {
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
                let items = _interp.collect_iterable(&args[0].value)?;
                let mut set: PySet = PySet::default();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                Ok(Value::set(set))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 1 argument, got {}", args.len()),
            )),
        }
    }

    /// CPython: frozenset([iterable]) — frozenset constructor.
    /// <https://docs.python.org/3/library/functions.html#func-frozenset>
    ///
    /// Not migrated in this batch — same arity-split constraint as `list`.
    ///
    /// This can run user code by dispatching `__hash__` via
    /// `value_to_pykey` when building the set, and `__eq__` via `set_insert`.
    fn frozenset(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(pyrust_builtins::frozenset::frozenset(PySet::default())),
            1 => {
                // frozenset(frozenset_instance) returns the same object (per CPython).
                if pyrust_builtins::frozenset::as_items(&args[0].value).is_some() {
                    return Ok(args[0].value.clone());
                }
                if let ValueKind::Set(source) = args[0].value.kind() {
                    return Ok(pyrust_builtins::frozenset::frozenset(
                        PySet::cpython_merged_copy(&source),
                    ));
                }
                let dict_keys = match args[0].value.kind() {
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
                let items = _interp.collect_iterable(&args[0].value)?;
                let mut set: PySet = PySet::default();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                Ok(pyrust_builtins::frozenset::frozenset(set))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 1 argument, got {}", args.len()),
            )),
        }
    }

}
