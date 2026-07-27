// ─── PyInt ────────────────────────────────────────────────────────────────────

/// `int` argument.  Strict 1:1 with the Python type, accepting **both**
/// inline `Int(i64)` and heap-allocated `BigInt` — Python's int is
/// unbounded, so the wrapper must mirror that.  `bool` still does not
/// auto-coerce (declare a `PyBool` overload, or a `PyValue` fallback,
/// to handle it).
///
/// # Accessing the value
///
/// - [`PyInt::as_i64`] returns `Some(i64)` when the value fits in i64
///   (the common case for everyday integers, including `BigInt`s whose
///   actual magnitude fits) and `None` for genuine bignums.
/// - [`PyInt::expect_i64`] wraps `as_i64` and raises `OverflowError`
///   with a CPython-style message — use it when the builtin's
///   semantics require an i64.
/// - [`PyInt::to_bigint`] always succeeds: returns an owned `BigInt`,
///   cloning the heap allocation if the source was already heap-stored.
/// - [`PyInt::is_big`] reports whether the source needed heap storage
///   — builtins can branch on this to keep their fast path in i64.
#[derive(Debug, Clone)]
pub(crate) struct PyInt<'a>(PyIntRepr<'a>);

/// Backing representation for [`PyInt`].  The `Small` variant holds the
/// inline i64 directly (no allocation); `Big` borrows the heap-stored
/// `BigInt` from the `Value`'s `Opaque::PyBigInt`.
///
/// Kept `pub(super)` so the macro-emitted prelude doesn't have to know
/// the variant names, but tests in this file can pattern-match on it.
#[derive(Debug, Clone)]
enum PyIntRepr<'a> {
    Small(i64),
    Big(&'a PyBigInt),
}

impl<'a> PyInt<'a> {
    /// Extract as `i64`, returning `None` if the value doesn't fit.
    /// Note that a `Big` representation whose magnitude happens to fit
    /// still returns `Some` — the variant is about storage, not range.
    pub fn as_i64(&self) -> Option<i64> {
        match &self.0 {
            PyIntRepr::Small(n) => Some(*n),
            PyIntRepr::Big(b) => b.to_i64(),
        }
    }

    /// Like [`as_i64`], but converts overflow into a CPython-style
    /// `OverflowError` instead of `None`.  Used by builtin bodies that
    /// can't process bignums.  Note: `chr()` uses `as_i64()` directly
    /// so it can supply the CPython-exact message (#1584); this helper
    /// remains available for other builtins with different wording
    /// requirements.
    #[allow(dead_code)]
    pub fn expect_i64(&self, fn_name: &str, arg_name: &str) -> Result<i64> {
        self.as_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                format!("{fn_name}() argument '{arg_name}' too large to fit in i64",),
            )
        })
    }

    /// Convert to an owned [`PyBigInt`].  Always succeeds.  `Small`
    /// allocates a fresh `BigInt`; `Big` clones the shared heap form.
    /// Use when the builtin's arithmetic needs arbitrary precision.
    pub fn to_bigint(&self) -> PyBigInt {
        match &self.0 {
            PyIntRepr::Small(n) => PyBigInt::from(*n),
            PyIntRepr::Big(b) => (*b).clone(),
        }
    }

    /// True if the value is `Big` (heap-stored).  Useful for builtins
    /// that fast-path the small case (e.g. choose between i64 and
    /// BigInt arithmetic without an extra conversion).  Exercised only
    /// by unit tests today; production builtins haven't yet split a
    /// small/big path through `PyInt` (issue #400 migration WIP).
    #[allow(dead_code)]
    pub fn is_big(&self) -> bool {
        matches!(self.0, PyIntRepr::Big(_))
    }
}

/// Ergonomic default-value construction: `#[default(0)] x: PyInt`.
impl From<i64> for PyInt<'static> {
    fn from(n: i64) -> Self {
        PyInt(PyIntRepr::Small(n))
    }
}

impl<'a> FromValue<'a> for PyInt<'a> {
    const PY_TYPE_NAME: &'static str = "int";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.kind() {
            ValueKind::Int(n) => Ok(PyInt(PyIntRepr::Small(n))),
            ValueKind::BigInt(b) => Ok(PyInt(PyIntRepr::Big(b))),
            _ => Err(must_be_error(fn_name, arg_name, "int", value)),
        }
    }

    fn matches(value: &'a Value) -> bool {
        matches!(value.kind(), ValueKind::Int(_) | ValueKind::BigInt(_))
    }
}

// ─── PyFloat ──────────────────────────────────────────────────────────────────

