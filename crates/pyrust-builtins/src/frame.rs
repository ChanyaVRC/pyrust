//! `frame` object — a read-only snapshot of a VM execution frame.
//!
//! CPython exposes live frame objects via `sys._getframe()`, generator
//! `gi_frame`, and traceback `tb_frame`.  pyrust's VM keeps its frames in
//! register files that are not directly addressable from Python, so this is a
//! lightweight read-only *snapshot* `BuiltinObject` capturing the attributes
//! `inspect` / `logging` / `traceback` most commonly read: `f_code`,
//! `f_lineno`, `f_back`, `f_globals`, `f_locals`, `f_builtins` (issue #2171).
//!
//! Because the snapshot is taken at construction time it does not track later
//! mutation of the underlying frame; this is sufficient for the introspection
//! use cases (debuggers/loggers read the frame at the moment they obtain it).

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
            // `f_builtins` is the builtins namespace; pyrust resolves builtins
            // through the env chain rather than a dedicated dict, so surface the
            // globals dict (a non-error, mapping-typed best effort).
            "f_builtins" => Some(s.globals.clone()),
            // `f_lasti` is the last executed bytecode offset; pyrust exposes no
            // stable bytecode offset, so report `-1` (the value CPython uses for
            // a frame that has not yet executed an instruction).
            "f_lasti" => Some(Value::int(-1)),
            // `f_trace` is the per-frame trace function; pyrust has no tracing
            // hook, so it is always `None` (matching an untraced CPython frame).
            "f_trace" => Some(Value::none()),
            _ => None,
        }
    }
}

/// Construct a `frame` snapshot value.
pub fn frame(code: Value, lineno: i64, back: Value, globals: Value, locals: Value) -> Value {
    let state: Box<dyn Any> = Box::new(FrameState {
        code,
        lineno,
        back,
        globals,
        locals,
    });
    Value::builtin_object(FRAME_OPS, state)
}

/// Returns `true` if `value` is a frame object.
pub fn is_frame(value: &Value) -> bool {
    matches!(value.kind(), pyrust_core::ValueKind::BuiltinObject { ops, .. } if builtin_ops_is::<FrameOps>(ops))
}
