//! Typed argument wrappers + parsing for `pyrust_module!` builtins.
//!
//! # Wrappers
//!
//! Each wrapper (e.g. [`PyInt`], [`PyFloat`], [`PyStr`]) implements
//! [`FromValue`], which validates a `Value` against **exactly one** Python
//! type and produces a typed Rust local.  Wrappers are **strict 1:1** with
//! Python types: `PyFloat` accepts only `float`, `PyInt` accepts only `int`,
//! etc. — no implicit promotion.  See [`PyValue`] for the catch-all.
//!
//! # Overload dispatch
//!
//! Because wrappers are strict, builtins that handle multiple type
//! combinations (e.g. `pow(int, int)` vs `pow(float, float)`) declare one
//! `fn` per combination; `pyrust_module!` groups them by Python-level name
//! and generates a dispatcher that picks the first overload whose
//! parameters all match.  The [`FromValue::matches`] predicate is the
//! allocation-free type check used by the dispatcher.
//!
//! # Generated prelude (single-body case)
//!
//! For a typed-signature builtin the macro generates a prelude that:
//!
//! 1. Rejects unknown keyword arguments.
//! 2. Checks min / max positional arg counts.
//! 3. Resolves each parameter from positional + keyword args, applying
//!    `#[default(...)]` if supplied.
//! 4. Calls `FromValue::try_from_value` for the strict type check.
//! 5. Binds the result as a local with the parameter's name.
//!
//! After the prelude, the user-written body sees typed locals (`path: PyStr`,
//! `mode: PyStr`, etc.) and can call straightforwardly into native Rust APIs.

use std::ops::Deref;
use std::rc::Rc;

use indexmap::{IndexMap, IndexSet};

use crate::error::{PyError, Result};
use crate::value::{PyKey, Value, ValueKind};

use super::ExpandedCallArg;

// ─── Trait ────────────────────────────────────────────────────────────────────

/// Convert a `Value` into a typed Rust local with CPython-style error
/// messages.  Implemented by every wrapper used in a typed builtin signature.
pub(crate) trait FromValue: Sized {
    /// Python-level type name for error messages ("int", "str", ...).
    /// Used by both the missing-arg error path and `try_from_value`'s
    /// "must be X, not Y" message.
    const PY_TYPE_NAME: &'static str;

    /// Attempt the conversion.  `fn_name` is the Python-level fully-qualified
    /// name of the calling builtin (e.g. `"math.sqrt"`); `arg_name` is the
    /// parameter name (e.g. `"x"`) used in error messages.
    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self>;

    /// Allocation-free type-match predicate used by overload dispatch.
    /// Returns `true` iff `try_from_value` would succeed for this value.
    /// The default delegates to `try_from_value`; strict wrappers (where
    /// the conversion is just a kind check) should override for speed.
    fn matches(value: &Value) -> bool {
        // Default — implementations with a cheap predicate path should
        // override to avoid the (possibly allocating) full conversion.
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

// ─── PyInt ────────────────────────────────────────────────────────────────────

/// `int` argument.  Accepts `int` only — strict 1:1 with the Python type.
/// `bool` does not auto-coerce (declare a `PyBool` overload, or a `PyValue`
/// fallback, to handle it).
#[derive(Debug)]
pub(crate) struct PyInt(pub i64);

impl Deref for PyInt {
    type Target = i64;
    fn deref(&self) -> &i64 {
        &self.0
    }
}

impl FromValue for PyInt {
    const PY_TYPE_NAME: &'static str = "int";

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.kind() {
            ValueKind::Int(n) => Ok(PyInt(n)),
            _ => Err(must_be_error(fn_name, arg_name, "int", value)),
        }
    }

    fn matches(value: &Value) -> bool {
        matches!(value.kind(), ValueKind::Int(_))
    }
}

// ─── PyFloat ──────────────────────────────────────────────────────────────────

/// `float` argument.  Accepts `float` only — strict 1:1 with the Python type.
/// `int` and `bool` do not auto-coerce; declare additional overloads for
/// those combinations, or use a `PyValue` fallback for mixed-type handling.
#[derive(Debug)]
pub(crate) struct PyFloat(pub f64);

impl Deref for PyFloat {
    type Target = f64;
    fn deref(&self) -> &f64 {
        &self.0
    }
}

impl FromValue for PyFloat {
    const PY_TYPE_NAME: &'static str = "float";

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.kind() {
            ValueKind::Float(f) => Ok(PyFloat(f)),
            _ => Err(must_be_error(fn_name, arg_name, "float", value)),
        }
    }

