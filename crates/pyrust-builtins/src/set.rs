use indexmap::IndexSet;
use pyrust_core::{PyError, PyKey, Result, Value, ValueKind};

/// Canonical list of method names dispatched by `call`.
pub const METHODS: &[&str] = &[
    "add",
    "remove",
    "discard",
    "pop",
    "clear",
    "update",
    "intersection_update",
    "difference_update",
    "symmetric_difference_update",
    "copy",
    "union",
    "intersection",
    "difference",
    "symmetric_difference",
    "issubset",
    "issuperset",
    "isdisjoint",
];

/// Returns `true` if `method` is the name of a built-in `set` method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

/// Dispatch a `set` method.  Receiver is `&Value`; each method body
/// either reads via `set_with` or writes via `set_with_mut`, with
/// argument iteration happening *outside* the scoped borrow so
/// self-aliased calls (`s.update(s)`) never simultaneously borrow the
/// same storage (#448).
pub fn call(method: &str, receiver: &Value, args: Vec<Value>) -> Result<Value> {
    let args = args.as_slice();
    let not_set = || PyError::named("TypeError", "set method receiver is not a set".to_string());
    match method {
        // ── Mutating ───────────────────────────────────────────────
        "add" => receiver
            .set_with_mut(|items| add(items, args))
            .ok_or_else(not_set)?,
        "remove" => receiver
            .set_with_mut(|items| remove(items, args))
            .ok_or_else(not_set)?,
        "discard" => receiver
            .set_with_mut(|items| discard(items, args))
            .ok_or_else(not_set)?,
        "pop" => receiver.set_with_mut(pop).ok_or_else(not_set)?,
        "clear" => {
            receiver.set_clear()?;
            Ok(Value::none())
        }
        // Iterating + mutating methods: materialise all arg
        // iterables BEFORE borrow_mut so a self-aliased call
        // (`s.update(s)`) doesn't take a `&` to the same storage
        // we're about to `&mut`.
        // For the `*_update` family we collect ONE iterable, apply it,
        // then move to the next.  CPython does the same — an error
        // collecting the 2nd arg leaves the 1st arg's effect on the
        // receiver visible (`s.update([1], object())` leaves `1` in
        // `s` before raising TypeError).  Collecting all args upfront
        // would make these all-or-nothing, diverging from CPython.
        "update" => {
            for arg in args {
                let snap = snapshot_iterable(receiver, arg)?;
                receiver
                    .set_with_mut(|items| items.extend(snap))
                    .ok_or_else(not_set)?;
            }
            Ok(Value::none())
        }
        "intersection_update" => {
            for arg in args {
                let snap = snapshot_iterable(receiver, arg)?;
                receiver
                    .set_with_mut(|items| items.retain(|k| snap.contains(k)))
                    .ok_or_else(not_set)?;
            }
            Ok(Value::none())
        }
        "difference_update" => {
            for arg in args {
                let snap = snapshot_iterable(receiver, arg)?;
                receiver
                    .set_with_mut(|items| {
                        for k in &snap {
                            items.shift_remove(k);
                        }
                    })
                    .ok_or_else(not_set)?;
            }
            Ok(Value::none())
        }
        "symmetric_difference_update" => {
            let other = args
                .first()
                .ok_or_else(|| {
                    PyError::Runtime(
                        "set.symmetric_difference_update() requires 1 argument".to_string(),
                    )
                })
                .and_then(|v| snapshot_iterable(receiver, v))?;
            receiver
                .set_with_mut(|items| {
                    let mut to_add: Vec<PyKey> = Vec::new();
                    for k in &other {
                        if !items.contains(k) {
                            to_add.push(k.clone());
                        }
                    }
                    items.retain(|k| !other.contains(k));
                    for k in to_add {
                        items.insert(k);
                    }
                })
                .ok_or_else(not_set)?;
            Ok(Value::none())
        }
        // ── Non-mutating ───────────────────────────────────────────
        // Scoped read borrow + clone is enough; no `&mut` ever taken.
        "copy" => receiver
            .set_with(|items| Value::set(items.clone()))
            .ok_or_else(not_set),
        "union" => union(receiver, args),
        "intersection" => intersection(receiver, args),
        "difference" => difference(receiver, args),
        "symmetric_difference" => symmetric_difference(receiver, args),
        "issubset" => issubset(receiver, args),
        "issuperset" => issuperset(receiver, args),
        "isdisjoint" => isdisjoint(receiver, args),
        _ => Err(PyError::Runtime(format!(
            "'set' object has no attribute '{method}'"
        ))),
    }
}

/// Materialise each arg into an owned `IndexSet<PyKey>`.  Performed
/// before any `borrow_mut` on the receiver so a self-aliased call
/// (`s.update(s)`) reads its own pre-update snapshot, exactly matching
/// CPython's iterate-then-mutate semantics.
fn collect_iterables(receiver: &Value, args: &[Value]) -> Result<Vec<IndexSet<PyKey>>> {
    args.iter()
        .map(|arg| snapshot_iterable(receiver, arg))
        .collect()
}

