use pyrust_core::{PyError, Result, Value, ValueKind};

use crate::sequence::normalise_index;

fn iter_value(v: &Value) -> Result<Vec<Value>> {
    match v.kind() {
        ValueKind::List(items) => Ok(items.to_vec()),
        ValueKind::Tuple(items) => Ok(items.to_vec()),
        ValueKind::Str(s) => Ok(s.chars().map(|c| Value::string(c.to_string())).collect()),
        ValueKind::Range { start, stop, step } => {
            let mut out = Vec::new();
            let mut cur = start;
            loop {
                if step > 0 && cur >= stop {
                    break;
                }
                if step < 0 && cur <= stop {
                    break;
                }
                out.push(Value::int(cur));
                cur = cur.wrapping_add(step);
            }
            Ok(out)
        }
        _ => Err(PyError::Runtime(format!(
            "'{}' object is not iterable",
            pyrust_core::builtin_type_name(v)
        ))),
    }
}

// Mutable Sequence Operations (https://docs.python.org/3/library/stdtypes.html#mutable-sequence-types)

pub fn append(items: &mut Vec<Value>, args: Vec<Value>) -> Result<Value> {
    let mut iter = args.into_iter();
    let val = iter
        .next()
        .ok_or_else(|| PyError::Runtime("list.append() requires 1 argument".to_string()))?;
    items.push(val);
    Ok(Value::none())
}

pub fn clear(items: &mut Vec<Value>, _args: Vec<Value>) -> Result<Value> {
    items.clear();
    Ok(Value::none())
}

pub fn copy(items: &[Value], _args: Vec<Value>) -> Result<Value> {
    Ok(Value::list(items.to_vec()))
}

pub fn extend(items: &mut Vec<Value>, args: Vec<Value>) -> Result<Value> {
    let mut iter = args.into_iter();
    let iterable = iter
        .next()
        .ok_or_else(|| PyError::Runtime("list.extend() requires 1 argument".to_string()))?;
    let new_items = iter_value(&iterable)?;
    items.extend(new_items);
    Ok(Value::none())
}

pub fn insert(items: &mut Vec<Value>, args: Vec<Value>) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::Runtime(
            "list.insert() requires 2 arguments".to_string(),
        ));
    }
    let mut iter = args.into_iter();
    let idx_val = iter.next().unwrap();
    let val = iter.next().unwrap();
    let idx = match idx_val.kind() {
        ValueKind::Int(i) => i,
        ValueKind::Bool(b) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "list.insert() index must be an integer".to_string(),
            ));
        }
    };
    let len = items.len();
    let pos = if idx < 0 {
        let from_end = (-idx) as usize;
        len.saturating_sub(from_end)
    } else {
        (idx as usize).min(len)
    };
    items.insert(pos, val);
    Ok(Value::none())
}

pub fn pop(items: &mut Vec<Value>, args: Vec<Value>) -> Result<Value> {
    let first = args.into_iter().next();
    let idx = match first.as_ref().map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => i,
        Some(ValueKind::Bool(b)) => b as i64,
        Some(_) => {
            return Err(PyError::Runtime(
                "list.pop() index must be an integer".to_string(),
            ));
        }
        None => -1,
    };
    if items.is_empty() {
        return Err(PyError::Runtime("pop from empty list".to_string()));
    }
    let pos = normalise_index(idx, items.len());
    if pos >= items.len() {
        return Err(PyError::named(
            "IndexError",
            "pop index out of range".to_string(),
        ));
    }
    Ok(items.remove(pos))
}

pub fn remove(items: &mut Vec<Value>, args: Vec<Value>) -> Result<Value> {
    let mut iter = args.into_iter();
    let target = iter
        .next()
        .ok_or_else(|| PyError::Runtime("list.remove() requires 1 argument".to_string()))?;
    let pos = items
        .iter()
        .position(|v| v == &target)
        .ok_or_else(|| PyError::Runtime(format!("{} is not in list", target.repr())))?;
    items.remove(pos);
    Ok(Value::none())
}

pub fn reverse(items: &mut [Value], _args: Vec<Value>) -> Result<Value> {
    items.reverse();
    Ok(Value::none())
}
