// `copy` module registration facade — `copy.copy` (shallow copy) and
// `copy.deepcopy` (deep copy).
//
// Private submodules split the implementation by responsibility:
//
// - `memo`: object identity, memo helpers, `copy.Error` generations, depth guard;
// - `protocol`: `__reduce_ex__`/`__reduce__`, `__getstate__`/`__setstate__`, exceptions;
// - `reconstruct`: reduction tuple & iterator reconstruction helpers;
// - `shallow`: `copy.copy` container fast paths and default instance protocol;
// - `deep`: `copy.deepcopy` traversal for containers, storage, iterators, instances;
// - `deep_protocol`: default instance deep-copy backing/slot/memo ordering (#2968).
//
// Reference: <https://docs.python.org/3/library/copy.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::{PyDict, PyInstance, PyKey, PyModule, Value};
use pyrust_derive::pyrust_module;

#[path = "copy/deep.rs"]
mod deep;
#[path = "copy/deep_protocol.rs"]
mod deep_protocol;
#[path = "copy/memo.rs"]
mod memo;
#[path = "copy/protocol.rs"]
mod protocol;
#[path = "copy/reconstruct.rs"]
mod reconstruct;
#[path = "copy/shallow.rs"]
mod shallow;

const COPY_PY_SOURCE: &str = include_str!("copy_py.py");

/// Bind the native `copy` and `deepcopy` fast paths to this import
/// generation's private Python reconstruction helpers. The helpers stay out of
/// the public module namespace and are supplied as hidden receivers on each
/// call.
pub(crate) fn inject_python_members(
    interp: &mut crate::interpreter::Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    interp.exec_source(COPY_PY_SOURCE, Some(ns.clone()), None)?;
    let helper_shallow = ns
        .dict_with(|dict| dict.get(&PyKey::str_from("_reconstruct_shallow")).cloned())
        .flatten()
        .ok_or_else(|| PyError::Runtime("copy: shallow reconstruction helper missing".into()))?;
    let helper_deep = ns
        .dict_with(|dict| dict.get(&PyKey::str_from("_reconstruct_deep")).cloned())
        .flatten()
        .ok_or_else(|| PyError::Runtime("copy: deep reconstruction helper missing".into()))?;
    let native_copy = module
        .borrow()
        .attrs
        .get("copy")
        .cloned()
        .ok_or_else(|| PyError::Runtime("copy: native copy function missing".into()))?;
    let native_deepcopy = module
        .borrow()
        .attrs
        .get("deepcopy")
        .cloned()
        .ok_or_else(|| PyError::Runtime("copy: native deepcopy function missing".into()))?;
    let copy = pyrust_builtins::native_builtin_callable::native_generation_builtin(
        native_copy,
        helper_shallow,
        "copy",
        "copy",
    );
    let deepcopy = pyrust_builtins::native_builtin_callable::native_generation_builtin(
        native_deepcopy,
        helper_deep,
        "deepcopy",
        "copy",
    );
    {
        let mut module_mut = module.borrow_mut();
        module_mut.insert_attr("copy".to_string(), copy);
        module_mut.insert_attr("deepcopy".to_string(), deepcopy);
    }
    Ok(Some(ns))
}

/// Snapshot the instance attributes and `__slots__` member slots used by the
/// default copy protocol / `object.__getstate__`, or return `None` when the
/// instance carries no state.
pub(crate) fn default_instance_state(rc: &Rc<RefCell<PyInstance>>) -> Value {
    crate::interpreter::default_instance_state(rc)
}

pyrust_module! {
    constants {
        // copy.Error — subclass of Exception, raised on deepcopy failures.
        "Error" => memo::new_copy_error_class_value(),
        // CPython's `copy.py` keeps a lowercase alias for backward
        // compatibility: `error = Error`.  Both names must resolve to the
        // same class so `copy.error is copy.Error` is `True`.
        "error" => memo::current_copy_error_class_value(),
    }

    /// CPython: copy.copy(x) — shallow copy.
    fn copy(args) -> Result<Value> {
        // The injected public callable supplies its private reconstruction
        // helper as args[0]; only the remaining arguments are user-visible.
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() takes exactly 1 argument ({} given)",
                    args.len().saturating_sub(1)
                ),
            ));
        }
        let reconstruct = args[0].value.clone();
        let obj = args[1].value.clone();
        shallow::shallow_copy_with_reconstruct(obj, Some(&reconstruct), _interp)
    }

    /// CPython: copy.deepcopy(x, memo=None) — deep copy.
    fn deepcopy(args) -> Result<Value> {
        // The injected public callable supplies its private reconstruction
        // helper as args[0]; only the remaining arguments are user-visible.
        let reconstruct_deep = args[0].value.clone();
        let user_args = &args[1..];
        if user_args.is_empty() || user_args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() takes 1 or 2 arguments ({} given)",
                    user_args.len()
                ),
            ));
        }
        let obj = user_args[0].value.clone();
        let memo = if user_args.len() == 2 && !user_args[1].value.is_none() {
            user_args[1].value.clone()
        } else {
            Value::dict(PyDict::default())
        };
        deep::deep_copy_with_reconstruct(obj, &memo, Some(&reconstruct_deep), _interp)
    }
}