    fn matches(value: &Value) -> bool {
        matches!(value.kind(), ValueKind::Float(_))
    }
}

// ─── PyStr ────────────────────────────────────────────────────────────────────

/// `str` argument.  Accepts `str` only — no auto-coercion (matches CPython
/// for APIs like `open(path, mode)` that require an actual string).
///
/// Stores an owned `String`; cheap to construct since builtin call boundaries
/// already allocate.  Derefs to `&str` so callers can pass it directly to
/// `&str`-taking APIs.
#[derive(Debug)]
pub(crate) struct PyStr(pub String);

impl Deref for PyStr {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for PyStr {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Ergonomic default-value construction: `#[default("r".into())] mode: PyStr`.
impl From<&str> for PyStr {
    fn from(s: &str) -> Self {
        PyStr(s.to_string())
    }
}

impl From<String> for PyStr {
    fn from(s: String) -> Self {
        PyStr(s)
    }
}

impl FromValue for PyStr {
    const PY_TYPE_NAME: &'static str = "str";

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.as_str() {
            Some(s) => Ok(PyStr(s.to_string())),
            None => Err(must_be_error(fn_name, arg_name, "str", value)),
        }
    }

    fn matches(value: &Value) -> bool {
        value.is_str()
    }
}

// ─── PyBool ───────────────────────────────────────────────────────────────────

/// `bool` argument.  Accepts `bool` only.  (CPython is lenient here, accepting
/// any truthy value, but typed APIs that want strict bool are common enough
/// to justify a separate wrapper from `PyValue`.)
#[derive(Debug)]
pub(crate) struct PyBool(pub bool);

impl Deref for PyBool {
    type Target = bool;
    fn deref(&self) -> &bool {
        &self.0
    }
}

impl FromValue for PyBool {
    const PY_TYPE_NAME: &'static str = "bool";

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.kind() {
            ValueKind::Bool(b) => Ok(PyBool(b)),
            _ => Err(must_be_error(fn_name, arg_name, "bool", value)),
        }
    }

    fn matches(value: &Value) -> bool {
        value.is_bool()
    }
}

// ─── PyBytes ──────────────────────────────────────────────────────────────────

/// `bytes` argument.  Stored as an Rc-shared `Vec<u8>` (matching the underlying
/// `Opaque::Bytes` representation), so cloning is cheap.
#[derive(Debug)]
pub(crate) struct PyBytes(pub Rc<Vec<u8>>);

impl Deref for PyBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl FromValue for PyBytes {
    const PY_TYPE_NAME: &'static str = "bytes";

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.kind() {
            ValueKind::Bytes(rc) => Ok(PyBytes(Rc::clone(rc))),
            _ => Err(must_be_error(fn_name, arg_name, "bytes", value)),
        }
    }

    fn matches(value: &Value) -> bool {
        matches!(value.kind(), ValueKind::Bytes(_))
    }
}

// ─── PyList / PyTuple / PyDict / PySet ────────────────────────────────────────
//
// These wrap the Value itself so the body can borrow the underlying slice /
// map / set with one method call.  No copy at construction time.

/// `list` argument.  Use [`PyList::as_slice`] to read elements.
#[derive(Debug)]
pub(crate) struct PyList(pub Value);

