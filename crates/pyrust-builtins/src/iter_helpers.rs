//! `enumerate`, `zip`, `reversed` — iterator helper builtins.
//!
//! Eliminated from `pyrust-core`'s Tier 1 (#295) because each one wraps an
//! arbitrary source iterator: the payload isn't small enough to justify a
//! dedicated `Opaque` variant.  The values live here as `BuiltinObject`s.
//!
//! Materialisation of the source happens **lazily on first `iter_next`**, not
//! at construction time, so side effects of the source (e.g. `open()` opening
//! a file) are deferred to iteration start — matching the previous behavior.

use std::cell::RefCell;

use pyrust_core::{BuiltinState, BuiltinTypeOps, PyError, Result, Value, iter_values_via_registry};

// ── enumerate ────────────────────────────────────────────────────────────────

pub struct EnumerateState {
    source: Value,
    start: i64,
    /// Materialised lazily on first `iter_next` call.
    items: RefCell<Option<Vec<Value>>>,
    pos: RefCell<usize>,
}

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
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<EnumerateState>()
            .ok_or_else(|| PyError::Runtime("internal: bad enumerate state".to_string()))?;
        ensure_materialized(&s.items, || iter_values_via_registry(&s.source))?;
        let items_ref = s.items.borrow();
        let items = items_ref.as_ref().unwrap();
        let mut pos = s.pos.borrow_mut();
        if *pos < items.len() {
            let tuple = Value::tuple(vec![Value::int(*pos as i64 + s.start), items[*pos].clone()]);
            *pos += 1;
            Ok(Some(tuple))
        } else {
            Ok(None)
        }
    }
}

/// `enumerate(source, start=0)` — yields `(start + i, x)` for each `x` in
/// `source`.  Source is *not* drained yet — the first `iter_next` call
/// triggers materialisation.
pub fn enumerate(source: Value, start: i64) -> Value {
    let state = EnumerateState {
        source,
        start,
        items: RefCell::new(None),
        pos: RefCell::new(0),
    };
    Value::builtin_object(ENUMERATE_OPS, Box::new(state))
}

// ── zip ──────────────────────────────────────────────────────────────────────

pub struct ZipState {
    sources: Vec<Value>,
    /// Per-source materialised vecs; lazy.
    columns: RefCell<Option<Vec<Vec<Value>>>>,
    /// Min length across columns.
    len: RefCell<usize>,
    pos: RefCell<usize>,
    /// `strict=True` raises `ValueError` when sources have unequal length.
    strict: bool,
}

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
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ZipState>()
            .ok_or_else(|| PyError::Runtime("internal: bad zip state".to_string()))?;
        if s.columns.borrow().is_none() {
            let mut cols: Vec<Vec<Value>> = Vec::with_capacity(s.sources.len());
            for src in &s.sources {
                cols.push(iter_values_via_registry(src)?);
            }
            let len = cols.iter().map(|v| v.len()).min().unwrap_or(0);
            *s.columns.borrow_mut() = Some(cols);
            *s.len.borrow_mut() = len;
        }
        let cols_ref = s.columns.borrow();
        let cols = cols_ref.as_ref().unwrap();
        let len = *s.len.borrow();
        let mut pos = s.pos.borrow_mut();
        if *pos < len {
            let row: Vec<Value> = cols.iter().map(|c| c[*pos].clone()).collect();
            *pos += 1;
            Ok(Some(Value::tuple(row)))
        } else {
            // Past the shortest column.  In strict mode we must distinguish:
            //   - the *first* iterator to stop sits at index `i`; all
            //     iterators at indices < `i` still have a value at row
            //     `*pos`, so the stopped iterator is "shorter than args
            //     1..i" (when i > 0), or
            //   - the first iterator (index 0) stopped, and a later
            //     iterator at index `j` still has a value at row `*pos`, so
            //     "argument j+1 is longer than args 1..j" (when j > 0; for
            //     j == 1 just "argument 2 is longer than argument 1").
            if s.strict && cols.len() > 1 {
                // Find the first iterator that ran out at row `*pos`.
                let stopped_at = cols.iter().position(|c| c.len() == *pos);
                if let Some(i) = stopped_at {
                    if i > 0 {
                        return Err(PyError::named("ValueError", shorter_message(i)));
                    }
                    // i == 0: find next iterator that still has a value.
                    if let Some(j) = cols.iter().position(|c| c.len() > *pos) {
                        return Err(PyError::named("ValueError", longer_message(j)));
                    }
                }
            }
            Ok(None)
        }
    }
}

/// `zip(it1, it2, ..., strict=False)` — yields tuples drawn pointwise from
/// each source.  Sources are drained on first `iter_next`.  When `strict` is
/// true, a length mismatch raises `ValueError` matching CPython's wording.
pub fn zip(sources: Vec<Value>, strict: bool) -> Value {
    let state = ZipState {
        sources,
        columns: RefCell::new(None),
        len: RefCell::new(0),
        pos: RefCell::new(0),
        strict,
    };
    Value::builtin_object(ZIP_OPS, Box::new(state))
}

/// CPython wording for "argument N is shorter than ..." where `i` is the
/// 0-based index of the iterator that ran short.
fn shorter_message(i: usize) -> String {
    if i == 1 {
        format!("zip() argument {} is shorter than argument 1", i + 1)
    } else {
        format!("zip() argument {} is shorter than arguments 1-{}", i + 1, i)
    }
}

