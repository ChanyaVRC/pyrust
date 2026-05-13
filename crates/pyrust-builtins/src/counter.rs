//! `collections.Counter` — dict-like multiset.
//!
//! Counter is a real built-in object type (not "Counter-the-function
//! returning a plain dict"), so all the Counter-specific methods
//! (`most_common`, `elements`, `update`, `subtract`) and the contains/len
//! protocols light up.  Storage is an `IndexMap<PyKey, i64>` — values are
//! ints by construction, and we serialise out as a regular dict for
//! interop with the rest of the codebase only when explicitly asked
//! (currently nowhere — the Counter value behaves like a dict from the
//! outside via the `BuiltinTypeOps` surface).
//!
//! What's deliberately *not* supported yet:
//!
//! - Arithmetic operators (`+`, `-`, `&`, `|`).  CPython's Counter
//!   supports them with the "keep only positive counts" rule; implementing
//!   that requires plumbing into the interpreter's binary-op dispatch,
//!   which is a separable next step.
//! - Subclassing.  Counter is a leaf type here, not a subclass of dict.
//!
//! Reference: <https://docs.python.org/3/library/collections.html#collections.Counter>

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, PyKey, Result, Value, ValueKind};

/// Internal Counter state.  Counts are signed `i64` because `subtract()`
/// allows them to go below zero (and even negative) — only `elements()`
/// and `most_common()` filter to positive entries.
///
/// The map lives behind `Rc<RefCell<…>>` so `borrow_counts` can hand the
/// same backing storage to every consumer of the method-dispatch path —
/// mutations through `set_item`, `update`, and `subtract` then persist
/// across calls.  Cheap clones (Rc bump) keep the dispatch surface tidy
/// without the methods having to thread the BuiltinState borrow through
/// their bodies.
pub struct CounterState {
    pub counts: Rc<RefCell<IndexMap<PyKey, i64>>>,
}

pub struct CounterOps;

pub const COUNTER_OPS: &CounterOps = &CounterOps;
pub const TYPE_NAME: &str = "collections.Counter";

/// Canonical list of method names — consumed by `dir(Counter(...))`.
pub const METHODS: &[&str] = &[
    "copy",
    "elements",
    "get",
    "items",
    "keys",
    "most_common",
    "subtract",
    "update",
    "values",
];

