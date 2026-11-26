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
use crate::value::{PyDict, PyInstance, Value, ValueKind};
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

const COPY_DOC: &str = "Generic (shallow and deep) copying operations.

Interface summary:

        import copy

        x = copy.copy(y)        # make a shallow copy of y
        x = copy.deepcopy(y)    # make a deep copy of y

For module specific errors, copy.Error is raised.

The difference between shallow and deep copying is only relevant for
compound objects (objects that contain other objects, like lists or
class instances).

- A shallow copy constructs a new compound object and then (to the
  extent possible) inserts *the same objects* into it that the
  original contains.

- A deep copy constructs a new compound object and then, recursively,
  inserts *copies* into it of the objects found in the original.

Two problems often exist with deep copy operations that don't exist
with shallow copy operations:

 a) recursive objects (compound objects that, directly or indirectly,
    contain a reference to themselves) may cause a recursive loop

 b) because deep copy copies *everything* it may copy too much, e.g.
    administrative data structures that should be shared even between
    copies

Python's deep copy operation avoids these problems by:

 a) keeping a table of objects already copied during the current
    copying pass

 b) letting user-defined classes override the copying operation or the
    set of components copied

This version does not copy types like module, class, function, method,
nor stack trace, stack frame, nor file, socket, window, nor any
similar types.

Classes can use the same interfaces to control copying that they use
to control pickling: they can define methods called __getinitargs__(),
__getstate__() and __setstate__().  See the documentation for module
\"pickle\" for information on these methods.
";

/// Wire the module attributes CPython's `copy.py` imports: `copyreg` and its
/// shared `dispatch_table`.
pub(crate) fn prepare_module(interp: &mut crate::interpreter::Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    let Ok(copyreg_mod) = interp.load_module("copyreg") else {
        return;
    };
    let dispatch_table = match copyreg_mod.kind() {
        ValueKind::PyModule(m) => m.borrow().get_attr_value("dispatch_table"),
        _ => None,
    };
    let mut module_mut = module_rc.borrow_mut();
    if let Some(dispatch_table) = dispatch_table {
        module_mut.insert_attr("dispatch_table".to_string(), dispatch_table);
    }
    module_mut.insert_attr("copyreg".to_string(), copyreg_mod);
}

/// Snapshot the instance attributes and `__slots__` member slots used by the
/// default copy protocol / `object.__getstate__`, or return `None` when the
/// instance carries no state.
pub(crate) fn default_instance_state(rc: &Rc<RefCell<PyInstance>>) -> Value {
    crate::interpreter::default_instance_state(rc)
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(COPY_DOC),
        "__all__" => Value::list(vec![
            Value::string("Error"),
            Value::string("copy"),
            Value::string("deepcopy"),
        ]),
        // copy.Error — subclass of Exception, raised on deepcopy failures.
        "Error" => memo::new_copy_error_class_value(),
        // CPython's `copy.py` keeps a lowercase alias for backward
        // compatibility: `error = Error`.  Both names must resolve to the
        // same class so `copy.error is copy.Error` is `True`.
        "error" => memo::current_copy_error_class_value(),
    }

    /// CPython: copy.copy(x) — shallow copy.
    fn copy(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [x] = bind_python_args("copy", &["x"], 1, args, &kwargs)?;
        shallow::shallow_copy(x.expect("required"), _interp)
    }

    /// CPython: copy.deepcopy(x, memo=None, _nil=[]) — deep copy.
    fn deepcopy(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let [x, memo, _nil] = bind_python_args("deepcopy", &["x", "memo", "_nil"], 1, args, &kwargs)?;
        let memo = match memo {
            Some(memo) if !memo.is_none() => memo,
            _ => Value::dict(PyDict::default()),
        };
        deep::deep_copy(x.expect("required"), &memo, _interp)
    }
}

/// Bind `args` / `kwargs` to the parameters of a pure-Python function the way
/// CPython does, reproducing its `TypeError` wording. The first `required`
/// parameters have no default; an omitted optional parameter yields `None`.
fn bind_python_args<const N: usize>(
    fn_name: &str,
    params: &[&str; N],
    required: usize,
    args: &[Value],
    kwargs: &indexmap::IndexMap<String, Value>,
) -> Result<[Option<Value>; N]> {
    if args.len() > N {
        let given = args.len();
        let takes = if required == N {
            format!("{N} positional argument{}", if N == 1 { "" } else { "s" })
        } else {
            format!("from {required} to {N} positional arguments")
        };
        let verb = if given == 1 { "was" } else { "were" };
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes {takes} but {given} {verb} given"),
        ));
    }
    let mut bound: [Option<Value>; N] = std::array::from_fn(|index| args.get(index).cloned());
    for (key, value) in kwargs {
        let Some(index) = params.iter().position(|name| name == key) else {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() got an unexpected keyword argument '{key}'"),
            ));
        };
        if bound[index].is_some() {
            return Err(PyError::named(
                "TypeError",
                format!("{fn_name}() got multiple values for argument '{key}'"),
            ));
        }
        bound[index] = Some(value.clone());
    }
    let missing: Vec<String> = params
        .iter()
        .zip(&bound)
        .take(required)
        .filter(|(_, value)| value.is_none())
        .map(|(name, _)| format!("'{name}'"))
        .collect();
    if !missing.is_empty() {
        let list = match missing.as_slice() {
            [only] => only.clone(),
            [first, last] => format!("{first} and {last}"),
            [init @ .., last] => format!("{}, and {last}", init.join(", ")),
            [] => unreachable!(),
        };
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() missing {} required positional argument{}: {list}",
                missing.len(),
                if missing.len() == 1 { "" } else { "s" }
            ),
        ));
    }
    Ok(bound)
}
