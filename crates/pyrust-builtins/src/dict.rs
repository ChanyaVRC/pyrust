use indexmap::IndexMap;
use pyrust_core::{PyError, PyKey, Result, Value, ValueKind};

/// Canonical list of method names dispatched by `call`.
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
        "get" => receiver
            .dict_with(|dict| get(dict, args))
            .ok_or_else(not_dict)?,
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
        "pop" => receiver
            .dict_with_mut(|dict| pop(dict, args))
            .ok_or_else(not_dict)?,
        "popitem" => receiver.dict_with_mut(popitem).ok_or_else(not_dict)?,
        "clear" => {
            receiver.dict_clear()?;
            Ok(Value::none())
        }
        "setdefault" => receiver
            .dict_with_mut(|dict| setdefault(dict, args))
            .ok_or_else(not_dict)?,
        "copy" => receiver
            .dict_with(|dict| Value::dict(dict.clone()))
            .ok_or_else(not_dict),
        _ => Err(PyError::Runtime(format!(
            "'dict' object has no attribute '{method}'"
        ))),
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
        match arg.kind() {
            ValueKind::Dict(other_map) => {
                for (k, v) in other_map {
                    out.push((k.clone(), v.clone()));
                }
            }
            ValueKind::List(items) | ValueKind::Tuple(items) => {
                for pair in items {
                    match pair.kind() {
                        ValueKind::Tuple(kv) | ValueKind::List(kv) if kv.len() == 2 => {
                            let k = kv[0].to_key().ok_or_else(|| {
                                PyError::Runtime("dict.update(): key is not hashable".to_string())
                            })?;
                            out.push((k, kv[1].clone()));
                        }
                        _ => {
                            return Err(PyError::Runtime(
                                "dict.update() element must be a (key, value) pair".to_string(),
                            ));
                        }
                    }
                }
            }
            _ => {
                return Err(PyError::Runtime(
                    "dict.update() argument must be a dict or iterable of pairs".to_string(),
                ));
            }
        }
    }
    Ok(out)
}

fn get(dict: &IndexMap<PyKey, Value>, args: Vec<Value>) -> Result<Value> {
    let mut iter = args.into_iter();
    let key = iter
        .next()
        .ok_or_else(|| PyError::Runtime("dict.get() requires at least 1 argument".to_string()))?;
    let default = iter.next().unwrap_or_else(Value::none);
    let pk = key
        .to_key()
        .ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
    Ok(dict.get(&pk).cloned().unwrap_or(default))
}

fn pop(dict: &mut IndexMap<PyKey, Value>, args: Vec<Value>) -> Result<Value> {
    let mut iter = args.into_iter();
    let key = iter
        .next()
        .ok_or_else(|| PyError::Runtime("dict.pop() requires at least 1 argument".to_string()))?;
    let pk = key.to_key().ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!(
                "unhashable type: '{}'",
                pyrust_core::builtin_type_name(&key)
            ),
        )
    })?;
    match dict.shift_remove(&pk) {
        Some(v) => Ok(v),
        None => {
            if let Some(default) = iter.next() {
                Ok(default)
            } else {
                Err(PyError::named("KeyError", key.repr()))
            }
        }
    }
}

fn popitem(dict: &mut IndexMap<PyKey, Value>) -> Result<Value> {
    match dict.pop() {
        Some((k, v)) => Ok(Value::tuple(vec![key_to_value(k), v])),
        None => Err(PyError::named(
            "KeyError",
            "'popitem(): dictionary is empty'".to_string(),
        )),
    }
}

fn setdefault(dict: &mut IndexMap<PyKey, Value>, args: Vec<Value>) -> Result<Value> {
    let mut iter = args.into_iter();
    let key = iter.next().ok_or_else(|| {
        PyError::Runtime("dict.setdefault() requires at least 1 argument".to_string())
    })?;
    let default = iter.next().unwrap_or_else(Value::none);
    let pk = key
        .to_key()
        .ok_or_else(|| PyError::Runtime("unhashable type".to_string()))?;
    Ok(dict.entry(pk).or_insert(default).clone())
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
