//! Frozenset value — relocated from `pyrust-core` Tier 1.
//!
//! Frozensets have no literal syntax (only the `frozenset(...)` call) and no
//! compile-time-specializable payload.  The hashable key form `PyKey::FrozenSet`
//! stays in pyrust-core; the value form lives here as a generic BuiltinObject.

use std::any::Any;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};
use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, PyKey, Result, Value, ValueKind};

/// Internal frozenset state.  `Rc` so that clones are cheap and so that
/// the same backing storage can be shared via `Value::builtin_object_shared`.
pub struct FrozenSetState {
    pub items: Rc<IndexSet<PyKey>>,
}

pub struct FrozenSetOps;

pub const FROZENSET_OPS: &FrozenSetOps = &FrozenSetOps;
pub const TYPE_NAME: &str = "frozenset";

/// Canonical list of method names exposed by frozenset.  Frozenset reuses
/// the non-mutating subset of `set`'s methods via `call_method` (see below);
/// this list mirrors the names recognised there and is consumed by `dir()`.
pub const METHODS: &[&str] = &[
    "copy",
    "difference",
    "intersection",
    "isdisjoint",
    "issubset",
    "issuperset",
    "symmetric_difference",
    "union",
];

impl BuiltinTypeOps for FrozenSetOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let items = borrow_items(state).expect("frozenset state");
        if items.is_empty() {
            return "frozenset()".to_string();
        }
        let inner: Vec<String> = items.iter().map(key_repr).collect();
        format!("frozenset({{{}}})", inner.join(", "))
    }

    fn truthy(&self, state: &BuiltinState) -> bool {
        borrow_items(state).map(|s| !s.is_empty()).unwrap_or(false)
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let lhs = match borrow_items(state) {
            Some(s) => s,
            None => return false,
        };
        match other.kind() {
            ValueKind::Set(rhs) => *lhs == *rhs,
            ValueKind::BuiltinObject {
                ops,
                state: rhs_state,
            } if ops.type_name() == TYPE_NAME => {
                let rhs = match borrow_items(rhs_state) {
                    Some(s) => s,
                    None => return false,
                };
                // Use Rc::ptr_eq fast path, then fall back to content equality.
                Rc::ptr_eq(&lhs, &rhs) || *lhs == *rhs
            }
            _ => false,
        }
    }

    fn len(&self, state: &BuiltinState) -> Option<usize> {
        borrow_items(state).map(|s| s.len())
    }

    fn contains(&self, state: &BuiltinState, item: &Value) -> Result<bool> {
        let items = borrow_items(state)
            .ok_or_else(|| PyError::Runtime("internal: bad frozenset state".to_string()))?;
        match item.to_key() {
            Some(k) => Ok(items.contains(&k)),
            None => Err(PyError::named(
                "TypeError",
                format!(
                    "unhashable type: '{}'",
                    pyrust_core::builtin_type_name(item)
                ),
            )),
        }
    }

    // Frozensets iterate via materialisation through `iter_values()` (which
    // calls `as_items` directly) rather than via `iter_next`.  Leaving
    // `is_iterable()` at its default `false` keeps the VM on the
    // materialisation path; iter_next is intentionally unset.

    fn has_method(&self, name: &str) -> bool {
        matches!(
            name,
            "copy"
                | "union"
                | "intersection"
                | "difference"
                | "symmetric_difference"
                | "issubset"
                | "issuperset"
                | "isdisjoint"
        )
    }

    fn to_key(&self, state: &BuiltinState) -> Option<PyKey> {
        let items = borrow_items(state)?;
        // Content-based hashable key.  Canonicalise by sorting the inner
        // keys' Debug form so different insertion orders compare equal.
        let mut keys: Vec<PyKey> = items.iter().cloned().collect();
        keys.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        Some(PyKey::FrozenSet(keys))
    }

    fn call_method(
        &self,
        state: &BuiltinState,
        method: &str,
        args: Vec<Value>,
        _kwargs: &IndexMap<String, Value>,
    ) -> Result<Value> {
        let rc = borrow_items(state)
            .ok_or_else(|| PyError::Runtime("internal: bad frozenset state".to_string()))?;
        // Treat the frozenset as an immutable set for non-mutating methods.
        // Clone into a regular IndexSet, call the method, and (for the
        // result-returning ones) re-wrap if necessary.
        let mut items: IndexSet<PyKey> = (*rc).clone();
        let result = crate::set::call(method, &mut items, args)?;
        if matches!(
            method,
            "copy" | "union" | "intersection" | "difference" | "symmetric_difference"
        ) {
            if let ValueKind::Set(s) = result.kind() {
                return Ok(frozenset(s.clone()));
            }
        }
        Ok(result)
    }
}

/// Construct a frozenset Value from an `IndexSet`.
pub fn frozenset(items: IndexSet<PyKey>) -> Value {
    frozenset_rc(Rc::new(items))
}

/// Construct a frozenset Value from an existing `Rc<IndexSet>` — useful when
/// converting from a `Set` value while sharing storage.
pub fn frozenset_rc(items: Rc<IndexSet<PyKey>>) -> Value {
    let state: Box<dyn Any> = Box::new(FrozenSetState { items });
    Value::builtin_object(FROZENSET_OPS, state)
}

/// Extract the inner `Rc<IndexSet<PyKey>>` from a frozenset Value, or None if
/// the value isn't a frozenset.  Used by interpreter code that needs direct
/// content access (iteration, key conversion, etc.).
pub fn as_items(value: &Value) -> Option<Rc<IndexSet<PyKey>>> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if ops.type_name() != TYPE_NAME {
        return None;
    }
    borrow_items(state)
}

fn borrow_items(state: &BuiltinState) -> Option<Rc<IndexSet<PyKey>>> {
    let borrow = state.borrow();
    borrow
        .downcast_ref::<FrozenSetState>()
        .map(|s| Rc::clone(&s.items))
}

fn key_repr(key: &PyKey) -> String {
    match key {
        PyKey::Int(v) => v.to_string(),
        PyKey::Float(v) => f64::from_bits(*v).to_string(),
        PyKey::Str(v) => format!("'{}'", v),
        PyKey::Bool(v) => if *v { "True" } else { "False" }.to_string(),
        PyKey::None => "None".to_string(),
        PyKey::FrozenSet(items) => {
            if items.is_empty() {
                "frozenset()".to_string()
            } else {
                let inner = items.iter().map(key_repr).collect::<Vec<_>>().join(", ");
                format!("frozenset({{{inner}}})")
            }
        }
    }
}