impl BuiltinTypeOps for CounterOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let counts = match borrow_counts(state) {
            Some(c) => c,
            None => return "Counter(<bad state>)".to_string(),
        };
        let m = counts.borrow();
        if m.is_empty() {
            return "Counter()".to_string();
        }
        // CPython's repr is most-common-first, like `Counter({'a': 3, 'b': 1})`.
        let mut sorted: Vec<(&PyKey, &i64)> = m.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        let inner: Vec<String> = sorted
            .iter()
            .map(|(k, v)| format!("{}: {v}", key_repr(k)))
            .collect();
        format!("Counter({{{}}})", inner.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        borrow_counts(state).is_some_and(|c| !c.borrow().is_empty())
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let lhs = match borrow_counts(state) {
            Some(c) => c,
            None => return false,
        };
        // Counter == Counter: same keys and counts.
        // Counter == dict:    same keys and counts (Counter is a dict subclass in CPython).
        match other.kind() {
            ValueKind::BuiltinObject {
                ops,
                state: other_state,
            } if ops.type_name() == TYPE_NAME => {
                let rhs_rc = match borrow_counts(other_state) {
                    Some(c) => c,
                    None => return false,
                };
                let lhs_m = lhs.borrow();
                let rhs_m = rhs_rc.borrow();
                if lhs_m.len() != rhs_m.len() {
                    return false;
                }
                lhs_m.iter().all(|(k, v)| rhs_m.get(k) == Some(v))
            }
            ValueKind::Dict(rhs_map) => {
                let lhs_m = lhs.borrow();
                if lhs_m.len() != rhs_map.len() {
                    return false;
                }
                lhs_m.iter().all(|(k, lhs_v)| match rhs_map.get(k) {
                    Some(rhs_val) => matches!(rhs_val.kind(), ValueKind::Int(n) if n == *lhs_v),
                    None => false,
                })
            }
            _ => false,
        }
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        borrow_counts(state).map(|c| c.borrow().len())
    }

    fn get_item(&self, state: &BuiltinState, key: &Value) -> Result<Value> {
        // Counter's missing-key rule: return 0 (not KeyError).  This is the
        // *only* dict-subclass quirk pyrust replicates faithfully — it's
        // the whole reason Counter exists.
        let counts = borrow_counts(state)
            .ok_or_else(|| PyError::Runtime("internal: bad Counter state".to_string()))?;
        let pk = key
            .to_key()
            .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
        Ok(Value::int(counts.borrow().get(&pk).copied().unwrap_or(0)))
    }

    fn set_item(&self, state: &BuiltinState, key: &Value, value: Value) -> Result<()> {
        let counts = borrow_counts(state)
            .ok_or_else(|| PyError::Runtime("internal: bad Counter state".to_string()))?;
        let pk = key
            .to_key()
            .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
        let n = match value.kind() {
            ValueKind::Int(n) => n,
            ValueKind::Bool(b) => b as i64,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "Counter values must be integers".to_string(),
                ));
            }
        };
        counts.borrow_mut().insert(pk, n);
        Ok(())
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let counts = borrow_counts(state)
            .ok_or_else(|| PyError::Runtime("internal: bad Counter state".to_string()))?;
        let pk = item
            .to_key()
            .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
        Ok(counts.borrow().contains_key(&pk))
    }

    fn has_method(&self, name: &str) -> bool {
        METHODS.contains(&name)
    }

    fn is_iterable(&self) -> bool {
        // Iteration yields keys, matching dict semantics.
        true
    }

    fn iter_next(&self, _state: &BuiltinState) -> Result<Option<Value>> {
        // The VM materialises iterables before consuming them when
        // `is_iterable` is true and no separate cursor exists; the
        // collected items come from `iter_values_via_registry` which
        // probes `iter_keys`-like paths.  We surface keys via `keys()`
        // below; the default `iter_next` errs by returning None, so this
        // path is unreachable in practice for Counter.
        Ok(None)
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        method: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let counts = borrow_counts(state)
            .ok_or_else(|| PyError::Runtime("internal: bad Counter state".to_string()))?;
        match method {
            "most_common" => most_common(&counts, args),
            "elements" => elements(&counts, args),
            "update" => update(&counts, args),
            "subtract" => subtract(&counts, args),
            "copy" => copy(&counts, args),
            "get" => get(&counts, args),
            "keys" => keys(&counts, args),
            "values" => values(&counts, args),
            "items" => items(&counts, args),
            _ => Err(PyError::named(
                "AttributeError",
                format!("'Counter' object has no attribute '{method}'"),
            )),
        }
    }
}

/// Construct a Counter Value with the given counts.
pub fn counter(counts: IndexMap<PyKey, i64>) -> Value {
    let state: Box<dyn Any> = Box::new(CounterState {
        counts: Rc::new(RefCell::new(counts)),
    });
    Value::builtin_object(COUNTER_OPS, state)
}

/// Extract the inner counts map from a Counter Value, or None if the
/// value isn't a Counter.
pub fn as_counts(value: &Value) -> Option<Rc<RefCell<IndexMap<PyKey, i64>>>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    borrow_counts(state)
}

// ── method implementations ───────────────────────────────────────────────────

fn most_common(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, args: Vec<Value>) -> Result<Value> {
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            "most_common() takes at most 1 argument".to_string(),
        ));
    }
    let n: Option<usize> = match args.first().map(|a| a.kind()) {
        None | Some(ValueKind::None) => None,
        Some(ValueKind::Int(v)) if v >= 0 => Some(v as usize),
        Some(ValueKind::Bool(b)) => Some(b as usize),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "most_common() n must be a non-negative integer or None".to_string(),
            ));
        }
    };
    // Build (key, count) pairs sorted by descending count, ties broken by
    // insertion order (which matches CPython's stable-sort behaviour now
    // that dicts preserve order).
    let borrowed = counts.borrow();
    let mut pairs: Vec<(PyKey, i64)> = borrowed.iter().map(|(k, v)| (k.clone(), *v)).collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    let upper = n.unwrap_or(pairs.len()).min(pairs.len());
    let out: Vec<Value> = pairs
        .into_iter()
        .take(upper)
        .map(|(k, v)| Value::tuple(vec![key_to_value(k), Value::int(v)]))
        .collect();
    Ok(Value::list(out))
}

fn elements(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, args: Vec<Value>) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "elements() takes no arguments".to_string(),
        ));
    }
    // Per CPython: yields each element `count` times, for elements whose
    // count is > 0.  Zero or negative counts are skipped silently.
    let borrowed = counts.borrow();
    let mut out: Vec<Value> = Vec::new();
    for (k, &v) in borrowed.iter() {
        if v > 0 {
            for _ in 0..v {
                out.push(key_to_value(k.clone()));
            }
        }
    }
    Ok(Value::list(out))
}

