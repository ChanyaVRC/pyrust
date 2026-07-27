/// Construct a [`PyError::Named`] with a static class literal and a message.
///
/// Collapses the ubiquitous four-line
/// `PyError::named("TypeError", format!("…", …))` shape into one expression.
/// It evaluates to a `PyError` *value* (not an early return), so it works in
/// every position the constructor did — `Err(py_err!(...))`,
/// `.ok_or_else(|| py_err!(...))`, a match arm, etc.
///
/// Two arms, one rule: **a string-literal message is run through `format!`**
/// (so inline captures `{x}` and explicit `"…{}", arg` both work, matching the
/// original `format!(…)` sites); **any other expression is taken verbatim** as
/// an `impl Into<String>` (a `String`/`&str` variable, or a literal that must
/// not be formatted — pass it as `"text".to_string()`).
///
/// The message is only built on evaluation (the error path).  Prefer the
/// per-exception sugar ([`type_err!`], [`value_err!`], …); this is the escape
/// hatch for the rarer classes.
#[macro_export]
macro_rules! py_err {
    ($cls:literal, $fmt:literal $(, $($arg:tt)*)?) => {
        $crate::PyError::named($cls, format!($fmt $(, $($arg)*)?))
    };
    ($cls:literal, $msg:expr $(,)?) => {
        $crate::PyError::named($cls, $msg)
    };
}

/// Per-exception sugar over [`py_err!`], so call sites read
/// `type_err!("…", x)` instead of `py_err!("TypeError", "…", x)`.  Each forwards
/// to `py_err!` with the class baked in, preserving the literal / format / expr
/// arms (and the lazy `format!`).
#[macro_export]
macro_rules! type_err {
    ($($t:tt)+) => { $crate::py_err!("TypeError", $($t)+) };
}
#[macro_export]
macro_rules! value_err {
    ($($t:tt)+) => { $crate::py_err!("ValueError", $($t)+) };
}
#[macro_export]
macro_rules! index_err {
    ($($t:tt)+) => { $crate::py_err!("IndexError", $($t)+) };
}
#[macro_export]
macro_rules! overflow_err {
    ($($t:tt)+) => { $crate::py_err!("OverflowError", $($t)+) };
}
#[macro_export]
macro_rules! zerodiv_err {
    ($($t:tt)+) => { $crate::py_err!("ZeroDivisionError", $($t)+) };
}
#[macro_export]
macro_rules! runtime_err {
    ($($t:tt)+) => { $crate::py_err!("RuntimeError", $($t)+) };
}

/// Build the `TypeError` raised when a type-qualified builtin method (an
/// unbound descriptor such as `str.__len__` / `int.__add__`) is called with no
/// `self` argument, e.g. `str.__len__()`.  Single source of truth for the
/// descriptor receiver-presence message so the ~59 open-coded copies can't
/// drift.
///
/// CPython 3.12 raises one of two messages depending on the descriptor kind:
///
/// * **slot wrapper** (dunders backed by a type slot — `__len__`, `__add__`,
///   comparison ops, `__repr__`, `__getitem__`, …):
///   `"descriptor '<m>' of '<type>' object needs an argument"` — the default arm.
/// * **method_descriptor** (C-level methods — `str.upper`, `list.append`,
///   `int.conjugate`, and a few dunders like `object.__sizeof__` /
///   `float.__trunc__`): `"unbound method <type>.<m>() needs an argument"` — the
///   `method` arm.
#[macro_export]
macro_rules! descriptor_needs_arg {
    ($method:expr, $type_name:expr $(,)?) => {
        $crate::PyError::named(
            "TypeError",
            format!(
                "descriptor '{}' of '{}' object needs an argument",
                $method, $type_name
            ),
        )
    };
    ($method:expr, $type_name:expr, method $(,)?) => {
        $crate::PyError::named(
            "TypeError",
            format!(
                "unbound method {}.{}() needs an argument",
                $type_name, $method
            ),
        )
    };
}

/// Build the `TypeError` raised when a type-qualified builtin method (an
/// unbound descriptor) receives a `self` argument of the wrong type, e.g.
/// `str.__len__(5)`.  Single source of truth for the descriptor receiver-type
/// message so the ~27 open-coded copies can't drift.
///
/// Like `descriptor_needs_arg!`, CPython 3.12 picks the wording by descriptor
/// kind:
///
/// * **slot wrapper**: `"descriptor '<m>' requires a '<type>' object but
///   received a '<actual>'"` — the default 3-arg arm.
/// * **method_descriptor**: `"descriptor '<m>' for '<type>' objects doesn't
///   apply to a '<actual>' object"` — the `method` arm.
///
/// The 2-arg arm (no actual type) is a defensive fallback for the native
/// `<seq>.__getitem__` super() helpers, whose wrong-type branch is unreachable
/// from ordinary Python code.
#[macro_export]
macro_rules! descriptor_requires {
    ($method:expr, $type_name:expr, $actual:expr, method $(,)?) => {
        $crate::PyError::named(
            "TypeError",
            format!(
                "descriptor '{}' for '{}' objects doesn't apply to a '{}' object",
                $method, $type_name, $actual
            ),
        )
    };
    ($method:expr, $type_name:expr, $actual:expr $(,)?) => {
        $crate::PyError::named(
            "TypeError",
            format!(
                "descriptor '{}' requires a '{}' object but received a '{}'",
                $method, $type_name, $actual
            ),
        )
    };
    ($method:expr, $type_name:expr $(,)?) => {
        $crate::PyError::named(
            "TypeError",
            format!(
                "descriptor '{}' requires a '{}' object",
                $method, $type_name
            ),
        )
    };
}