/// CPython wording for "argument N is longer than ..." where `j` is the
/// 0-based index of the iterator that still had values when earlier ones
/// (indices 0..j) had all stopped.
fn longer_message(j: usize) -> String {
    if j == 1 {
        format!("zip() argument {} is longer than argument 1", j + 1)
    } else {
        format!("zip() argument {} is longer than arguments 1-{}", j + 1, j)
    }
}

// ── reversed ─────────────────────────────────────────────────────────────────

pub struct ReversedState {
    source: Value,
    items: RefCell<Option<Vec<Value>>>,
    /// Cursor walks from the end of `items` down to 0.  `usize::MAX` means
    /// "not yet initialised"; set to `items.len()` once materialised.
    pos: RefCell<usize>,
}

pub struct ReversedOps;
pub const REVERSED_OPS: &ReversedOps = &ReversedOps;
pub const REVERSED_TYPE_NAME: &str = "list_reverseiterator";

impl BuiltinTypeOps for ReversedOps {
    fn type_name(&self) -> &'static str {
        REVERSED_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let addr = std::rc::Rc::as_ptr(state) as usize;
        format!("<list_reverseiterator object at 0x{addr:x}>")
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn is_iterable(&self) -> bool {
        true
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ReversedState>()
            .ok_or_else(|| PyError::Runtime("internal: bad reversed state".to_string()))?;
        if s.items.borrow().is_none() {
            let items = iter_values_via_registry(&s.source)?;
            let len = items.len();
            *s.items.borrow_mut() = Some(items);
            *s.pos.borrow_mut() = len;
        }
        let items_ref = s.items.borrow();
        let items = items_ref.as_ref().unwrap();
        let mut pos = s.pos.borrow_mut();
        if *pos > 0 {
            *pos -= 1;
            Ok(Some(items[*pos].clone()))
        } else {
            Ok(None)
        }
    }
}

/// `reversed(source)` — yields `source` items in reverse order.  Source is
/// drained on first `iter_next`.
pub fn reversed(source: Value) -> Value {
    let state = ReversedState {
        source,
        items: RefCell::new(None),
        pos: RefCell::new(0),
    };
    Value::builtin_object(REVERSED_OPS, Box::new(state))
}

// ── chain ────────────────────────────────────────────────────────────────────

pub struct ChainState {
    sources: Vec<Value>,
    /// Cursor over `sources`; advanced past each one as it's drained.
    source_idx: RefCell<usize>,
    /// Materialised items for the source at `source_idx`; refilled when we
    /// move on.  Holding the current source as a `Vec<Value>` (rather than
    /// re-iterating piecewise) keeps the surface aligned with the rest of
    /// this file's lazy helpers — `iter_values_via_registry` returns a
    /// `Vec`, so each *source* is drained eagerly but the chain as a whole
    /// only walks to the current source.
    current_items: RefCell<Option<Vec<Value>>>,
    pos: RefCell<usize>,
}

pub struct ChainOps;
pub const CHAIN_OPS: &ChainOps = &ChainOps;
pub const CHAIN_TYPE_NAME: &str = "itertools.chain";

impl BuiltinTypeOps for ChainOps {
    fn type_name(&self) -> &'static str {
        CHAIN_TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let addr = std::rc::Rc::as_ptr(state) as usize;
        format!("<itertools.chain object at 0x{addr:x}>")
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn is_iterable(&self) -> bool {
        true
    }

    fn iter_next(&self, state: &BuiltinState) -> Result<Option<Value>> {
        let borrow = state.borrow();
        let s = borrow
            .downcast_ref::<ChainState>()
            .ok_or_else(|| PyError::Runtime("internal: bad chain state".to_string()))?;
        loop {
            // Drain the current source.
            {
                let items_ref = s.current_items.borrow();
                if let Some(items) = items_ref.as_ref() {
                    let mut pos = s.pos.borrow_mut();
                    if *pos < items.len() {
                        let v = items[*pos].clone();
                        *pos += 1;
                        return Ok(Some(v));
                    }
                }
            }
            // Move to the next source (or stop).
            let mut idx = s.source_idx.borrow_mut();
            if *idx >= s.sources.len() {
                return Ok(None);
            }
            let next = iter_values_via_registry(&s.sources[*idx])?;
            *idx += 1;
            *s.current_items.borrow_mut() = Some(next);
            *s.pos.borrow_mut() = 0;
        }
    }
}

/// `itertools.chain(*iterables)` — concatenate iterables lazily.  Each
/// source is materialised when we reach it during iteration, so the
/// pattern `chain(huge_source_1, huge_source_2)` only pays for what's
/// actually consumed.
pub fn chain(sources: Vec<Value>) -> Value {
    let state = ChainState {
        sources,
        source_idx: RefCell::new(0),
        current_items: RefCell::new(None),
        pos: RefCell::new(0),
    };
    Value::builtin_object(CHAIN_OPS, Box::new(state))
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn ensure_materialized<F>(slot: &RefCell<Option<Vec<Value>>>, fill: F) -> Result<()>
where
    F: FnOnce() -> Result<Vec<Value>>,
{
    if slot.borrow().is_none() {
        let items = fill()?;
        *slot.borrow_mut() = Some(items);
    }
    Ok(())
}
