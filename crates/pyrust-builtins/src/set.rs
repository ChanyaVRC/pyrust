use indexmap::IndexSet;
use pyrust_core::{PyError, PyKey, Result, Value, ValueKind};

/// Returns `true` if `method` is the name of a built-in `set` method.
pub fn has_method(method: &str) -> bool {
    matches!(
        method,
        "add"
            | "remove"
            | "discard"
            | "pop"
            | "clear"
            | "update"
            | "intersection_update"
            | "difference_update"
            | "symmetric_difference_update"
            | "copy"
            | "union"
            | "intersection"
            | "difference"
            | "symmetric_difference"
            | "issubset"
            | "issuperset"
            | "isdisjoint"
    )
}

pub fn call(method: &str, items: &mut IndexSet<PyKey>, args: Vec<Value>) -> Result<Value> {
    let args = args.as_slice();
    match method {
        // Mutating
        "add" => add(items, args),
        "remove" => remove(items, args),
        "discard" => discard(items, args),
        "pop" => pop(items),
        "clear" => {
            items.clear();
            Ok(Value::none())
        }
        "update" => update(items, args),
        "intersection_update" => intersection_update(items, args),
        "difference_update" => difference_update(items, args),
        "symmetric_difference_update" => symmetric_difference_update(items, args),
        // Non-mutating
        "copy" => Ok(Value::set(items.clone())),
        "union" => union(items, args),
        "intersection" => intersection(items, args),
        "difference" => difference(items, args),
        "symmetric_difference" => symmetric_difference(items, args),
        "issubset" => issubset(items, args),
        "issuperset" => issuperset(items, args),
        "isdisjoint" => isdisjoint(items, args),
        _ => Err(PyError::Runtime(format!(
            "'set' object has no attribute '{method}'"
        ))),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn to_key(v: &Value) -> Result<PyKey> {
    v.to_key()
        .ok_or_else(|| PyError::Named("TypeError".to_string(), "unhashable type".to_string()))
}

/// Collect an iterable `Value` into a set of `PyKey`s.
fn collect_iterable(v: &Value) -> Result<IndexSet<PyKey>> {
    let mut out = IndexSet::new();
    match v.kind() {
        ValueKind::Set(s) => {
            for k in s {
                out.insert(k.clone());
            }
        }
        ValueKind::List(items) | ValueKind::Tuple(items) => {
            for item in items {
                out.insert(to_key(item)?);
            }
        }
        ValueKind::Dict(d) => {
            for k in d.keys() {
                out.insert(k.clone());
            }
        }
        ValueKind::Str(s) => {
            for ch in s.chars() {
                out.insert(PyKey::Str(ch.to_string()));
            }
        }
        ValueKind::Range { start, stop, step } => {
            if step != 0 {
                let mut cur = start;
                loop {
                    if step > 0 && cur >= stop {
                        break;
                    }
                    if step < 0 && cur <= stop {
                        break;
                    }
                    out.insert(PyKey::Int(cur));
                    cur = cur.wrapping_add(step);
                }
            }
        }
        _ => {
            return Err(PyError::Named(
                "TypeError".to_string(),
                format!("'{}' object is not iterable", type_name(v)),
            ));
        }
    }
    Ok(out)
}

fn type_name(v: &Value) -> &'static str {
    match v.kind() {
        ValueKind::Int(_) | ValueKind::BigInt(_) => "int",
        ValueKind::Float(_) => "float",
        ValueKind::Bool(_) => "bool",
        ValueKind::None => "NoneType",
        ValueKind::Str(_) => "str",
        ValueKind::List(_) => "list",
        ValueKind::Tuple(_) => "tuple",
        ValueKind::Set(_) => "set",
        ValueKind::Dict(_) => "dict",
        ValueKind::Range { .. } => "range",
        _ => "object",
    }
}

// ── mutating methods ──────────────────────────────────────────────────────────

fn add(items: &mut IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let elem = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.add() requires 1 argument".to_string()))?;
    items.insert(to_key(elem)?);
    Ok(Value::none())
}

fn remove(items: &mut IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let elem = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.remove() requires 1 argument".to_string()))?;
    let key = to_key(elem)?;
    if items.shift_remove(&key) {
        Ok(Value::none())
    } else {
        Err(PyError::Named("KeyError".to_string(), elem.repr()))
    }
}

fn discard(items: &mut IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let elem = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.discard() requires 1 argument".to_string()))?;
    let key = to_key(elem)?;
    items.shift_remove(&key);
    Ok(Value::none())
}

fn pop(items: &mut IndexSet<PyKey>) -> Result<Value> {
    match items.pop() {
        Some(k) => Ok(key_to_value(k)),
        None => Err(PyError::Named(
            "KeyError".to_string(),
            "pop from an empty set".to_string(),
        )),
    }
}

fn update(items: &mut IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    for arg in args {
        let other = collect_iterable(arg)?;
        for k in other {
            items.insert(k);
        }
    }
    Ok(Value::none())
}

fn intersection_update(items: &mut IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    for arg in args {
        let other = collect_iterable(arg)?;
        items.retain(|k| other.contains(k));
    }
    Ok(Value::none())
}

fn difference_update(items: &mut IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    for arg in args {
        let other = collect_iterable(arg)?;
        for k in &other {
            items.shift_remove(k);
        }
    }
    Ok(Value::none())
}

fn symmetric_difference_update(items: &mut IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| {
            PyError::Runtime("set.symmetric_difference_update() requires 1 argument".to_string())
        })
        .and_then(collect_iterable)?;
    // elements in other but not in self
    let mut to_add: Vec<PyKey> = Vec::new();
    for k in &other {
        if !items.contains(k) {
            to_add.push(k.clone());
        }
    }
    // remove elements that are in both
    items.retain(|k| !other.contains(k));
    for k in to_add {
        items.insert(k);
    }
    Ok(Value::none())
}

