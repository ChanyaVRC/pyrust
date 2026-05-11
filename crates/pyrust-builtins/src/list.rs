use indexmap::IndexMap;
use pyrust_core::{PyError, PyKey, Result, Value, ValueKind};

use crate::mutable_sequence as ms;
use crate::sequence;

pub fn call(
    method: &str,
    items: &mut Vec<Value>,
    args: Vec<Value>,
    kwargs: &IndexMap<PyKey, Value>,
) -> Result<Value> {
    match method {
        // Common Sequence Operations
        "index" => sequence::seq_index(items, &args, "list"),
        "count" => sequence::seq_count(items, &args, "list"),
        // Mutable Sequence Operations
        "append" => ms::append(items, args),
        "clear" => ms::clear(items, args),
        "copy" => ms::copy(items, args),
        "extend" => ms::extend(items, args),
        "insert" => ms::insert(items, args),
        "pop" => ms::pop(items, args),
        "remove" => ms::remove(items, args),
        "reverse" => ms::reverse(items, args),
        // List-specific
        "sort" => sort(items, &args, kwargs),
        _ => Err(PyError::Runtime(format!(
            "'list' object has no attribute '{method}'"
        ))),
    }
}

fn sort(items: &mut Vec<Value>, args: &[Value], kwargs: &IndexMap<PyKey, Value>) -> Result<Value> {
    let reverse_flag = extract_reverse(args, kwargs)?;
    sort_by_cmp(items, reverse_flag)
}

fn extract_reverse(args: &[Value], kwargs: &IndexMap<PyKey, Value>) -> Result<bool> {
    Ok(
        match (
            args.first().map(|v| v.kind()),
            kwargs
                .get(&PyKey::Str("reverse".to_string()))
                .map(|v| v.kind()),
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

fn sort_by_cmp(items: &mut Vec<Value>, reverse: bool) -> Result<Value> {
    let snapshot = items.clone();
    let mut err: Option<PyError> = None;
    items.sort_by(|a, b| {
        if err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
        match compare_values(lhs, rhs) {
            Ok(ord) => ord,
            Err(e) => {
                err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = err {
        *items = snapshot;
        return Err(e);
    }
    Ok(Value::none())
}

/// Sort items using precomputed keys (one key per item in the same order).
/// Called by the VM after evaluating each key function call.
pub fn sort_with_precomputed_keys(
    items: &mut Vec<Value>,
    keys: Vec<Value>,
    reverse: bool,
) -> Result<Value> {
    debug_assert_eq!(items.len(), keys.len());
    let snapshot = items.clone();
    let mut keyed: Vec<(Value, Value)> = keys.into_iter().zip(items.iter().cloned()).collect();
    let mut sort_err: Option<PyError> = None;
    keyed.sort_by(|(ka, _), (kb, _)| {
        if sort_err.is_some() {
            return std::cmp::Ordering::Equal;
        }
        let (lhs, rhs) = if reverse { (kb, ka) } else { (ka, kb) };
        match compare_values(lhs, rhs) {
            Ok(ord) => ord,
            Err(e) => {
                sort_err = Some(e);
                std::cmp::Ordering::Equal
            }
        }
    });
    if let Some(e) = sort_err {
        *items = snapshot;
        return Err(e);
    }
    *items = keyed.into_iter().map(|(_, v)| v).collect();
    Ok(Value::none())
}

pub fn compare_values(a: &Value, b: &Value) -> Result<std::cmp::Ordering> {
    match (a.kind(), b.kind()) {
        (ValueKind::Int(x), ValueKind::Int(y)) => Ok(x.cmp(&y)),
        (ValueKind::Float(x), ValueKind::Float(y)) => {
            Ok(x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal))
        }
        (ValueKind::Int(x), ValueKind::Float(y)) => Ok((x as f64)
            .partial_cmp(&y)
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Float(x), ValueKind::Int(y)) => Ok(x
            .partial_cmp(&(y as f64))
            .unwrap_or(std::cmp::Ordering::Equal)),
        (ValueKind::Str(x), ValueKind::Str(y)) => Ok(x.cmp(y)),
        (ValueKind::Bool(x), ValueKind::Bool(y)) => Ok(x.cmp(&y)),
        (ValueKind::Bool(x), ValueKind::Int(y)) => Ok((x as i64).cmp(&y)),
        (ValueKind::Int(x), ValueKind::Bool(y)) => Ok(x.cmp(&(y as i64))),
        (ValueKind::Tuple(x), ValueKind::Tuple(y)) => {
            for (xi, yi) in x.iter().zip(y.iter()) {
                let ord = compare_values(xi, yi)?;
                if ord != std::cmp::Ordering::Equal {
                    return Ok(ord);
                }
            }
            Ok(x.len().cmp(&y.len()))
        }
        _ => Err(PyError::Runtime(format!(
            "'<' not supported between instances of '{}' and '{}'",
            type_name(a),
            type_name(b)
        ))),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v.kind() {
        ValueKind::Int(_) => "int",
        ValueKind::Float(_) => "float",
        ValueKind::Str(_) => "str",
        ValueKind::Bool(_) => "bool",
        ValueKind::None => "NoneType",
        ValueKind::List(_) => "list",
        ValueKind::Tuple(_) => "tuple",
        _ => "object",
    }
}
