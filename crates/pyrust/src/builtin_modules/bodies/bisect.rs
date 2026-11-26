// `bisect` — 100% native Rust implementation of CPython 3.12's `bisect` module
// (issues #2784, #3310).
//
// CPython's `Lib/bisect.py` is a thin wrapper that re-exports the C
// accelerator `_bisect` (`Modules/_bisectmodule.c`); this file ports that C
// code directly.  All six public functions are declared in the
// `pyrust_module!` block below and share one binary-search core, so there is
// no Python-source injection.  `bisect` / `insort` are the CPython aliases of
// `bisect_right` / `insort_right`, wired in [`prepare_module`].
//
// Semantics preserved from `_bisectmodule.c`:
//  * `lo` / `hi` go through `__index__` (`TypeError` / `OverflowError`);
//    `hi=None` *and* `hi=-1` both mean `len(a)`.
//  * `key` is applied to each probed element, never to `x` for the
//    `bisect_*` functions; `insort_*` computes `key(x)` first.
//  * comparisons are always `<` (`x < key(a[mid])` / `key(a[mid]) < x`).
//  * `insort_*` inserts with `list.insert` — directly for an exact `list`,
//    through the `insert` attribute otherwise.
//
// Reference: <https://docs.python.org/3/library/bisect.html>

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyIndex, PyValue};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, lookup_value_special_method, value_type_name_str,
};
use crate::value::{Value, ValueKind};
use pyrust_derive::pyrust_module;

