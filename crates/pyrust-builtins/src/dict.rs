use pyrust_core::{PyDict, PyError, PyKey, Result, Value, ValueKind};

/// Canonical list of method names exposed for `dict`.
///
/// **Note** (#425): of these, `get`, `pop`, `setdefault`, and `__contains__`
/// are NOT dispatched by `call` below — `Interpreter::call_dict_method`
/// (`crates/pyrust/src/interpreter/runtime/expr.rs`) intercepts those four
/// before delegating, because they need to fire user-defined `__hash__` /
/// `__eq__` (#368) which an interpreter-free dispatcher can't do.
/// `has_method` still must report them (instance-attr `d.pop` resolution
/// goes through `builtin_has_method` → this list), so they stay listed
/// here.  The unreachable function bodies that used to live below them in
/// this file are gone (only the `match` arms in `call` were dead — see #425).
pub const METHODS: &[&str] = &[
    "__iter__",
    "get",
    "keys",
    "values",
    "items",
    "update",
    "pop",
    "popitem",
    "clear",
    "setdefault",
    "copy",
];

/// Returns `true` if `method` is the name of a built-in `dict` method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Returns `true` if `method` produces a *lazy view* (`keys` / `values` /
/// `items`) that must share the source dict's backing storage.
///
/// These three cannot go through `call` below — the interpreter-free
/// signature only sees a `Vec<Value>` snapshot, whereas a live view needs the
/// `Rc<RefCell<IndexMap>>`.  The VM dispatcher queries this predicate and
/// routes matching methods to `dict_views::dict_{keys,values,items}` instead.
/// Single source of truth for the carve-out (see
/// `crates/pyrust-builtins/README.md`).
pub fn needs_rc(method: &str) -> bool {
    matches!(method, "keys" | "values" | "items")
}

/// Dispatch a `dict` method.  Receiver is `&Value`; each branch
/// opens a scoped `dict_with` / `dict_with_mut` borrow.  Iterating
/// methods (`update`) snapshot the mapping arg via the receiver's
/// own scoped borrow when the arg aliases the receiver, so
/// `d.update(d)` never simultaneously borrows the same `IndexMap`
/// (#448).
pub fn call(method: &str, receiver: &Value, args: Vec<Value>, kwargs: &PyDict) -> Result<Value> {
    let not_dict = || {
        PyError::named(
            "TypeError",
            "dict method receiver is not a dict".to_string(),
        )
    };
    match method {
        "keys" => receiver
            .dict_with(|dict| Value::list(dict.keys().cloned().map(key_to_value).collect()))
            .ok_or_else(not_dict),
        "values" => receiver
            .dict_with(|dict| Value::list(dict.values().cloned().collect()))
            .ok_or_else(not_dict),
        "items" => receiver
            .dict_with(|dict| {
                Value::list(
                    dict.iter()
                        .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                        .collect(),
                )
            })
            .ok_or_else(not_dict),
        "update" => {
            // CPython: `dict.update([mapping_or_iterable], **kwargs)` —
            // at most one positional arg.  >1 positional → TypeError.
            if args.len() > 1 {
                return Err(PyError::named(
                    "TypeError",
                    format!("update expected at most 1 argument, got {}", args.len()),
                ));
            }
            // Materialise the mapping snapshot BEFORE borrow_mut so
            // a self-aliased call (`d.update(d)`) reads its pre-
            // update state and doesn't `&` the storage we'd `&mut`.
            let snapshot = snapshot_update_arg(receiver, &args)?;
            receiver
                .dict_with_mut(|dict| {
                    for (k, v) in snapshot {
                        dict.insert(k, v);
                    }
                    // Keyword arguments are inserted after the positional arg,
                    // matching CPython's order: positional mapping first, then kwargs.
                    for (k, v) in kwargs {
                        dict.insert(k.clone(), v.clone());
                    }
                })
                .ok_or_else(not_dict)?;
            Ok(Value::none())
        }
        "popitem" => receiver.dict_with_mut(popitem).ok_or_else(not_dict)?,
        "clear" => {
            receiver.dict_clear()?;
            Ok(Value::none())
        }
        "copy" => receiver
            .dict_with(|dict| Value::dict(dict.clone()))
            .ok_or_else(not_dict),
        _ => Err(PyError::named(
            "AttributeError",
            format!("'dict' object has no attribute '{method}'"),
        )),
    }
}