impl PyList {
    pub(crate) fn as_slice(&self) -> &[Value] {
        // SAFETY (no unsafe used): `try_from_value` verified is_list(),
        // and `Value::clone` preserves that.  `as_list()` is then infallible.
        self.0.as_list().expect("PyList wraps a list")
    }
}

impl FromValue for PyList {
    const PY_TYPE_NAME: &'static str = "list";

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyList(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "list", value))
        }
    }

    fn matches(value: &Value) -> bool {
        value.is_list()
    }
}

/// `tuple` argument.  Use [`PyTuple::as_slice`].
#[derive(Debug)]
pub(crate) struct PyTuple(pub Value);

impl PyTuple {
    pub(crate) fn as_slice(&self) -> &[Value] {
        self.0.as_tuple().expect("PyTuple wraps a tuple")
    }
}

impl FromValue for PyTuple {
    const PY_TYPE_NAME: &'static str = "tuple";

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyTuple(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "tuple", value))
        }
    }

    fn matches(value: &Value) -> bool {
        value.is_tuple()
    }
}

/// `dict` argument.  Use [`PyDict::as_map`].
#[derive(Debug)]
pub(crate) struct PyDict(pub Value);

impl PyDict {
    pub(crate) fn as_map(&self) -> &IndexMap<PyKey, Value> {
        self.0.as_dict().expect("PyDict wraps a dict")
    }
}

impl FromValue for PyDict {
    const PY_TYPE_NAME: &'static str = "dict";

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyDict(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "dict", value))
        }
    }

    fn matches(value: &Value) -> bool {
        matches!(value.kind(), ValueKind::Dict(_))
    }
}

/// `set` argument.  Use [`PySet::as_set`].
#[derive(Debug)]
pub(crate) struct PySet(pub Value);

impl PySet {
    pub(crate) fn as_set(&self) -> &IndexSet<PyKey> {
        match self.0.kind() {
            ValueKind::Set(s) => s,
            _ => unreachable!("PySet wraps a set"),
        }
    }
}

impl FromValue for PySet {
    const PY_TYPE_NAME: &'static str = "set";

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PySet(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "set", value))
        }
    }

    fn matches(value: &Value) -> bool {
        matches!(value.kind(), ValueKind::Set(_))
    }
}

// ─── PyValue (pass-through) ───────────────────────────────────────────────────

/// `Any` — accepts any value, no type checking.  Use when the builtin handles
/// its own polymorphism (e.g. `repr(obj)`, `id(obj)`).
#[derive(Debug)]
pub(crate) struct PyValue(pub Value);

impl Deref for PyValue {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}

impl FromValue for PyValue {
    const PY_TYPE_NAME: &'static str = "object";

    fn try_from_value(value: &Value, _fn_name: &str, _arg_name: &str) -> Result<Self> {
        Ok(PyValue(value.clone()))
    }

    fn matches(_value: &Value) -> bool {
        true
    }
}

// ─── Option<T> — for default-None args ────────────────────────────────────────
//
// An `Option<T>` argument accepts either:
//   - `Value::none()` → `None`
//   - a `T` (via `T::try_from_value`) → `Some(T)`
// Use together with `#[default(None)]` in the macro signature for clean
// "may be absent" semantics.

impl<T: FromValue> FromValue for Option<T> {
    const PY_TYPE_NAME: &'static str = T::PY_TYPE_NAME;

    fn try_from_value(value: &Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if value.is_none() {
            Ok(None)
        } else {
            T::try_from_value(value, fn_name, arg_name).map(Some)
        }
    }

    fn matches(value: &Value) -> bool {
        value.is_none() || T::matches(value)
    }
}

// ─── Generic positional+keyword extractor ─────────────────────────────────────
//
// The macro emits, per parameter, code shaped like:
//
//     let path = extract_arg::<PyStr>(
//         args, &positional, FN_NAME, "path",
//         /* pos_index = */ 0,
//         /* kw_allowed = */ true,
//         /* default = */ None,
//     )?;
//
// For a default value, the caller supplies it directly when the slot is
// missing (the macro inlines the literal expression).  This keeps the
// runtime hot path branch-free and lets defaults be arbitrary expressions
// (`PyStr("r".to_string())`, `Value::int(0)`, etc.).

