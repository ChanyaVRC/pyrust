use indexmap::IndexMap;
use pyrust_core::{PyError, PyKey, Result, Value, ValueKind};

pub fn call(
    method: &str,
    dict: &mut IndexMap<PyKey, Value>,
    args: &[Value],
) -> Result<Value> {
    match method {
        "get" => get(dict, args),
        "keys" => Ok(Value::list(
            dict.keys().cloned().map(key_to_value).collect(),
        )),
        "values" => Ok(Value::list(dict.values().cloned().collect())),
        "items" => Ok(Value::list(
            dict.iter()
                .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                .collect(),
        )),
        "update" => update(dict, args),
        "pop" => pop(dict, args),
        "popitem" => popitem(dict),
        "clear" => {
            dict.clear();
            Ok(Value::none())
        }
        "setdefault" => setdefault(dict, args),
        "copy" => Ok(Value::dict(dict.clone())),
        _ => Err(PyError::Runtime(format!(
            "'dict' object has no attribute '{method}'"
        ))),
    }
}

fn get(dict: &IndexMap<PyKey, Value>, args: &[Value]) -> Result<Value> {
    let key = args.first().ok_or_else(|| {
        PyError::Runtime("dict.get() requires at least 1 argument".to_string())
    })?;
    let default = args.get(1).cloned().unwrap_or_else(Value::none);
    let pk = key.to_key().ok_or_else(|| {
        PyError::Runtime("unhashable type".to_string())
    })?;
    Ok(dict.get(&pk).cloned().unwrap_or(default))
}

fn update(dict: &mut IndexMap<PyKey, Value>, args: &[Value]) -> Result<Value> {
    let other = args.first().ok_or_else(|| {
        PyError::Runtime("dict.update() requires 1 argument".to_string())
    })?;
    match other.kind() {
        ValueKind::Dict(other_map) => {
            for (k, v) in other_map {
                dict.insert(k.clone(), v.clone());
            }
        }
        _ => {
            return Err(PyError::Runtime(
                "dict.update() argument must be a dict".to_string(),
            ));
        }
    }
    Ok(Value::none())
}

fn pop(dict: &mut IndexMap<PyKey, Value>, args: &[Value]) -> Result<Value> {
    let key = args.first().ok_or_else(|| {
        PyError::Runtime("dict.pop() requires at least 1 argument".to_string())
    })?;
    let pk = key.to_key().ok_or_else(|| {
        PyError::Runtime("unhashable type".to_string())
    })?;
    match dict.shift_remove(&pk) {
        Some(v) => Ok(v),
        None => {
            if let Some(default) = args.get(1) {
                Ok(default.clone())
            } else {
                Err(PyError::Runtime(format!("KeyError: {}", key.repr())))
            }
        }
    }
}


fn popitem(dict: &mut IndexMap<PyKey, Value>) -> Result<Value> {
    match dict.pop() {
        Some((k, v)) => Ok(Value::tuple(vec![key_to_value(k), v])),
        None => Err(PyError::Runtime("dictionary is empty".to_string())),
    }
}

fn setdefault(dict: &mut IndexMap<PyKey, Value>, args: &[Value]) -> Result<Value> {
    let key = args.first().ok_or_else(|| {
        PyError::Runtime("dict.setdefault() requires at least 1 argument".to_string())
    })?;
    let default = args.get(1).cloned().unwrap_or_else(Value::none);
    let pk = key.to_key().ok_or_else(|| {
        PyError::Runtime("unhashable type".to_string())
    })?;
    Ok(dict.entry(pk).or_insert(default).clone())
}

fn key_to_value(k: PyKey) -> Value {
    match k {
        PyKey::Int(v) => Value::int(v),
        PyKey::Float(bits) => Value::float(f64::from_bits(bits)),
        PyKey::Str(s) => Value::string(s),
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
    }
}