/// `float` argument.  Accepts `float` only — strict 1:1 with the Python type.
/// `int` and `bool` do not auto-coerce; declare additional overloads for
/// those combinations, or use a `PyValue` fallback for mixed-type handling.
#[derive(Debug, Clone)]
pub(crate) struct PyFloat(pub f64);

impl Deref for PyFloat {
    type Target = f64;
    fn deref(&self) -> &f64 {
        &self.0
    }
}

/// Ergonomic default-value construction: `#[default(0.0)] x: PyFloat`.
impl From<f64> for PyFloat {
    fn from(f: f64) -> Self {
        PyFloat(f)
    }
}

impl<'a> FromValue<'a> for PyFloat {
    const PY_TYPE_NAME: &'static str = "float";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.kind() {
            ValueKind::Float(f) => Ok(PyFloat(f)),
            _ => Err(must_be_error(fn_name, arg_name, "float", value)),
        }
    }

    fn matches(value: &'a Value) -> bool {
        matches!(value.kind(), ValueKind::Float(_))
    }
}

// ─── PyStr ────────────────────────────────────────────────────────────────────

/// `str` argument.  Accepts `str` only — no auto-coercion (matches CPython
/// for APIs like `open(path, mode)` that require an actual string).
///
/// Stores a `Cow<'a, str>` so:
/// - **`try_from_value`** populates `Cow::Borrowed(&'a str)` directly from
///   the `Value`'s backing buffer (zero-copy through the call boundary).
/// - **`#[default(...)]`** literals like `"r".into()` produce
///   `Cow::Borrowed("r")` at `'static` (also zero-copy).
/// - Owned strings work via `Cow::Owned(String)` for the rare case where the
///   body needs to construct a fresh `String` as a default.
///
/// Derefs to `&str`, so call sites pass it to `&str`-taking APIs unchanged.
#[derive(Debug, Clone)]
pub(crate) struct PyStr<'a>(pub Cow<'a, str>);

impl<'a> Deref for PyStr<'a> {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl<'a> AsRef<str> for PyStr<'a> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Ergonomic default-value construction: `#[default("r".into())] mode: PyStr`.
impl From<&'static str> for PyStr<'static> {
    fn from(s: &'static str) -> Self {
        PyStr(Cow::Borrowed(s))
    }
}

impl From<String> for PyStr<'static> {
    fn from(s: String) -> Self {
        PyStr(Cow::Owned(s))
    }
}

impl<'a> FromValue<'a> for PyStr<'a> {
    const PY_TYPE_NAME: &'static str = "str";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.as_str() {
            Some(s) => Ok(PyStr(Cow::Borrowed(s))),
            None => Err(must_be_error(fn_name, arg_name, "str", value)),
        }
    }

    fn matches(value: &'a Value) -> bool {
        value.is_str()
    }
}

// ─── PyBool ───────────────────────────────────────────────────────────────────

/// `bool` argument.  Accepts `bool` only.  (CPython is lenient here, accepting
/// any truthy value, but typed APIs that want strict bool are common enough
/// to justify a separate wrapper from `PyValue`.)
#[derive(Debug, Clone)]
pub(crate) struct PyBool(pub bool);

impl Deref for PyBool {
    type Target = bool;
    fn deref(&self) -> &bool {
        &self.0
    }
}

/// Ergonomic default-value construction: `#[default(false)] flag: PyBool`.
impl From<bool> for PyBool {
    fn from(b: bool) -> Self {
        PyBool(b)
    }
}

impl<'a> FromValue<'a> for PyBool {
    const PY_TYPE_NAME: &'static str = "bool";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.kind() {
            ValueKind::Bool(b) => Ok(PyBool(b)),
            _ => Err(must_be_error(fn_name, arg_name, "bool", value)),
        }
    }

    fn matches(value: &'a Value) -> bool {
        value.is_bool()
    }
}

// ─── PyBytes ──────────────────────────────────────────────────────────────────

/// `bytes` argument.  Stored as an Rc-shared `Vec<u8>` (matching the underlying
/// `Opaque::Bytes` representation), so cloning is cheap.
#[derive(Debug, Clone)]
pub(crate) struct PyBytes(pub Rc<Vec<u8>>);

impl Deref for PyBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl<'a> FromValue<'a> for PyBytes {
    const PY_TYPE_NAME: &'static str = "bytes";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        match value.kind() {
            ValueKind::Bytes(rc) => Ok(PyBytes(Rc::clone(rc))),
            _ => Err(must_be_error(fn_name, arg_name, "bytes", value)),
        }
    }

    fn matches(value: &'a Value) -> bool {
        matches!(value.kind(), ValueKind::Bytes(_))
    }
}