fn update(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, args: Vec<Value>) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "update() takes exactly 1 argument".to_string(),
        ));
    }
    apply_delta(counts, &args[0], /* sign = */ 1)?;
    Ok(Value::none())
}

fn subtract(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, args: Vec<Value>) -> Result<Value> {
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            "subtract() takes exactly 1 argument".to_string(),
        ));
    }
    apply_delta(counts, &args[0], /* sign = */ -1)?;
    Ok(Value::none())
}

fn copy(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, args: Vec<Value>) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "copy() takes no arguments".to_string(),
        ));
    }
    Ok(counter(counts.borrow().clone()))
}

fn get(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, args: Vec<Value>) -> Result<Value> {
    if args.is_empty() || args.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            "get() takes 1 or 2 arguments".to_string(),
        ));
    }
    let key = args[0]
        .to_key()
        .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
    match counts.borrow().get(&key) {
        Some(&v) => Ok(Value::int(v)),
        None => Ok(args.get(1).cloned().unwrap_or_else(Value::none)),
    }
}

fn keys(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, args: Vec<Value>) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "keys() takes no arguments".to_string(),
        ));
    }
    let out: Vec<Value> = counts.borrow().keys().cloned().map(key_to_value).collect();
    Ok(Value::list(out))
}

fn values(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, args: Vec<Value>) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "values() takes no arguments".to_string(),
        ));
    }
    let out: Vec<Value> = counts.borrow().values().map(|v| Value::int(*v)).collect();
    Ok(Value::list(out))
}

fn items(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, args: Vec<Value>) -> Result<Value> {
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "items() takes no arguments".to_string(),
        ));
    }
    let out: Vec<Value> = counts
        .borrow()
        .iter()
        .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), Value::int(*v)]))
        .collect();
    Ok(Value::list(out))
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn borrow_counts(state: &BuiltinState) -> Option<Rc<RefCell<IndexMap<PyKey, i64>>>> {
    let borrow = state.borrow();
    borrow
        .downcast_ref::<CounterState>()
        .map(|s| Rc::clone(&s.counts))
}

/// Apply `update`/`subtract` semantics: for each element of `other`, adjust
/// the count by `sign`.  `other` can be an iterable (counts as +1/-1 per
/// element) or a mapping (uses the mapping's values).
fn apply_delta(counts: &Rc<RefCell<IndexMap<PyKey, i64>>>, other: &Value, sign: i64) -> Result<()> {
    if let ValueKind::Dict(map) = other.kind() {
        // Mapping form: keys are the keys, values are the deltas.
        let mut m = counts.borrow_mut();
        for (k, v) in map.iter() {
            let delta = match v.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "Counter delta values must be integers".to_string(),
                    ));
                }
            };
            *m.entry(k.clone()).or_insert(0) += sign * delta;
        }
        return Ok(());
    }
    if let Some(other_counts) = as_counts(other) {
        let mut m = counts.borrow_mut();
        for (k, v) in other_counts.borrow().iter() {
            *m.entry(k.clone()).or_insert(0) += sign * v;
        }
        return Ok(());
    }
    // Iterable form: each element contributes ±1.
    let items = pyrust_core::iter_values_via_registry(other)?;
    let mut m = counts.borrow_mut();
    for item in items {
        let pk = item
            .to_key()
            .ok_or_else(|| PyError::named("TypeError", "unhashable type".to_string()))?;
        *m.entry(pk).or_insert(0) += sign;
    }
    Ok(())
}

fn key_to_value(key: PyKey) -> Value {
    match key {
        PyKey::Int(v) => Value::int(v),
        PyKey::Float(bits) => Value::float(f64::from_bits(bits)),
        PyKey::Str(s) => Value::string(s),
        PyKey::Bool(b) => Value::bool_(b),
        PyKey::None => Value::none(),
        PyKey::FrozenSet(items) => crate::frozenset::frozenset(items.into_iter().collect()),
    }
}

fn key_repr(key: &PyKey) -> String {
    match key {
        PyKey::Int(v) => v.to_string(),
        PyKey::Float(bits) => f64::from_bits(*bits).to_string(),
        PyKey::Str(s) => format!("'{s}'"),
        PyKey::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        PyKey::None => "None".to_string(),
        PyKey::FrozenSet(_) => "frozenset(...)".to_string(),
    }
}