/// Look up the `pos_index`-th positional or `arg_name`-keyword argument.
/// Returns the value to convert, or `None` if absent (caller substitutes
/// the default).
pub(crate) fn locate_arg<'a>(
    args: &'a [ExpandedCallArg],
    positional: &[&'a ExpandedCallArg],
    fn_name: &str,
    arg_name: &str,
    pos_index: usize,
    kw_allowed: bool,
) -> Result<Option<&'a Value>> {
    let pos_match = positional.get(pos_index).map(|a| &a.value);
    let kw_match = if kw_allowed {
        args.iter()
            .find(|a| a.name.as_deref() == Some(arg_name))
            .map(|a| &a.value)
    } else {
        None
    };
    match (pos_match, kw_match) {
        (Some(_), Some(_)) => Err(type_error(format!(
            "{fn_name}() got multiple values for argument '{arg_name}'"
        ))),
        (Some(v), None) => Ok(Some(v)),
        (None, Some(v)) => Ok(Some(v)),
        (None, None) => Ok(None),
    }
}

/// Build the list of positional args (in source order), checking that every
/// keyword argument is one we recognise.
pub(crate) fn validate_kwargs_and_collect_positional<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
    allowed_kwargs: &[&str],
) -> Result<Vec<&'a ExpandedCallArg>> {
    let mut positional = Vec::with_capacity(args.len());
    for arg in args {
        match &arg.name {
            None => positional.push(arg),
            Some(name) => {
                if !allowed_kwargs.iter().any(|k| *k == name.as_str()) {
                    return Err(type_error(format!(
                        "{fn_name}() got an unexpected keyword argument '{name}'"
                    )));
                }
            }
        }
    }
    Ok(positional)
}

/// Bound on positional argument count.  Used by macro-generated preludes to
/// emit `too many` / `missing` errors with CPython-style wording.
pub(crate) fn check_positional_count(
    fn_name: &str,
    positional_len: usize,
    min: usize,
    max: usize,
) -> Result<()> {
    if positional_len > max {
        return Err(type_error(format!(
            "{fn_name}() takes from {min} to {max} positional arguments but {positional_len} were given"
        )));
    }
    Ok(())
}

