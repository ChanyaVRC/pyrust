//! `frame` object — a read-only snapshot of a VM execution frame.
//!
//! CPython exposes live frame objects via `sys._getframe()`, generator
//! `gi_frame`, and traceback `tb_frame`.  pyrust's VM keeps its frames in
//! register files that are not directly addressable from Python, so this is a
//! lightweight read-only `BuiltinObject` exposing the attributes `inspect` /
//! `logging` / `traceback` most commonly read: `f_code`,
//! `f_lineno`, `f_back`, `f_globals`, `f_locals`, `f_builtins` (issue #2171).
//!
//! The interpreter weak-caches one object per live activation and refreshes
//! function `f_locals` when that attribute is read. Other captured values are
//! updated at their relevant introspection boundaries.

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value, builtin_ops_is};

/// Backing state for a `frame` object.
pub struct FrameState {
    /// `f_code` — the code object for this frame.
    pub code: Value,
    /// `f_lineno` — the 1-based current source line in this frame.
    pub lineno: i64,
    /// `f_back` — the next outer frame, or `None` for the outermost frame.
    pub back: Value,
    /// `f_globals` — the module globals dict.
    pub globals: Value,
    /// `f_locals` — the frame's local namespace snapshot.
    pub locals: Value,
    /// `f_builtins` — the builtins namespace dict (distinct from `f_globals`).
    pub builtins: Value,
    /// `f_lasti` — last executed bytecode offset (`-1` if not yet started).
    pub lasti: i64,
    /// `f_trace` — the per-frame local trace function, or `None`.
    pub trace: Value,
    /// `f_trace_lines` — whether `'line'` events are emitted for this frame.
    pub trace_lines: bool,
    /// `f_trace_opcodes` — whether `'opcode'` events are emitted for this frame.
    pub trace_opcodes: bool,
}

pub struct FrameOps;
pub const FRAME_OPS: &FrameOps = &FrameOps;
pub const TYPE_NAME: &str = "frame";

impl BuiltinTypeOps for FrameOps {
    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn repr(&self, state: &BuiltinState) -> String {
        let borrow = state.borrow();
        match borrow.downcast_ref::<FrameState>() {
            Some(s) => {
                let name = match s.code.kind() {
                    pyrust_core::ValueKind::BuiltinObject { state, .. } => state
                        .borrow()
                        .downcast_ref::<crate::code::CodeState>()
                        .map(|c| c.name.clone())
                        .unwrap_or_else(|| "?".to_string()),
                    _ => "?".to_string(),
                };
                format!("<frame at 0x0, line {}, code {}>", s.lineno, name)
            }
            None => "<frame object>".to_string(),
        }
    }

    fn truthy(&self, _state: &BuiltinState) -> bool {
        true
    }

    fn getattr(&self, state: &BuiltinState, name: &str) -> Option<Value> {
        let borrow = state.borrow();
        let s = borrow.downcast_ref::<FrameState>()?;
        match name {
            "f_code" => Some(s.code.clone()),
            "f_lineno" => Some(Value::int(s.lineno)),
            "f_back" => Some(s.back.clone()),
            "f_globals" => Some(s.globals.clone()),
            "f_locals" => Some(s.locals.clone()),
            "f_builtins" => Some(s.builtins.clone()),
            "f_lasti" => Some(Value::int(s.lasti)),
            // `f_trace` is the per-frame trace function (writable, defaults to None).
            "f_trace" => Some(s.trace.clone()),
            "f_trace_lines" => Some(Value::bool_(s.trace_lines)),
            "f_trace_opcodes" => Some(Value::bool_(s.trace_opcodes)),
            _ => None,
        }
    }

    fn has_method(&self, name: &str) -> bool {
        name == "clear"
    }
}

