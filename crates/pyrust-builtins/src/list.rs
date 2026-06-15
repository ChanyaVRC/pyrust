use pyrust_core::{PyDict, PyError, Result, StrKey, Value, ValueKind, compare_values_via_registry};

use crate::mutable_sequence as ms;
use crate::sequence;

/// Canonical list of method names dispatched by `call`.
pub const METHODS: &[&str] = &[
    "__iter__", "index", "count", "append", "clear", "copy", "extend", "insert", "pop", "remove",
    "reverse", "sort",
];

/// Returns `true` if `method` is the name of a built-in `list` method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Returns `true` if `method` may need mutable interpreter access — i.e. it
/// can fire user-defined dunder methods (`__eq__` for `index`/`count`/`remove`,
/// the `key=` callable for `sort`).  The VM dispatcher queries this predicate to
/// decide whether to take the interpreter-aware slow path (`call_seq_index` /
/// `call_seq_count` / `call_seq_remove` / precomputed-key sort) or hand off
/// directly to the interpreter-free `call` below.  Single source of truth for
/// the carve-out (see `crates/pyrust-builtins/README.md`).
pub fn requires_interpreter(method: &str) -> bool {
    matches!(method, "sort" | "index" | "count" | "remove")
}

pub fn call(method: &str, receiver: &Value, args: Vec<Value>, kwargs: &PyDict) -> Result<Value> {
    match method {
        // Read-only sequence operations — borrow scoped to the call.
        "index" => receiver
            .list_with(|items| sequence::seq_index(items, &args, "list"))
            .ok_or_else(|| {
                PyError::named("TypeError", "list.index receiver is not a list".to_string())
            })?,
        "count" => receiver
            .list_with(|items| sequence::seq_count(items, &args, "list"))
            .ok_or_else(|| {
                PyError::named("TypeError", "list.count receiver is not a list".to_string())
            })?,
        // Mutable Sequence Operations — each ms::* takes &Value and
        // scopes its own borrow_mut().
        "append" => ms::append(receiver, args),
        "clear" => ms::clear(receiver, args),
        "copy" => ms::copy(receiver, args),
        "extend" => ms::extend(receiver, args),
        "insert" => ms::insert(receiver, args),
        "pop" => ms::pop(receiver, args),
        "remove" => ms::remove(receiver, args),
        "reverse" => ms::reverse(receiver, args),
        // List-specific
        "sort" => sort(receiver, &args, kwargs),
        // Intercepted upstream in vm.rs / calls.rs; sentinel for drift guard.
        "__iter__" => Err(PyError::named(
            "TypeError",
            "'list' __iter__ must be dispatched by the interpreter",
        )),
        _ => Err(PyError::named(
            "AttributeError",
            format!("'list' object has no attribute '{method}'"),
        )),
    }
}

fn sort(receiver: &Value, args: &[Value], kwargs: &PyDict) -> Result<Value> {
    let reverse_flag = extract_reverse(args, kwargs)?;
    sort_by_cmp(receiver, reverse_flag)
}

fn extract_reverse(args: &[Value], kwargs: &PyDict) -> Result<bool> {
    // StrKey probe (issue #506): zero-alloc borrowed-str lookup — no heap
    // allocation on every list.sort() call.
    Ok(
        match (
            args.first().map(|v| v.kind()),
            kwargs.get(&StrKey("reverse")).map(|v| v.kind()),
        ) {
            (_, Some(ValueKind::Bool(b))) => b,
            (_, Some(ValueKind::Int(0))) => false,
            (_, Some(v)) => {
                matches!(v, ValueKind::Int(n) if n != 0)
                    || matches!(v, ValueKind::Bool(true))
                    || matches!(v, ValueKind::Float(f) if f != 0.0)
            }
            (Some(ValueKind::Bool(b)), _) => b,
            _ => false,
        },
    )
}

/// Sort a key-less list with the `reverse` flag already resolved to a `bool`
/// by the interpreter (issue #2126).  The receiver-only `call("sort", …)` path
/// re-parses `reverse` via `extract_reverse`, which recognises only
/// Bool/Int/Float; CPython applies `bool(reverse)` to any object.  The
/// interpreter computes that truthiness (honouring user `__bool__`) and calls
/// here directly, matching `sorted()`.
pub fn sort_no_key(receiver: &Value, reverse: bool) -> Result<Value> {
    sort_by_cmp(receiver, reverse)
}

fn sort_by_cmp(receiver: &Value, reverse: bool) -> Result<Value> {
    // Snapshot the items into an owned Vec.  The comparator may call
    // user `__lt__` which can re-enter the same list — by working on
    // a snapshot we keep the receiver's borrow unscoped during the
    // sort, then write the result back inside a `list_with_mut`
    // borrow_mut window.  Matches the previous `items.clone() →
    // sort_by → restore on err` shape.
    let mut snapshot = receiver.list_with(|items| items.clone()).ok_or_else(|| {
        PyError::named("TypeError", "list.sort receiver is not a list".to_string())
    })?;
    let mut err: Option<PyError> = None;
    snapshot.sort_by(|a, b| {
        if err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
        match compare_values_via_registry(lhs, rhs) {
            Ok(ord) => ord,
            Err(e) => {
                err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    receiver.list_with_mut(|items| *items = snapshot);
    Ok(Value::none())
}

/// Sort items using precomputed keys (one key per item in the same order).
/// Called by the VM after evaluating each key function call.
pub fn sort_with_precomputed_keys(
    receiver: &Value,
    keys: Vec<Value>,
    reverse: bool,
) -> Result<Value> {
    let snapshot = receiver.list_with(|items| items.clone()).ok_or_else(|| {
        PyError::named("TypeError", "list.sort receiver is not a list".to_string())
    })?;
    debug_assert_eq!(snapshot.len(), keys.len());
    let mut keyed: Vec<(Value, Value)> = keys.into_iter().zip(snapshot).collect();
    let mut sort_err: Option<PyError> = None;
    keyed.sort_by(|(ka, _), (kb, _)| {
        if sort_err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        let (lhs, rhs) = if reverse { (kb, ka) } else { (ka, kb) };
        match compare_values_via_registry(lhs, rhs) {
            Ok(ord) => ord,
            Err(e) => {
                sort_err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = sort_err {
        return Err(e);
    }
    let new_items: Vec<Value> = keyed.into_iter().map(|(_, v)| v).collect();
    receiver.list_with_mut(|items| *items = new_items);
    Ok(Value::none())
}