/// Construct the "missing required argument" error.  The macro emits a call
/// to this from the per-arg extraction when no value is found and no default
/// is supplied.
pub(crate) fn missing_arg<T>(fn_name: &str, arg_name: &str) -> Result<T> {
    Err(type_error(format!(
        "{fn_name}() missing required argument: '{arg_name}'"
    )))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    //! Pin the strict-1:1 contract for every wrapper.  These tests double as
    //! the executable spec the `pyrust_module!` overload dispatcher relies on:
    //! if `PyFloat::matches` ever returns `true` for an `int` value, every
    //! `(PyInt, PyInt)` / `(PyFloat, PyFloat)` overload pair across the code
    //! base would silently shift behaviour, so the matches predicates get
    //! tighter coverage than `try_from_value` alone.

    use super::*;

    // Helper — extract the `Named` exception class out of a PyError.
    fn err_class(e: &PyError) -> &str {
        match e {
            PyError::Named(cls, _) => cls.as_ref(),
            _ => "<not-named>",
        }
    }

    fn err_msg(e: &PyError) -> &str {
        match e {
            PyError::Named(_, msg) => msg.as_str(),
            _ => "",
        }
    }

    // ── PyInt — strict 1:1 with `int`; rejects bool, float, str, …
    #[test]
    fn pyint_accepts_int_only() {
        let v = Value::int(42);
        let r = PyInt::try_from_value(&v, "f", "x").expect("int accepted");
        assert_eq!(r.0, 42);
        assert!(PyInt::matches(&v));
    }

    #[test]
    fn pyint_rejects_bool() {
        let v = Value::bool_(true);
        assert!(PyInt::try_from_value(&v, "f", "x").is_err());
        assert!(!PyInt::matches(&v));
    }

    #[test]
    fn pyint_rejects_float() {
        let v = Value::float(1.0);
        let err = PyInt::try_from_value(&v, "f", "x").unwrap_err();
        assert_eq!(err_class(&err), "TypeError");
        assert!(err_msg(&err).contains("'x' must be int"));
        assert!(!PyInt::matches(&v));
    }

    // ── PyFloat — strict 1:1 with `float`; rejects int, bool, …
    #[test]
    fn pyfloat_accepts_float_only() {
        let v = Value::float(3.14);
        let r = PyFloat::try_from_value(&v, "f", "x").expect("float accepted");
        assert_eq!(r.0, 3.14);
        assert!(PyFloat::matches(&v));
    }

    #[test]
    fn pyfloat_rejects_int() {
        let v = Value::int(1);
        assert!(PyFloat::try_from_value(&v, "f", "x").is_err());
        assert!(!PyFloat::matches(&v));
    }

    #[test]
    fn pyfloat_rejects_bool() {
        let v = Value::bool_(false);
        assert!(PyFloat::try_from_value(&v, "f", "x").is_err());
        assert!(!PyFloat::matches(&v));
    }

    // ── PyStr — only `str`; no __str__ coercion.
    #[test]
    fn pystr_accepts_str_only() {
        let v = Value::string("hi");
        let r = PyStr::try_from_value(&v, "f", "x").unwrap();
        assert_eq!(r.0, "hi");
        assert_eq!(&*r, "hi"); // Deref<Target = str>
        assert!(PyStr::matches(&v));
    }

    #[test]
    fn pystr_rejects_int() {
        let v = Value::int(7);
        assert!(PyStr::try_from_value(&v, "f", "x").is_err());
        assert!(!PyStr::matches(&v));
    }

    // ── PyBool / PyBytes / PyList / PyTuple / PyDict / PySet — strict.
    #[test]
    fn pybool_strict() {
        assert!(PyBool::matches(&Value::bool_(true)));
        assert!(!PyBool::matches(&Value::int(1)));
        assert!(!PyBool::matches(&Value::float(0.0)));
    }

    #[test]
    fn pylist_strict() {
        let v = Value::list(vec![Value::int(1), Value::int(2)]);
        assert!(PyList::matches(&v));
        assert!(!PyList::matches(&Value::tuple(vec![])));
        let l = PyList::try_from_value(&v, "f", "x").unwrap();
        assert_eq!(l.as_slice().len(), 2);
    }

    #[test]
    fn pytuple_strict() {
        let v = Value::tuple(vec![Value::int(1)]);
        assert!(PyTuple::matches(&v));
        assert!(!PyTuple::matches(&Value::list(vec![])));
    }

    #[test]
    fn pydict_strict() {
        let v = Value::dict(indexmap::IndexMap::new());
        assert!(PyDict::matches(&v));
        assert!(!PyDict::matches(&Value::list(vec![])));
    }

    #[test]
    fn pyset_strict() {
        let v = Value::set(indexmap::IndexSet::new());
        assert!(PySet::matches(&v));
        assert!(!PySet::matches(&Value::list(vec![])));
    }

    // ── PyValue — always matches.
    #[test]
    fn pyvalue_matches_anything() {
        for v in [
            Value::int(0),
            Value::float(0.0),
            Value::string("s"),
            Value::list(vec![]),
            Value::none(),
        ] {
            assert!(PyValue::matches(&v));
        }
    }

    // ── Option<T> — accepts None or T.
    #[test]
    fn option_t_accepts_none() {
        let v = Value::none();
        let r = <Option<PyInt>>::try_from_value(&v, "f", "x").unwrap();
        assert!(r.is_none());
        assert!(<Option<PyInt>>::matches(&v));
    }

    #[test]
    fn option_t_accepts_t() {
        let v = Value::int(5);
        let r = <Option<PyInt>>::try_from_value(&v, "f", "x").unwrap();
        assert_eq!(r.unwrap().0, 5);
        assert!(<Option<PyInt>>::matches(&v));
    }

    #[test]
    fn option_t_rejects_wrong_inner_type() {
        let v = Value::float(5.0);
        assert!(<Option<PyInt>>::try_from_value(&v, "f", "x").is_err());
        assert!(!<Option<PyInt>>::matches(&v));
    }

    // ── Error messages — CPython parity.
    #[test]
    fn typeerror_must_be_message_format() {
        let v = Value::int(5);
        let err = PyStr::try_from_value(&v, "open", "path").unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("open()") && msg.contains("'path'") && msg.contains("must be str"),
            "unexpected error message: {msg:?}"
        );
    }

    #[test]
    fn missing_arg_message_format() {
        let err = missing_arg::<()>("open", "path").unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("open()") && msg.contains("missing required argument: 'path'"),
            "unexpected error message: {msg:?}"
        );
    }

    #[test]
    fn check_positional_count_rejects_too_many() {
        let err = check_positional_count("open", 3, 1, 2).unwrap_err();
        assert!(
            err_msg(&err).contains("from 1 to 2 positional arguments but 3"),
            "unexpected error message: {:?}",
            err_msg(&err)
        );
        // Within bounds — no error.
        assert!(check_positional_count("open", 2, 1, 2).is_ok());
        assert!(check_positional_count("open", 1, 1, 2).is_ok());
    }

    #[test]
    fn unknown_kwarg_rejected() {
        let args = vec![ExpandedCallArg {
            name: Some("bogus".to_string()),
            value: Value::int(1),
        }];
        let err =
            validate_kwargs_and_collect_positional(&args, "open", &["path", "mode"]).unwrap_err();
        assert!(
            err_msg(&err).contains("unexpected keyword argument 'bogus'"),
            "unexpected error message: {:?}",
            err_msg(&err)
        );
    }

    #[test]
    fn allowed_kwarg_keeps_only_positional() {
        let args = vec![
            ExpandedCallArg {
                name: None,
                value: Value::string("/tmp/x"),
            },
            ExpandedCallArg {
                name: Some("mode".to_string()),
                value: Value::string("w"),
            },
        ];
        let positional =
            validate_kwargs_and_collect_positional(&args, "open", &["path", "mode"]).unwrap();
        assert_eq!(positional.len(), 1);
        assert_eq!(positional[0].value.as_str(), Some("/tmp/x"));
    }

    #[test]
    fn locate_arg_positional_then_keyword() {
        let args = vec![ExpandedCallArg {
            name: Some("mode".to_string()),
            value: Value::string("w"),
        }];
        let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
        // path is absent; should return None
        let p = locate_arg(&args, &positional, "open", "path", 0, true).unwrap();
        assert!(p.is_none());
        // mode is kw-only; should resolve via the keyword path
        let m = locate_arg(&args, &positional, "open", "mode", 1, true).unwrap();
        assert_eq!(m.and_then(|v| v.as_str()), Some("w"));
    }

    #[test]
    fn locate_arg_rejects_duplicate_pos_and_kw() {
        let args = vec![
            ExpandedCallArg {
                name: None,
                value: Value::string("/tmp/x"),
            },
            ExpandedCallArg {
                name: Some("path".to_string()),
                value: Value::string("/tmp/y"),
            },
        ];
        let positional: Vec<&ExpandedCallArg> = args.iter().filter(|a| a.name.is_none()).collect();
        let err = locate_arg(&args, &positional, "open", "path", 0, true).unwrap_err();
        assert!(
            err_msg(&err).contains("multiple values for argument 'path'"),
            "unexpected error message: {:?}",
            err_msg(&err)
        );
    }
}
