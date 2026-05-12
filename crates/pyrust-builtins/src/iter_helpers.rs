//! `enumerate`, `zip`, `reversed` — iterator helper builtins.
//!
//! Eliminated from `pyrust-core`'s Tier 1 (#295) because each one wraps an
//! arbitrary source iterator: the payload isn't small enough to justify a
//! dedicated `Opaque` variant.  The values live here as `BuiltinObject`s,
//! materialised eagerly (matching the previous behavior) — the source is
//! drained into a `Vec<Value>` at construction time.

use std::any::Any;
use std::cell::RefCell;

use indexmap::IndexMap;
use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, Result, Value};

/// Common cursor state used by all three helpers.  We keep a single
/// materialised buffer plus a `RefCell` cursor — iteration is O(1) per step
/// without re-walking the source.
pub struct IterHelperState {
    items: Vec<Value>,
    pos: RefCell<usize>,
}

impl IterHelperState {
    fn new(items: Vec<Value>) -> Self {
        Self {
            items,
            pos: RefCell::new(0),
        }
    }

    fn next(&self) -> Option<Value> {
        let mut pos = self.pos.borrow_mut();
        if *pos < self.items.len() {
            let v = self.items[*pos].clone();
            *pos += 1;
            Some(v)
        } else {
            None
        }
    }
}

// ── enumerate ────────────────────────────────────────────────────────────────

pub struct EnumerateOps;
pub const ENUMERATE_OPS: &EnumerateOps = &EnumerateOps;
pub const ENUMERATE_TYPE_NAME: &str = "enumerate";

impl BuiltinTypeOps for EnumerateOps {
    fn type_name(&self) -> &'static str {
        ENUMERATE_TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<enumerate object>".to_string()
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn is_iterable(&self) -> bool {
        true
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        next_helper(state)
    }
}

/// `enumerate(source, start=0)` — yields `(start + i, x)` for each `x`
/// drawn from `source`.  Caller provides the already-materialised source.
pub fn enumerate(source_items: Vec<Value>, start: i64) -> Value {
    let items: Vec<Value> = source_items
        .into_iter()
        .enumerate()
        .map(|(i, v)| Value::tuple(vec![Value::int(i as i64 + start), v]))
        .collect();
    let state: Box<dyn Any> = Box::new(IterHelperState::new(items));
    Value::builtin_object(ENUMERATE_OPS, state)
}

// ── zip ──────────────────────────────────────────────────────────────────────

pub struct ZipOps;
pub const ZIP_OPS: &ZipOps = &ZipOps;
pub const ZIP_TYPE_NAME: &str = "zip";

impl BuiltinTypeOps for ZipOps {
    fn type_name(&self) -> &'static str {
        ZIP_TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<zip object>".to_string()
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn is_iterable(&self) -> bool {
        true
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        next_helper(state)
    }
}

/// `zip(it1, it2, ...)` — yields tuples drawn pointwise from each source.
/// `sources` is the already-materialised list of per-source items.
pub fn zip(sources: Vec<Vec<Value>>) -> Value {
    let len = sources.iter().map(|v| v.len()).min().unwrap_or(0);
    let items: Vec<Value> = (0..len)
        .map(|i| Value::tuple(sources.iter().map(|v| v[i].clone()).collect()))
        .collect();
    let state: Box<dyn Any> = Box::new(IterHelperState::new(items));
    Value::builtin_object(ZIP_OPS, state)
}

// ── reversed ─────────────────────────────────────────────────────────────────

pub struct ReversedOps;
pub const REVERSED_OPS: &ReversedOps = &ReversedOps;
pub const REVERSED_TYPE_NAME: &str = "list_reverseiterator";

impl BuiltinTypeOps for ReversedOps {
    fn type_name(&self) -> &'static str {
        REVERSED_TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<list_reverseiterator object>".to_string()
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn is_iterable(&self) -> bool {
        true
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        next_helper(state)
    }
}

/// `reversed(source)` — yields `source` items in reverse order.
pub fn reversed(mut source_items: Vec<Value>) -> Value {
    source_items.reverse();
    let state: Box<dyn Any> = Box::new(IterHelperState::new(source_items));
    Value::builtin_object(REVERSED_OPS, state)
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn next_helper(state: &BuiltinState) -> Result<Option<Value>> {
    let borrow = state.borrow();
    let s = borrow
        .downcast_ref::<IterHelperState>()
        .ok_or_else(|| PyError::Runtime("internal: bad iter helper state".to_string()))?;
    Ok(s.next())
}

// Silence the import-unused warning if IndexMap is only used inside trait
// default args — keep it imported for future call_method impls.
const _: fn() -> Option<IndexMap<String, Value>> = || None;
