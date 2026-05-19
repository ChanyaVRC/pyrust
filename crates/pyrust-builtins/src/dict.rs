use indexmap::IndexMap;
use pyrust_core::{PyError, PyKey, Result, Value, ValueKind};

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

/// Dispatch a `dict` method.  Receiver is `&Value`; each branch
/// opens a scoped `dict_with` / `dict_with_mut` borrow.  Iterating
/// methods (`update`) snapshot the mapping arg via the receiver's
/// own scoped borrow when the arg aliases the receiver, so
/// `d.update(d)` never simultaneously borrows the same `IndexMap`
/// (#448).
pub fn call(method: &str, receiver: &Value, args: Vec<Value>) -> Result<Value> {
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
        // length-2 sequence.  Factored out so the `List` and `Tuple`
        // arms below can share it without combining their patterns
        // (List now carries `Ref<'_, Vec<Value>>`, Tuple still carries
        // `&[Value]` — they can't bind to the same variable post-#450).
        fn push_pair(pair: &Value, out: &mut Vec<(PyKey, Value)>) -> Result<()> {
            let kv: Vec<Value> = match pair.kind() {
                ValueKind::List(items) if items.len() == 2 => items.clone(),
                ValueKind::Tuple(items) if items.len() == 2 => items.to_vec(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "dict.update() element must be a (key, value) pair".to_string(),
                    ));
                }
            };
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
                for pair in items.iter() {
                    push_pair(pair, &mut out)?;
                }
            }
            ValueKind::Tuple(items) => {
                for pair in items {
                    push_pair(pair, &mut out)?;
                }
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "dict.update() argument must be a dict or iterable of pairs".to_string(),
                ));
            }
        }
    }
    Ok(out)
}

fn popitem(dict: &mut IndexMap<PyKey, Value>) -> Result<Value> {
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
        PyKey::Float(bits) => Value::float(f64::from_bits(bits)),
        PyKey::Str(s) => Value::string(s),
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
        PyKey::FrozenSet(items) => {
            let mut set = indexmap::IndexSet::new();
            for k in items {
                set.insert(k);
            }
            crate::frozenset::frozenset(set)
        }
        PyKey::Tuple(items) => Value::tuple(items.into_iter().map(key_to_value).collect()),
        PyKey::Object { value, .. } => value,
    }
}
