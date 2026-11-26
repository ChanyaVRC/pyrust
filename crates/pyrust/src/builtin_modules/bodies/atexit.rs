// `atexit` module — body for the `atexit` entry in `pyrust_builtin_modules!`
// (issue #3209).
//
// Stores registered exit callbacks per interpreter in
// `SharedFeatureStates::atexit` (shared across filesystem-import child
// interpreters via one `Rc`), executes them in LIFO order on normal
// interpreter shutdown or `SystemExit`, and matches CPython 3.12's `register`,
// `unregister`, `_run_exitfuncs`, `_clear`, and `_ncallbacks` semantics.
//
// Reference: <https://docs.python.org/3.12/library/atexit.html>

use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    AtexitCallback, ExpandedCallArg, is_builtin_callable_adapter, lookup_class_attr,
    reject_keyword_args_expanded,
};
use crate::value::{Value, ValueKind};
use pyrust_derive::pyrust_module;

fn value_is_callable(value: &Value) -> bool {
    match value.kind() {
        ValueKind::UserFunction(_)
        | ValueKind::BuiltinFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::PyClass(_) => true,
        ValueKind::BuiltinObject { .. } => is_builtin_callable_adapter(value),
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            lookup_class_attr(&class, "__call__").is_some()
        }
        _ => false,
    }
}

fn require_no_args(args: &[ExpandedCallArg], fn_name: &str, short_name: &str) -> Result<()> {
    reject_keyword_args_expanded(fn_name, args)?;
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{short_name}() takes no arguments ({} given)", args.len()),
        ));
    }
    Ok(())
}

pyrust_module! {
    /// Register a function to be executed upon normal interpreter termination.
    /// Signature: `register(func, /, *args, **kwargs) -> func`.
    fn register(args) -> Result<Value> {
        let Some(first) = args.first() else {
            return Err(PyError::named(
                "TypeError",
                "register() takes at least 1 argument (0 given)".to_string(),
            ));
        };
        if first.name.is_some() {
            return Err(PyError::named(
                "TypeError",
                "register() takes at least 1 argument (0 given)".to_string(),
            ));
        }
        let func = first.value.clone();
        if !value_is_callable(&func) {
            return Err(PyError::named(
                "TypeError",
                "the first argument must be callable".to_string(),
            ));
        }
        let cb_args = args[1..].to_vec();
        _interp.features().atexit_mut().push(Some(AtexitCallback {
                func: func.clone(),
                args: cb_args,
            }));
        Ok(func)
    }

    /// Remove all registered callbacks equal to `func` (via Python `==`).
    /// Signature: `unregister(func, /) -> None`.
    fn unregister(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "unregister() takes exactly one argument ({} given)",
                    args.len()
                ),
            ));
        }
        let target = args[0].value.clone();
        let mut index = 0;
        loop {
            let candidate = {
                let Some(state) = _interp.features().atexit_peek() else {
                    break;
                };
                if index >= state.len() {
                    break;
                }
                state[index].as_ref().map(|cb| cb.func.clone())
            };
            if let Some(cb_func) = candidate
                && _interp.values_richcompare_eq(&cb_func, &target)?
            {
                let mut state = _interp.features().atexit_mut();
                if index < state.len() {
                    state[index] = None;
                }
            }
            index += 1;
        }
        Ok(Value::none())
    }

    /// Run all registered exit functions in LIFO order.
    fn _run_exitfuncs(args) -> Result<Value> {
        require_no_args(args, FN_NAME, "_run_exitfuncs")?;
        _interp.run_atexit_callbacks();
        Ok(Value::none())
    }

    /// Clear the list of previously registered exit functions.
    fn _clear(args) -> Result<Value> {
        require_no_args(args, FN_NAME, "_clear")?;
        _interp.features().atexit_clear();
        Ok(Value::none())
    }

    /// Return the number of registered exit callback slots (matching CPython 3.12).
    fn _ncallbacks(args) -> Result<Value> {
        require_no_args(args, FN_NAME, "_ncallbacks")?;
        let count = _interp.features().atexit_len() as i64;
        Ok(Value::int(count))
    }
}