/// Construct a `frame` snapshot value with explicit `f_builtins` and `f_lasti`.
pub fn frame(
    code: Value,
    lineno: i64,
    back: Value,
    globals: Value,
    locals: Value,
    builtins: Value,
    lasti: i64,
) -> Value {
    let state: Box<dyn Any> = Box::new(FrameState {
        code,
        lineno,
        back,
        globals,
        locals,
        builtins,
        lasti,
        trace: Value::none(),
        trace_lines: true,
        trace_opcodes: false,
    });
    Value::builtin_object(FRAME_OPS, state)
}

/// Returns `true` if `value` is a frame object.
pub fn is_frame(value: &Value) -> bool {
    matches!(value.kind(), pyrust_core::ValueKind::BuiltinObject { ops, .. } if builtin_ops_is::<FrameOps>(ops))
}

/// Return the stored `f_back` value on a `frame` object without invoking
/// attribute dispatch (issue #3028).
pub fn stored_back(value: &Value) -> Option<Value> {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    borrow.downcast_ref::<FrameState>().map(|s| s.back.clone())
}

/// Store `back` into a `frame` object's `FrameState.back` slot (issue #3028).
pub fn set_back(value: &Value, back: Value) {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return;
    }
    let mut borrow = state.borrow_mut();
    if let Some(s) = borrow.downcast_mut::<FrameState>() {
        s.back = back;
    }
}

/// Return the frame's per-frame `f_trace` value (`Value::none()` if unset).
pub fn get_trace(value: &Value) -> Option<Value> {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return None;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return None;
    }
    let borrow = state.borrow();
    borrow.downcast_ref::<FrameState>().map(|s| s.trace.clone())
}

/// Set the frame's per-frame `f_trace` value.
pub fn set_trace(value: &Value, trace: Value) {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return;
    }
    let mut borrow = state.borrow_mut();
    if let Some(s) = borrow.downcast_mut::<FrameState>() {
        s.trace = trace;
    }
}

/// Return whether `f_trace_lines` is enabled on this frame.
pub fn get_trace_lines(value: &Value) -> bool {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return true;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return true;
    }
    let borrow = state.borrow();
    borrow
        .downcast_ref::<FrameState>()
        .is_none_or(|s| s.trace_lines)
}

/// Set `f_trace_lines` on this frame.
pub fn set_trace_lines(value: &Value, enabled: bool) {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return;
    }
    let mut borrow = state.borrow_mut();
    if let Some(s) = borrow.downcast_mut::<FrameState>() {
        s.trace_lines = enabled;
    }
}

/// Set `f_trace_opcodes` on this frame.
pub fn set_trace_opcodes(value: &Value, enabled: bool) {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return;
    }
    let mut borrow = state.borrow_mut();
    if let Some(s) = borrow.downcast_mut::<FrameState>() {
        s.trace_opcodes = enabled;
    }
}

/// Update `f_lineno` on this frame.
pub fn set_lineno(value: &Value, lineno: i64) {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return;
    }
    let mut borrow = state.borrow_mut();
    if let Some(s) = borrow.downcast_mut::<FrameState>() {
        s.lineno = lineno;
    }
}

/// Update `f_lasti` on this frame.
pub fn set_lasti(value: &Value, lasti: i64) {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return;
    }
    let mut borrow = state.borrow_mut();
    if let Some(s) = borrow.downcast_mut::<FrameState>() {
        s.lasti = lasti;
    }
}

/// Clear a non-executing frame's local bindings and trace hook (`frame.clear()`).
pub fn clear_inactive_frame(value: &Value) {
    let pyrust_core::ValueKind::BuiltinObject { ops, state } = value.kind() else {
        return;
    };
    if !builtin_ops_is::<FrameOps>(ops) {
        return;
    }
    let mut borrow = state.borrow_mut();
    if let Some(s) = borrow.downcast_mut::<FrameState>() {
        s.trace = Value::none();
        if !s.locals.is_identical_to(&s.globals) {
            let _ = s.locals.dict_clear();
        }
    }
}