/// Same as `collect_iterable` but takes a snapshot of the receiver
/// via `set_with` when the arg aliases it, avoiding the
/// `as_set`/`borrow` reentry on the same storage.
fn snapshot_iterable(receiver: &Value, arg: &Value) -> Result<IndexSet<PyKey>> {
    if std::ptr::eq(receiver as *const Value, arg as *const Value)
        || receiver.value_id() == arg.value_id() && receiver.value_id().is_some()
    {
        // Aliased: snapshot via the scoped borrow.
        return receiver.set_with(|items| items.clone()).ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set snapshot receiver is not a set".to_string(),
            )
        });
    }
    collect_iterable(arg)
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn to_key(v: &Value) -> Result<PyKey> {
    v.to_key()
        .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))
}

/// Collect an iterable `Value` into a set of `PyKey`s.
fn collect_iterable(v: &Value) -> Result<IndexSet<PyKey>> {
    let mut out = IndexSet::new();
    match v.kind() {
        ValueKind::Set(s) => {
            for k in s.iter() {
                out.insert(k.clone());
            }
        }
        _ if crate::frozenset::as_items(v).is_some() => {
            let rc = crate::frozenset::as_items(v).unwrap();
            for k in rc.iter() {
                out.insert(k.clone());
            }
        }
        ValueKind::List(items) => {
            for item in items.iter() {
                out.insert(to_key(item)?);
            }
        }
        ValueKind::Tuple(items) => {
            for item in items.iter() {
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
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object is not iterable",
                    pyrust_core::builtin_type_name(v)
                ),
            ));
        }
    }
    Ok(out)
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
        Err(PyError::key_error(elem.clone()))
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
        None => Err(PyError::named(
            "KeyError",
            "pop from an empty set".to_string(),
        )),
    }
}

// ── non-mutating methods ──────────────────────────────────────────────────────
//
// Each helper takes `&Value` and uses a scoped `set_with` borrow.
// Argument iterables are collected before the borrow opens, so
// self-aliased calls (`s.union(s)`) read a stable snapshot.

fn union(receiver: &Value, args: &[Value]) -> Result<Value> {
    let snapshots = collect_iterables(receiver, args)?;
    receiver
        .set_with(|items| {
            let mut result = items.clone();
            for snap in snapshots {
                for k in snap {
                    result.insert(k);
                }
            }
            Value::set(result)
        })
        .ok_or_else(|| PyError::named("TypeError", "set.union receiver is not a set".to_string()))
}

fn intersection(receiver: &Value, args: &[Value]) -> Result<Value> {
    let snapshots = collect_iterables(receiver, args)?;
    receiver
        .set_with(|items| {
            let mut result = items.clone();
            for snap in &snapshots {
                result.retain(|k| snap.contains(k));
            }
            Value::set(result)
        })
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.intersection receiver is not a set".to_string(),
            )
        })
}

fn difference(receiver: &Value, args: &[Value]) -> Result<Value> {
    let snapshots = collect_iterables(receiver, args)?;
    receiver
        .set_with(|items| {
            let mut result = items.clone();
            for snap in &snapshots {
                for k in snap {
                    result.shift_remove(k);
                }
            }
            Value::set(result)
        })
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.difference receiver is not a set".to_string(),
            )
        })
}

fn symmetric_difference(receiver: &Value, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| {
            PyError::Runtime("set.symmetric_difference() requires 1 argument".to_string())
        })
        .and_then(|v| snapshot_iterable(receiver, v))?;
    receiver
        .set_with(|items| {
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
            Value::set(result)
        })
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.symmetric_difference receiver is not a set".to_string(),
            )
        })
}

fn issubset(receiver: &Value, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.issubset() requires 1 argument".to_string()))
        .and_then(|v| snapshot_iterable(receiver, v))?;
    receiver
        .set_with(|items| Value::bool_(items.iter().all(|k| other.contains(k))))
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.issubset receiver is not a set".to_string(),
            )
        })
}

fn issuperset(receiver: &Value, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.issuperset() requires 1 argument".to_string()))
        .and_then(|v| snapshot_iterable(receiver, v))?;
    receiver
        .set_with(|items| Value::bool_(other.iter().all(|k| items.contains(k))))
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.issuperset receiver is not a set".to_string(),
            )
        })
}

fn isdisjoint(receiver: &Value, args: &[Value]) -> Result<Value> {
    let other = args
        .first()
        .ok_or_else(|| PyError::Runtime("set.isdisjoint() requires 1 argument".to_string()))
        .and_then(|v| snapshot_iterable(receiver, v))?;
    receiver
        .set_with(|items| Value::bool_(!items.iter().any(|k| other.contains(k))))
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                "set.isdisjoint receiver is not a set".to_string(),
            )
        })
}

// ── key → Value conversion ────────────────────────────────────────────────────

fn key_to_value(k: PyKey) -> Value {
    match k {
        PyKey::Int(v) => Value::int(v),
        PyKey::BigInt(v) => Value::bigint(*v),
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