fn call_positional(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

/// `PyNumber_AsSsize_t(o, PyExc_OverflowError)` for `lo` / `hi`.
fn index_to_ssize(idx: &PyIndex) -> Result<i64> {
    idx.as_isize().map(|n| n as i64).ok_or_else(|| {
        PyError::named(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

/// `PySequence_Size(a)`.
fn sequence_len(interp: &mut Interpreter, a: &Value) -> Result<i64> {
    if let Some(items) = a.as_list() {
        return Ok(items.len() as i64);
    }
    // `dict` has `mp_length` but no `sq_length`.
    if matches!(a.kind(), ValueKind::Dict(_)) {
        return Err(PyError::named("TypeError", "dict is not a sequence"));
    }
    let res = call_positional(
        interp,
        Value::builtin_function("len"),
        std::slice::from_ref(a),
    )?;
    let n = res
        .as_int()
        .ok_or_else(|| PyError::named("TypeError", "len() did not return an integer"))?;
    Ok(n)
}

/// CPython 3.12 probes `PySequence_GetItem`'s slot before the search loop, so
/// a non-sequence fails even when the range is empty.
fn check_sequence_item(interp: &mut Interpreter, a: &Value) -> Result<()> {
    let supported = match a.kind() {
        ValueKind::List(_)
        | ValueKind::Tuple(_)
        | ValueKind::Str(_)
        | ValueKind::Bytes(_)
        | ValueKind::Range { .. }
        | ValueKind::BigRange { .. } => true,
        // `dict` has `mp_subscript` but no `sq_item`.
        ValueKind::Dict(_) => {
            return Err(PyError::named("TypeError", "dict is not a sequence"));
        }
        ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
            lookup_value_special_method(a, "__getitem__").is_some()
        }
        _ => match interp.get_attr(a, "__getitem__") {
            Ok(_) => true,
            Err(err) if err.class_name_is("AttributeError") => false,
            Err(err) => return Err(err),
        },
    };
    if supported {
        Ok(())
    } else {
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object does not support indexing",
                value_type_name_str(a)
            ),
        ))
    }
}

/// `PySequence_GetItem(a, i)`; an exact `list` is read without dispatch.
fn sequence_item(interp: &mut Interpreter, a: &Value, i: i64) -> Result<Value> {
    if let Some(items) = a.as_list()
        && let Some(item) = usize::try_from(i).ok().and_then(|i| items.get(i))
    {
        return Ok(item.clone());
    }
    interp.eval_index(a, Value::int(i)).map_err(|err| {
        // `PySequence_GetItem` has no mapping fallback, so objects without a
        // sequence `__getitem__` report "does not support indexing".
        match not_subscriptable_type(&err) {
            Some(type_name) => PyError::named(
                "TypeError",
                format!("'{type_name}' object does not support indexing"),
            ),
            None => err,
        }
    })
}

/// The `X` of a `'X' object is not subscriptable` `TypeError`, if `err` is one.
fn not_subscriptable_type(err: &PyError) -> Option<String> {
    if !err.class_name_is("TypeError") {
        return None;
    }
    let msg = err.to_string();
    let rest = msg.rsplit_once("'")?.0;
    let name = rest.rsplit_once("'")?.1;
    msg.ends_with("' object is not subscriptable")
        .then(|| name.to_string())
}

/// `PyObject_RichCompareBool(lhs, rhs, Py_LT)`.
fn py_lt(interp: &mut Interpreter, lhs: &Value, rhs: &Value) -> Result<bool> {
    let res = interp.eval_binary(lhs.clone(), BinaryOp::Lt, rhs.clone())?;
    interp.truthy_value(&res)
}

/// `internal_bisect_left` / `internal_bisect_right`.
///
/// `hi` is the already-converted argument: `None` and `-1` both select
/// `len(a)`, mirroring `_Py_convert_optional_to_ssize_t`'s `-1` default.
fn internal_bisect(
    interp: &mut Interpreter,
    a: &Value,
    x: &Value,
    lo: i64,
    hi: Option<i64>,
    key: Option<&Value>,
    right: bool,
) -> Result<i64> {
    if lo < 0 {
        return Err(PyError::named("ValueError", "lo must be non-negative"));
    }
    let mut lo = lo;
    let mut hi = match hi {
        Some(h) if h != -1 => h,
        _ => sequence_len(interp, a)?,
    };
    check_sequence_item(interp, a)?;
    while lo < hi {
        // `lo` and `hi` are non-negative, so the u64 sum cannot overflow.
        let mid = ((lo as u64 + hi as u64) / 2) as i64;
        let mut item = sequence_item(interp, a, mid)?;
        if let Some(key) = key {
            item = call_positional(interp, key.clone(), &[item])?;
        }
        let go_right = if right {
            !py_lt(interp, x, &item)?
        } else {
            py_lt(interp, &item, x)?
        };
        if go_right {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

/// `_Py_convert_optional_to_ssize_t` for `hi`: `None` keeps the default.
fn optional_hi(interp: &mut Interpreter, hi: &Option<PyValue>) -> Result<Option<i64>> {
    let Some(hi) = hi else {
        return Ok(None);
    };
    if hi.0.is_none() {
        return Ok(None);
    }
    let Some(resolved) = interp.try_value_to_index(&hi.0)? else {
        return Err(PyError::named(
            "TypeError",
            format!(
                "argument should be integer or None, not '{}'",
                value_type_name_str(&hi.0)
            ),
        ));
    };
    let idx = PyIndex::try_from_value(&resolved, "bisect", "hi")?;
    idx.as_isize().map(|n| Some(n as i64)).ok_or_else(|| {
        PyError::named(
            "OverflowError",
            "cannot fit 'int' into an index-sized integer",
        )
    })
}

/// Resolve the shared `(lo, hi, key)` arguments of every public function.
fn resolve_bounds(
    interp: &mut Interpreter,
    lo: &PyIndex,
    hi: &Option<PyValue>,
    key: &Option<PyValue>,
) -> Result<(i64, Option<i64>, Option<Value>)> {
    let lo = index_to_ssize(lo)?;
    let hi = optional_hi(interp, hi)?;
    let key = key.as_ref().map(|k| k.0.clone()).filter(|k| !k.is_none());
    Ok((lo, hi, key))
}

fn bisect(
    interp: &mut Interpreter,
    a: &Value,
    x: &Value,
    lo: &PyIndex,
    hi: &Option<PyValue>,
    key: &Option<PyValue>,
    right: bool,
) -> Result<Value> {
    let (lo, hi, key) = resolve_bounds(interp, lo, hi, key)?;
    let idx = internal_bisect(interp, a, x, lo, hi, key.as_ref(), right)?;
    Ok(Value::int(idx))
}

fn insort(
    interp: &mut Interpreter,
    a: &Value,
    x: &Value,
    lo: &PyIndex,
    hi: &Option<PyValue>,
    key: &Option<PyValue>,
    right: bool,
) -> Result<Value> {
    let (lo, hi, key) = resolve_bounds(interp, lo, hi, key)?;
    let idx = match &key {
        None => internal_bisect(interp, a, x, lo, hi, None, right)?,
        Some(key) => {
            let key_x = call_positional(interp, key.clone(), std::slice::from_ref(x))?;
            internal_bisect(interp, a, &key_x, lo, hi, Some(key), right)?
        }
    };
    if a.is_list() {
        a.list_insert(idx.max(0) as usize, x.clone())?;
    } else {
        let insert = interp.get_attr(a, "insert")?;
        call_positional(interp, insert, &[Value::int(idx), x.clone()])?;
    }
    Ok(Value::none())
}

/// Module-load wiring:
///  * `bisect` / `insort` are the CPython names for the `*_right` functions;
///    the aliases must be the very same function objects.
///  * CPython's functions live in the `_bisect` accelerator that `bisect.py`
///    star-imports, so their `__module__` is `"_bisect"`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    for name in ["bisect_left", "bisect_right", "insort_left", "insort_right"] {
        let func = mod_rc.borrow().attrs.get(name).cloned();
        if let Some(func) = func {
            // Best effort: a failure only leaves `__module__` as `"bisect"`.
            let _ = interp.assign_attr(func, "__module__", Value::string("_bisect"));
        }
    }
    for (alias, target) in [("bisect", "bisect_right"), ("insort", "insort_right")] {
        let func = mod_rc.borrow().attrs.get(target).cloned();
        if let Some(func) = func {
            mod_rc.borrow_mut().attrs.insert(alias.to_string(), func);
        }
    }
}

pyrust_module! {
    constants {
        "__doc__" => Value::string("Bisection algorithms."),
    }

    #[arity_style(clinic)]
    #[bare_name]
    fn bisect_right(
        a: PyValue,
        x: PyValue,
        #[default(PyIndex::from(0i64))] lo: PyIndex,
        #[default(None)] hi: Option<PyValue>,
        #[keyword_only] #[default(None)] key: Option<PyValue>,
    ) -> Result<Value> {
        bisect(_interp, &a.0, &x.0, &lo, &hi, &key, true)
    }

    #[arity_style(clinic)]
    #[bare_name]
    fn bisect_left(
        a: PyValue,
        x: PyValue,
        #[default(PyIndex::from(0i64))] lo: PyIndex,
        #[default(None)] hi: Option<PyValue>,
        #[keyword_only] #[default(None)] key: Option<PyValue>,
    ) -> Result<Value> {
        bisect(_interp, &a.0, &x.0, &lo, &hi, &key, false)
    }

    #[arity_style(clinic)]
    #[bare_name]
    fn insort_right(
        a: PyValue,
        x: PyValue,
        #[default(PyIndex::from(0i64))] lo: PyIndex,
        #[default(None)] hi: Option<PyValue>,
        #[keyword_only] #[default(None)] key: Option<PyValue>,
    ) -> Result<Value> {
        insort(_interp, &a.0, &x.0, &lo, &hi, &key, true)
    }

    #[arity_style(clinic)]
    #[bare_name]
    fn insort_left(
        a: PyValue,
        x: PyValue,
        #[default(PyIndex::from(0i64))] lo: PyIndex,
        #[default(None)] hi: Option<PyValue>,
        #[keyword_only] #[default(None)] key: Option<PyValue>,
    ) -> Result<Value> {
        insort(_interp, &a.0, &x.0, &lo, &hi, &key, false)
    }
}
