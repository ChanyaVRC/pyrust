//! `traceback` built-in type for pyrust.
//!
//! CPython stores exception tracebacks as a linked list of `PyTracebackObject`
//! nodes, each carrying `tb_frame` (the frame executing at this level),
//! `tb_next` (the next *inner* traceback node, or `None` at the innermost),
//! `tb_lineno` (the source line), and `tb_lasti` (the last bytecode offset).
//!
//! pyrust builds an equivalent walkable chain from the frames captured lazily
//! as an exception unwinds (issue #2165), so user code can `while tb: tb =
//! tb.tb_next` and read `tb.tb_lineno` / `tb.tb_frame.f_code.co_name`
//! (issue #2170).  `tb_lasti` is a best-effort value (pyrust does not expose a
//! stable bytecode offset, so it is `-1`, which CPython also uses for frames
//! that have not yet executed an instruction).

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value};

pub const TYPE_NAME: &str = "traceback";
pub const TRACEBACK_OPS: &TracebackOps = &TracebackOps;

/// Backing state for one node in a traceback chain.
pub struct TracebackState {
    /// `tb_frame` — the frame object executing at this level.
    pub frame: Value,
    /// `tb_next` — the next inner traceback node, or `None` at the innermost.
    pub next: Value,
    /// `tb_lineno` — the 1-based source line raising/propagating here.
    pub lineno: i64,
    /// `tb_lasti` — last bytecode offset (best-effort `-1`).
    pub lasti: i64,
}

pub struct TracebackOps;

impl BuiltinTypeOps for TracebackOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, _state: &BuiltinState) -> String {
        "<traceback object>".to_string()
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<TracebackState>()?;
        match name {
            "tb_frame" => Some(s.frame.clone()),
            "tb_next" => Some(s.next.clone()),
            "tb_lineno" => Some(Value::int(s.lineno)),
            "tb_lasti" => Some(Value::int(s.lasti)),
            _ => None,
        }
    }
}

/// Construct a traceback node value.
pub fn traceback_node(frame: Value, next: Value, lineno: i64, lasti: i64) -> Value {
    let state: Box<dyn Any> = Box::new(TracebackState {
        frame,
        next,
        lineno,
        lasti,
    });
    Value::builtin_object(TRACEBACK_OPS, state)
}

/// Returns `true` if `value` is a traceback object.
pub fn is_traceback(value: &Value) -> bool {
    matches!(value.kind(), pyrust_core::ValueKind::BuiltinObject { ops, .. } if ops.type_name() == TYPE_NAME)
}

/// Read attribute `name` from any built-in object value (`frame`, `code`, …)
/// by dispatching to its `BuiltinTypeOps::getattr`.  Returns `None` when the
/// value is not a built-in object or has no such attribute.
fn builtin_getattr(value: &Value, name: &str) -> Option<Value> {
    match value.kind() {
        pyrust_core::ValueKind::BuiltinObject { ops, state } => ops.getattr(&state, name),
        _ => None,
    }
}

/// Walk a traceback chain and return `(co_name, tb_lineno)` for each node,
/// outermost-first (the chain's natural `tb_next` order).  Returns an empty
/// `Vec` for a non-traceback value.
///
/// Used by the uncaught-exception stderr formatter (issue #2404) to derive its
/// frame list from the exception's prepended `__traceback__` chain instead of
/// the captured-frame snapshot, so a re-raised exception's printed traceback
/// matches the Python-visible `__traceback__` walk.
pub fn walk_frames(value: &Value) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    let mut cur = value.clone();
    loop {
        let pyrust_core::ValueKind::BuiltinObject { ops, state } = cur.kind() else {
            break;
        };
        if ops.type_name() != TYPE_NAME {
            break;
        }
        let (frame, lineno, next) = {
            let borrow = state.borrow();
            match borrow.downcast_ref::<TracebackState>() {
                Some(s) => (s.frame.clone(), s.lineno, s.next.clone()),
                None => break,
            }
        };
        // co_name via tb_frame.f_code.co_name.
        let name = builtin_getattr(&frame, "f_code")
            .and_then(|code| builtin_getattr(&code, "co_name"))
            .and_then(|n| n.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "<unknown>".to_string());
        out.push((name, lineno));
        cur = next;
    }
    out
}

/// Count the number of nodes in a traceback chain by following `tb_next`.
/// Returns `0` for a non-traceback value.
pub fn chain_len(value: &Value) -> usize {
    let mut len = 0usize;
    let mut cur = value.clone();
    loop {
        let pyrust_core::ValueKind::BuiltinObject { ops, state } = cur.kind() else {
            break;
        };
        if ops.type_name() != TYPE_NAME {
            break;
        }
        let next = {
            let borrow = state.borrow();
            match borrow.downcast_ref::<TracebackState>() {
                Some(s) => s.next.clone(),
                None => break,
            }
        };
        len += 1;
        cur = next;
    }
    len
}