// ── non-mutating methods ──────────────────────────────────────────────────────

fn union(items: &IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let mut result = items.clone();
    for arg in args {
        let other = collect_iterable(arg)?;
        for k in other {
            result.insert(k);
        }
    }
    Ok(Value::set(result))
}

fn intersection(items: &IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let mut result = items.clone();
    for arg in args {
        let other = collect_iterable(arg)?;
        result.retain(|k| other.contains(k));
    }
    Ok(Value::set(result))
}

fn difference(items: &IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let mut result = items.clone();
    for arg in args {
        let other = collect_iterable(arg)?;
        for k in &other {
            result.shift_remove(k);
        }
    }
    Ok(Value::set(result))
}

fn symmetric_difference(items: &IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| {
            PyError::Runtime("set.symmetric_difference() requires 1 argument".to_string())
        })
        .and_then(collect_iterable)?;
    let mut result: IndexSet<PyKey> = IndexSet::new();
    for k in items {
        if !other.contains(k) {
            result.insert(k.clone());
        }
    }
    for k in &other {
        if !items.contains(k) {
            result.insert(k.clone());
        }
    }
    Ok(Value::set(result))
}

fn issubset(items: &IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.issubset() requires 1 argument".to_string()))
        .and_then(collect_iterable)?;
    Ok(Value::bool_(items.iter().all(|k| other.contains(k))))
}

fn issuperset(items: &IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.issuperset() requires 1 argument".to_string()))
        .and_then(collect_iterable)?;
    Ok(Value::bool_(other.iter().all(|k| items.contains(k))))
}

fn isdisjoint(items: &IndexSet<PyKey>, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.isdisjoint() requires 1 argument".to_string()))
        .and_then(collect_iterable)?;
    Ok(Value::bool_(!items.iter().any(|k| other.contains(k))))
}

// ── key → Value conversion ────────────────────────────────────────────────────

fn key_to_value(k: PyKey) -> Value {
    match k {
        PyKey::Int(v) => Value::int(v),
        PyKey::Float(bits) => Value::float(f64::from_bits(bits)),
        PyKey::Str(s) => Value::string(s),
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
    }
}
