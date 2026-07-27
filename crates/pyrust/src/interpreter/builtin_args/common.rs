// ─── Trait ────────────────────────────────────────────────────────────────────

/// Convert a `Value` into a typed Rust local with CPython-style error
/// messages.  Implemented by every wrapper used in a typed builtin signature.
///
/// The `'a` lifetime is the lifetime of the `Value` reference handed to
/// `try_from_value`.  Owned wrappers (like `PyInt`) don't use it; borrowing
/// wrappers (like `PyStr<'a>` carrying a `Cow<'a, str>`) tie their interior
/// reference back to the call's args slice — zero-copy when the value is
/// already a string in the VM's frame.
pub(crate) trait FromValue<'a>: Sized {
    /// Python-level type name for error messages ("int", "str", ...).
    /// Used by both the missing-arg error path and `try_from_value`'s
    /// "must be X, not Y" message.
    const PY_TYPE_NAME: &'static str;

    /// Attempt the conversion.  `fn_name` is the Python-level fully-qualified
    /// name of the calling builtin (e.g. `"math.sqrt"`); `arg_name` is the
    /// parameter name (e.g. `"x"`) used in error messages.
    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self>;

    /// Allocation-free type-match predicate used by overload dispatch.
    /// Returns `true` iff `try_from_value` would succeed for this value.
    /// Takes `&'a Value` so the default can delegate to `try_from_value`
    /// without an unsafe lifetime extension; impls with a cheap kind-only
    /// predicate path should override for speed (and to keep the
    /// dispatcher allocation-free).
    fn matches(value: &'a Value) -> bool {
        Self::try_from_value(value, "", "").is_ok()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn type_error<S: Into<String>>(msg: S) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn must_be_error(fn_name: &str, arg_name: &str, expected: &str, actual: &Value) -> PyError {
    type_error(format!(
        "{fn_name}() argument '{arg_name}' must be {expected}, not {}",
        pyrust_core::builtin_type_name(actual),
    ))
}