/// Materialise the `update()` argument(s) into `(PyKey, Value)`
/// pairs.  When the arg aliases the receiver we snapshot the
/// receiver's contents via its own scoped read borrow; otherwise we
/// drain the mapping arg directly.
fn snapshot_update_arg(receiver: &Value, args: &[Value]) -> Result<Vec<(PyKey, Value)>> {
    let mut out = Vec::new();
    for arg in args {
        let aliased = arg.value_id() == receiver.value_id() && arg.value_id().is_some();
        if aliased {
            let snap = receiver
                .dict_with(|dict| {
                    dict.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>()
                })
                .ok_or_else(|| {
                    PyError::named(
                        "TypeError",
                        "dict.update receiver is not a dict".to_string(),
                    )
                })?;
            out.extend(snap);
            continue;
        }
        // Helper: handle a single key/value pair from the iterable form
        // (`[(k1, v1), (k2, v2), ...]`).  Each `pair` must itself be a
        // length-2 sequence.  Factored out so the `List`, `Tuple`, and
        // `Str` arms below can share it without combining their patterns.
        // `idx` is the 0-based element position within the outer iterable,
        // used to match CPython's error messages.
        fn push_pair(pair: &Value, idx: usize, out: &mut Vec<(PyKey, Value)>) -> Result<()> {
            let (len, kv): (usize, Vec<Value>) = match pair.kind() {
                ValueKind::List(items) => (items.len(), items.clone()),
                ValueKind::Tuple(items) => (items.len(), items.to_vec()),
                ValueKind::Str(s) => {
                    let chars: Vec<Value> =
                        s.chars().map(|c| Value::string(c.to_string())).collect();
                    (chars.len(), chars)
                }
                _ => {
                    // Non-sequence element: CPython raises TypeError here.
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "cannot convert dictionary update sequence element #{idx} to a sequence"
                        ),
                    ));
                }
            };
            if len != 2 {
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "dictionary update sequence element #{idx} has length {len}; 2 is required"
                    ),
                ));
            }
            let k = kv[0].to_key().ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    format!(
                        "unhashable type: '{}'",
                        pyrust_core::builtin_type_name(&kv[0])
                    ),
                )
            })?;
            out.push((k, kv[1].clone()));
            Ok(())
        }
        match arg.kind() {
            ValueKind::Dict(other_map) => {
                for (k, v) in other_map.iter() {
                    out.push((k.clone(), v.clone()));
                }
            }
            ValueKind::List(items) => {
                for (idx, pair) in items.iter().enumerate() {
                    push_pair(pair, idx, &mut out)?;
                }
            }
            ValueKind::Tuple(items) => {
                for (idx, pair) in items.iter().enumerate() {
                    push_pair(pair, idx, &mut out)?;
                }
            }
            ValueKind::Str(s) => {
                // Strings are iterable (yield 1-char strings), but each
                // char is length 1, so push_pair will raise the
                // CPython-matching ValueError on element #0.
                for (idx, ch) in s.chars().enumerate() {
                    let char_val = Value::string(ch.to_string());
                    push_pair(&char_val, idx, &mut out)?;
                }
            }
            ValueKind::Bytes(rc) => {
                // Bytes are iterable (yield integers 0-255), but integers
                // are not sequences, so push_pair raises TypeError element #0.
                for (idx, b) in rc.iter().enumerate() {
                    let byte_val = Value::int(*b as i64);
                    push_pair(&byte_val, idx, &mut out)?;
                }
            }
            _ => {
                // Non-iterable argument: CPython propagates the TypeError
                // from the iterator protocol — `'X' object is not iterable`.
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object is not iterable",
                        pyrust_core::builtin_type_name(arg)
                    ),
                ));
            }
        }
    }
    Ok(out)
}

fn popitem(dict: &mut PyDict) -> Result<Value> {
    match dict.pop() {
        Some((k, v)) => Ok(Value::tuple(vec![key_to_value(k), v])),
        None => Err(PyError::named(
            "KeyError",
            "popitem(): dictionary is empty".to_string(),
        )),
    }
}

fn key_to_value(k: PyKey) -> Value {
    match k {
        PyKey::Int(v) => Value::int(v),
        PyKey::BigInt(v) => Value::bigint(*v),
        PyKey::Float(bits) => Value::float(f64::from_bits(bits)),
        PyKey::Str(s) => s,
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::FrozenSet(key) => crate::frozenset::frozenset_key(key),
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
        PyKey::Bytes(rc) => Value::bytes((*rc).clone()),
        PyKey::Complex(re, im) => Value::complex(re, im),
        PyKey::Object { value, .. } => value,
    }
}
