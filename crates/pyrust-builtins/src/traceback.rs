//! Minimal `traceback` built-in type for pyrust.
//!
//! CPython stores exception tracebacks as linked-list `PyTracebackObject`
//! values with frame/lineno/lasti slots.  Pyrust does not yet track
//! per-instruction line numbers, so this implementation is a lightweight
//! sentinel: a `BuiltinObject` whose type name is `"traceback"`.  It
//! satisfies `type(tb).__name__ == "traceback"` and `tb is not None`, which
//! are the observable properties required for parity with user code that
//! catches exceptions and inspects `__traceback__`.

use std::any::Any;

use pyrust_core::{BuiltinState, BuiltinTypeOps, Value};

pub const TYPE_NAME: &str = "traceback";
pub const TRACEBACK_OPS: &TracebackOps = &TracebackOps;

pub struct TracebackState;

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
}

/// Construct a traceback sentinel value.
pub fn make_traceback() -> Value {
    let state: Box<dyn Any> = Box::new(TracebackState);
    Value::builtin_object(TRACEBACK_OPS, state)
}

/// Returns `true` if `value` is a traceback object.
pub fn is_traceback(value: &Value) -> bool {
    matches!(value.kind(), pyrust_core::ValueKind::BuiltinObject { ops, .. } if ops.type_name() == TYPE_NAME)
}
