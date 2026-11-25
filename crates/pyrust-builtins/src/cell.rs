//! `cell` object — an element of `function.__closure__` (issues #2106 / #452).
//!
//! Wraps a live [`CellSlot`] shared with the owning frame and every closure
//! capturing that variable. Reading `cell_contents` reflects live mutations and
//! raises `ValueError: Cell is empty` when unbound; assigning or deleting
//! `cell_contents` updates the shared cell directly so closures observe the
//! change immediately (matching CPython 3.12).

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, CellSlot, PyError, Result, Value, ValueKind};

pub struct CellState {
    /// Live shared cell backing this closure variable.
    pub slot: CellSlot,
}

pub struct CellOps;
pub const CELL_OPS: &CellOps = &CellOps;
pub const TYPE_NAME: &str = "cell";

impl BuiltinTypeOps for CellOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn identity_payload(&self, state: &BuiltinState) -> Option<u64> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<CellState>()?;
        Some(s.slot.as_ptr() as usize as u64)
    }

    fn repr(&self, state: &BuiltinState) -> String {
        // CPython: `<cell at 0x...: int object at 0x...>` or `<cell at 0x...: empty>`.
        let borrow = state.borrow();
        match borrow.downcast_ref::<CellState>() {
            Some(s) => match s.slot.get() {
                Some(contents) => {
                    let type_name = pyrust_core::builtin_type_name(&contents);
                    format!("<cell at 0x0: {type_name} object at 0x0>")
                }
                None => "<cell at 0x0: empty>".to_string(),
            },
            None => "<cell at 0x0: empty>".to_string(),
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn eq(&self, state: &BuiltinState, other: &Value) -> bool {
        let ValueKind::BuiltinObject {
            ops: other_ops,
            state: other_state,
        } = other.kind()
        else {
            return false;
        };
        if !pyrust_core::builtin_ops_is::<CellOps>(other_ops) {
            return false;
        }
        let a = {
            let borrow = state.borrow();
            let Some(s) = borrow.downcast_ref::<CellState>() else {
                return false;
            };
            s.slot.get()
        };
        let b = {
            let borrow = other_state.borrow();
            let Some(s) = borrow.downcast_ref::<CellState>() else {
                return false;
            };
            s.slot.get()
        };
        match (a, b) {
            (None, None) => true,
            (Some(va), Some(vb)) => va == vb,
            _ => false,
        }
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        if name != "cell_contents" {
            return None;
        }
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<CellState>()?;
        s.slot.get()
    }

    fn setattr(&self, state: &BuiltinState, name: &str, value: Value) -> Result<()> {
        if name == "cell_contents" {
            let borrow = state.borrow();
            if let Some(s) = borrow.downcast_ref::<CellState>() {
                s.slot.set(value);
                return Ok(());
            }
        }
        Err(PyError::named(
            "AttributeError",
            format!("'cell' object has no attribute '{name}'"),
        ))
    }
}

/// Construct a `cell` object wrapping a live [`CellSlot`].
pub fn cell(slot: CellSlot) -> Value {
    let state: Box<dyn Any> = Box::new(CellState { slot });
    Value::builtin_object(CELL_OPS, state)
}

/// Construct a standalone `cell` object initialized with `contents`.
pub fn cell_with_value(contents: Value) -> Value {
    cell(CellSlot::new(contents))
}

/// Construct a standalone empty `cell` object (`types.CellType()`).
pub fn cell_empty() -> Value {
    cell(CellSlot::new_empty())
}

/// Extract the underlying shared [`CellSlot`] from a `cell` object.
pub fn cell_slot(value: &Value) -> Option<CellSlot> {
    let ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !pyrust_core::builtin_ops_is::<CellOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    borrow.downcast_ref::<CellState>().map(|s| s.slot.clone())
}

/// Return `true` if `value` is a `cell` object.
#[inline]
pub fn is_cell(value: &Value) -> bool {
    matches!(
        value.kind(),
        ValueKind::BuiltinObject { ops, .. } if pyrust_core::builtin_ops_is::<CellOps>(ops)
    )
}

/// Read `cell.cell_contents`, raising `ValueError: Cell is empty` when unbound.
pub fn get_cell_contents(value: &Value) -> Result<Value> {
    let ValueKind::BuiltinObject { state, .. } = value.kind() else {
        return Err(PyError::named("TypeError", "expected cell object"));
    };
    let slot = {
        let borrow = state.borrow();
        let Some(s) = borrow.downcast_ref::<CellState>() else {
            return Err(PyError::named("ValueError", "Cell is empty"));
        };
        s.slot.clone()
    };
    slot.get()
        .ok_or_else(|| PyError::named("ValueError", "Cell is empty"))
}

/// Write `cell.cell_contents = contents`.
pub fn set_cell_contents(value: &Value, contents: Value) -> Result<()> {
    let ValueKind::BuiltinObject { state, .. } = value.kind() else {
        return Err(PyError::named("TypeError", "expected cell object"));
    };
    let borrow = state.borrow();
    if let Some(s) = borrow.downcast_ref::<CellState>() {
        s.slot.set(contents);
    }
    Ok(())
}

/// Delete `cell.cell_contents` (idempotent in CPython 3.12 — clearing an
/// already-empty cell succeeds silently).
pub fn delete_cell_contents(value: &Value) -> Result<()> {
    let ValueKind::BuiltinObject { state, .. } = value.kind() else {
        return Err(PyError::named("TypeError", "expected cell object"));
    };
    let borrow = state.borrow();
    if let Some(s) = borrow.downcast_ref::<CellState>() {
        let _ = s.slot.take();
    }
    Ok(())
}
