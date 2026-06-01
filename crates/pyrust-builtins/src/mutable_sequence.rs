use pyrust_core::{PyError, Result, Value, ValueKind, iter_values_via_registry};

use crate::sequence::normalise_index;

// Mutable Sequence Operations (https://docs.python.org/3/library/stdtypes.html#mutable-sequence-types)
//
// Receivers are `&Value` and each method scopes its `RefCell::borrow_mut`
// to the operation's lifetime (#448).  No `&mut Vec<Value>` is exposed
// across crate boundaries, so the previous `unalias_args_for_mutation`
// dance is no longer needed.

pub fn append(receiver: &Value, args: Vec<Value>) -> Result<Value> {
    let mut iter = args.into_iter();
    let val = iter
        .next()
        .ok_or_else(|| PyError::Runtime("list.append() requires 1 argument".to_string()))?;
    receiver.list_push(val)?;
    Ok(Value::none())
}

pub fn clear(receiver: &Value, _args: Vec<Value>) -> Result<Value> {
    receiver.list_clear()?;
    Ok(Value::none())
}

pub fn copy(receiver: &Value, _args: Vec<Value>) -> Result<Value> {
    let snapshot = receiver.list_with(|items| items.clone()).ok_or_else(|| {
        PyError::named("TypeError", "list.copy receiver is not a list".to_string())
    })?;
    Ok(Value::list(snapshot))
}

pub fn extend(receiver: &Value, args: Vec<Value>) -> Result<Value> {
    let mut iter = args.into_iter();
    let iterable = iter
        .next()
        .ok_or_else(|| PyError::Runtime("list.extend() requires 1 argument".to_string()))?;
    // Materialise the snapshot BEFORE touching the receiver's storage.
    // This is the structural fix for #414: with no `&mut` held on the
    // receiver while we iterate the arg, aliasing (`a.extend(a)`) can't
    // produce a simultaneous borrow regardless of whether the arg is the
    // same Rc as the receiver.
    //
    // Goes through `iter_values_via_registry` (#427) so any iterable the
    // interpreter recognises — set, dict, dict-views, bytes, generators,
    // user `__iter__` — works here too, not just list/tuple/str/range.
    let snapshot = iter_values_via_registry(&iterable)?;
    receiver.list_extend(snapshot)?;
    Ok(Value::none())
}

pub fn insert(receiver: &Value, args: Vec<Value>) -> Result<Value> {
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
    let len = receiver.list_len().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "list.insert receiver is not a list".to_string(),
        )
    })?;
    let pos = if idx < 0 {
        let from_end = (-idx) as usize;
        len.saturating_sub(from_end)
    } else {
        (idx as usize).min(len)
    };
    receiver.list_insert(pos, val)?;
    Ok(Value::none())
}

pub fn pop(receiver: &Value, args: Vec<Value>) -> Result<Value> {
    let first = args.into_iter().next();
    let idx = match first.as_ref().map(|v| v.kind()) {
        Some(ValueKind::Int(i)) => i,
        Some(ValueKind::Bool(b)) => b as i64,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                "'list.pop' index must be an integer".to_string(),
            ));
        }
        None => -1,
    };
    let len = receiver.list_len().ok_or_else(|| {
        PyError::named("TypeError", "list.pop receiver is not a list".to_string())
    })?;
    if len == 0 {
        return Err(PyError::named(
            "IndexError",
            "pop from empty list".to_string(),
        ));
    }
    let pos = normalise_index(idx, len);
    if pos >= len {
        return Err(PyError::named(
            "IndexError",
            "pop index out of range".to_string(),
        ));
    }
    receiver.list_pop_at(pos)
}

pub fn remove(receiver: &Value, args: Vec<Value>) -> Result<Value> {
    let mut iter = args.into_iter();
    let target = iter
        .next()
        .ok_or_else(|| PyError::Runtime("list.remove() requires 1 argument".to_string()))?;
    let pos = receiver
        .list_with(|items| items.iter().position(|v| v == &target))
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "list.remove receiver is not a list".to_string(),
            )
        })?;
    let pos = pos
        .ok_or_else(|| PyError::named("ValueError", "list.remove(x): x not in list".to_string()))?;
    receiver.list_pop_at(pos)?;
    Ok(Value::none())
}

pub fn reverse(receiver: &Value, _args: Vec<Value>) -> Result<Value> {
    receiver.list_reverse()?;
    Ok(Value::none())
}
