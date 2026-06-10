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
//!
//! # `PyIterable` — "anything iterable" argument
//!
//! [`PyIterable`] is the wrapper for builtins whose canonical signature
//! is "anything iterable" — `list()`, `tuple()`, `set()`, `dict()`,
//! `sum()`, `min()`, `max()`, `any()`, `all()`, `sorted()`,
//! `reversed()`, `map()`, `filter()`, `zip()`, `enumerate()`, `iter()`,
//! `next()`, etc.  It materialises the source into a `Vec<Value>` at
//! `try_from_value` time (eager, matching the existing
//! `pyrust_builtins::iter_helpers` shape).  A future lazy `PyIter<'a>`
//! variant can be added if profiles show the materialisation cost
//! matters.
//!
//! Materialisation routes through
//! [`pyrust_core::iter_values_via_registry`], which the interpreter
//! installs at startup ([`Interpreter::default`] in
//! `crates/pyrust/src/interpreter.rs`).  That removes the need for an
//! interpreter handle on the [`FromValue`] trait — the wrapper drains
//! lists, tuples, dicts, sets, strings, bytes, ranges, generators,
//! iterable `BuiltinObject`s, and user-class `PyInstance`s with
//! `__iter__` via the same path the rest of the interpreter uses.
//!
//! Per-builtin migrations off the legacy `(args)` dialect are tracked
//! under #400; landing the wrapper alone (this module) is #398.

use std::borrow::Cow;
use std::ops::Deref;
use std::rc::Rc;

use smallvec::SmallVec;

use crate::error::{PyError, Result};
use crate::value::{PyBigInt, PyToPrimitive, Value, ValueKind};

use super::ExpandedCallArg;

/// Inline storage for the positional-args list a typed builtin's
/// dispatcher prelude collects from `validate_kwargs_and_collect_positional`.
/// All migrated builtins so far have ≤ 4 parameters, so the `Vec` path
/// is heap-free for them; longer signatures still work via the
/// `SmallVec` overflow spill.  Sized at 4 to match the
/// `Interpreter::call_arg_buf` budget used elsewhere in this crate.
///
/// Per-call hot-path benchmarks (see PR following #403) showed the
/// previous `Vec::with_capacity(args.len())` was the dominant per-call
/// cost (~8 ns/call), shading every Tier 1 migration.  Replacing with
/// `SmallVec` eliminates the alloc for the common case.
pub(crate) type PositionalArgs<'a> = SmallVec<[&'a ExpandedCallArg; 4]>;

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

// ─── PyList / PyTuple / PyDict / PySet ────────────────────────────────────────
//
// These wrap the Value itself so the body can borrow the underlying slice /
// map / set with one method call.  No copy at construction time.
//
// `#[allow(dead_code)]` on the struct + the `as_slice` / `as_map` / `as_set`
// methods: none of the migrated `pyrust_module!` builtins consume these
// shapes yet (the typed-signature dialect — issue #400 — has so far only
// flipped over `PyInt` / `PyBool` / `PyStr` / `PyFloat` / `PyBytes` /
// `PyValue` call sites).  The wrappers stay as ready-to-use infrastructure
// for the remaining #400 migrations; deleting them now would just force
// the next migration to redefine the exact same shapes.

/// `list` argument.  Use [`PyList::as_slice`] to read elements.
#[allow(dead_code)] // #400 typed-signature dialect stub
#[derive(Debug, Clone)]
pub(crate) struct PyList(pub Value);

#[allow(dead_code)] // #400 stub
impl PyList {
    pub(crate) fn as_slice(&self) -> &[Value] {
        // SAFETY (no unsafe used): `try_from_value` verified is_list(),
        // and `Value::clone` preserves that.  `as_list()` is then infallible.
        self.0.as_list().expect("PyList wraps a list")
    }
}

impl<'a> FromValue<'a> for PyList {
    const PY_TYPE_NAME: &'static str = "list";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyList(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "list", value))
        }
    }

    fn matches(value: &'a Value) -> bool {
        value.is_list()
    }
}

/// `tuple` argument.  Use [`PyTuple::as_slice`].
#[allow(dead_code)] // #400 typed-signature dialect stub
#[derive(Debug, Clone)]
pub(crate) struct PyTuple(pub Value);

#[allow(dead_code)] // #400 stub
impl PyTuple {
    pub(crate) fn as_slice(&self) -> &[Value] {
        self.0.as_tuple().expect("PyTuple wraps a tuple")
    }
}

impl<'a> FromValue<'a> for PyTuple {
    const PY_TYPE_NAME: &'static str = "tuple";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyTuple(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "tuple", value))
        }
    }

    fn matches(value: &'a Value) -> bool {
        value.is_tuple()
    }
}

/// `dict` argument.  Use [`PyDict::as_map`].
#[allow(dead_code)] // #400 typed-signature dialect stub
#[derive(Debug, Clone)]
pub(crate) struct PyDict(pub Value);

#[allow(dead_code)] // #400 stub
impl PyDict {
    pub(crate) fn as_map(&self) -> &pyrust_core::PyDict {
        self.0.as_dict().expect("PyDict wraps a dict")
    }
}

impl<'a> FromValue<'a> for PyDict {
    const PY_TYPE_NAME: &'static str = "dict";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PyDict(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "dict", value))
        }
    }

    fn matches(value: &'a Value) -> bool {
        matches!(value.kind(), ValueKind::Dict(_))
    }
}

/// `set` argument.  Use [`PySet::as_set`].
#[allow(dead_code)] // #400 typed-signature dialect stub
#[derive(Debug, Clone)]
pub(crate) struct PySet(pub Value);

#[allow(dead_code)] // #400 stub
impl PySet {
    /// Returns the underlying `IndexSet`.  Panics if the wrapper somehow
    /// doesn't wrap a `Set` — impossible by construction (`try_from_value`
    /// checks `is_set` and `Value::clone` preserves the kind), but the
    /// `expect` style matches the sibling wrappers' panic-message wording.
    /// Run `f` against the underlying `IndexSet` view.  Returns the
    /// closure's result.  Post-#450 the `IndexSet` is reached via a
    /// scoped `Ref` borrow from `ValueKind::Set`, so the API now
    /// passes a `&PySet` into the closure rather than
    /// handing one back (which the borrow lifetimes can't express).
    pub(crate) fn as_set<R>(&self, f: impl FnOnce(&pyrust_core::PySet) -> R) -> R {
        match self.0.kind() {
            ValueKind::Set(s) => f(&s),
            _ => panic!("PySet wraps a set"),
        }
    }
}

impl<'a> FromValue<'a> for PySet {
    const PY_TYPE_NAME: &'static str = "set";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if Self::matches(value) {
            Ok(PySet(value.clone()))
        } else {
            Err(must_be_error(fn_name, arg_name, "set", value))
        }
    }

    fn matches(value: &'a Value) -> bool {
        matches!(value.kind(), ValueKind::Set(_))
    }
}

// ─── PyValue (pass-through) ───────────────────────────────────────────────────

/// `Any` — accepts any value, no type checking.  Use when the builtin handles
/// its own polymorphism (e.g. `repr(obj)`, `id(obj)`).
#[derive(Debug, Clone)]
pub(crate) struct PyValue(pub Value);

impl Deref for PyValue {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.0
    }
}

impl<'a> FromValue<'a> for PyValue {
    const PY_TYPE_NAME: &'static str = "object";

    fn try_from_value(value: &'a Value, _fn_name: &str, _arg_name: &str) -> Result<Self> {
        Ok(PyValue(value.clone()))
    }

    fn matches(_value: &'a Value) -> bool {
        true
    }
}

// ─── PyIterable ───────────────────────────────────────────────────────────────

/// `iterable` argument — materialises any iterable source into
/// `Vec<Value>` on construction.  Eager (matches the existing
/// `pyrust_builtins::iter_helpers` shape); a lazy single-pass `PyIter<'a>`
/// can be added later if profiles show the materialisation cost matters.
///
/// # Sources accepted
///
/// Anything the interpreter's `iter_values` already handles:
///
/// - Built-in iterables: `list`, `tuple`, `dict` (yields keys, matching
///   CPython), `set`, `str` (yields 1-character strings), `bytes`
///   (yields `int` codepoints), `range`.
/// - User classes whose `__iter__` is callable.
/// - Iterable `BuiltinObject`s (e.g. frozenset, dict views), via the
///   type's `BuiltinTypeOps::is_iterable` predicate.
/// - Generators (drained — iterating a generator consumes it; this is
///   intentional, matching the eager materialisation contract).
///
/// # Errors
///
/// `try_from_value` returns `TypeError: <fn>() argument '<name>' must be
/// iterable, not <type>` when the source isn't iterable.  Errors raised
/// *during* iteration (e.g. a user-defined `__next__` that raises)
/// propagate through `iter_values_via_registry`.
///
/// # Registry dependency
///
/// Materialisation goes through [`pyrust_core::iter_values_via_registry`],
/// which the interpreter installs in [`Interpreter::default`]
/// (`crates/pyrust/src/interpreter.rs`) before any builtin can be called.
/// In standalone tests that exercise `PyIterable::try_from_value` without
/// first constructing an `Interpreter`, install the callback manually —
/// the `mod tests` block below does this once via [`std::sync::Once`].
#[derive(Debug, Clone)]
#[allow(dead_code)] // #400 typed-signature dialect stub
pub(crate) struct PyIterable<'a> {
    items: Vec<Value>,
    _phantom: std::marker::PhantomData<&'a Value>,
}

#[allow(dead_code)] // #400 stub
impl<'a> PyIterable<'a> {
    /// Read-only view of the materialised items.
    pub fn as_slice(&self) -> &[Value] {
        &self.items
    }

    /// Take ownership of the materialised items.  Use when the builtin
    /// builds its result (e.g. `list(iterable)`) directly from them.
    pub fn into_items(self) -> Vec<Value> {
        self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl<'a> FromValue<'a> for PyIterable<'a> {
    const PY_TYPE_NAME: &'static str = "iterable";

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if !Self::matches(value) {
            return Err(must_be_error(fn_name, arg_name, "iterable", value));
        }
        // Drain through the interpreter-installed callback.  `matches`
        // already filtered out the obviously-non-iterable kinds; what
        // remains is either iterable, or a user class whose `__iter__`
        // turns out to be non-callable (which surfaces from the
        // registry call as a structured error — CPython parity).
        let items = pyrust_core::iter_values_via_registry(value)?;
        Ok(PyIterable {
            items,
            _phantom: std::marker::PhantomData,
        })
    }

    /// Structural type-match — allocation-free.  Cannot materialise to
    /// check (the overload dispatcher requires `matches` not to
    /// allocate), so the predicate inspects `ValueKind` against the set
    /// of known iterable kinds and, for `PyInstance`, probes the class
    /// for `__iter__` without calling it.
    ///
    /// A user class whose `__iter__` is structurally present but not
    /// callable will pass this predicate; the actual call later in
    /// `try_from_value` may then fail.  That mirrors CPython, where the
    /// caller learns at iteration time rather than at dispatch.
    fn matches(value: &'a Value) -> bool {
        match value.kind() {
            ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::Str(_)
            | ValueKind::Bytes(_)
            | ValueKind::Range { .. }
            | ValueKind::Generator(_) => true,
            ValueKind::BuiltinObject { ops, .. } => ops.is_iterable(),
            ValueKind::PyInstance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                crate::interpreter::lookup_class_attr(&class, "__iter__").is_some()
                    || crate::interpreter::instance_builtin_data(inst).is_some()
            }
            _ => false,
        }
    }
}

// ─── Option<T> — for default-None args ────────────────────────────────────────
//
// An `Option<T>` argument accepts either:
//   - `Value::none()` → `None`
//   - a `T` (via `T::try_from_value`) → `Some(T)`
// Use together with `#[default(None)]` in the macro signature for clean
// "may be absent" semantics.

impl<'a, T: FromValue<'a>> FromValue<'a> for Option<T> {
    // Kept for trait coherence; consumers that print a parameter type for
    // an `Option<T>` should compose `T::PY_TYPE_NAME` + " or None" at the
    // call site (as `try_from_value` does below).  The const can't carry
    // that suffix because Rust requires `const &'static str` to be
    // statically-known, and we can't concat at trait-impl time.
    const PY_TYPE_NAME: &'static str = T::PY_TYPE_NAME;

    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        if value.is_none() {
            return Ok(None);
        }
        // Use `matches` as the predicate so we know whether `T`'s own
        // `try_from_value` would succeed before we run it — that lets us
        // surface the correct "must be X or None" wording on the failure
        // path without first generating, then discarding, T's stricter
        // "must be X" message.
        if T::matches(value) {
            return T::try_from_value(value, fn_name, arg_name).map(Some);
        }
        Err(type_error(format!(
            "{fn_name}() argument '{arg_name}' must be {} or None, not {}",
            T::PY_TYPE_NAME,
            pyrust_core::builtin_type_name(value),
        )))
    }

    fn matches(value: &'a Value) -> bool {
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

/// Tightest path through arg validation — for typed signatures whose
/// every parameter is `#[positional_only]` (the all-CPython-builtin
/// shape).  Rejects any keyword argument outright and skips the
/// positional-args collection: when no kwargs are legal, the slice
/// the caller already holds *is* the positional list, indexable
/// directly via `args.get(i)`.
///
/// The macro emits a call to this — plus a direct `args.get(i)` per
/// parameter — instead of the slower
/// `validate_kwargs_and_collect_positional` + `locate_arg` chain when
/// it can prove at compile time that no parameter accepts kwargs.
pub(crate) fn reject_named_args(args: &[ExpandedCallArg], fn_name: &str) -> Result<()> {
    if args.iter().any(|a| a.name.is_some()) {
        return Err(type_error(format!(
            "{fn_name}() takes no keyword arguments"
        )));
    }
    Ok(())
}

/// Build the list of positional args (in source order), checking that every
/// keyword argument is one we recognise.  Returns a `SmallVec` so the
/// common case (≤ 4 args) needs no heap allocation — see
/// [`PositionalArgs`] for the inline-storage rationale.
pub(crate) fn validate_kwargs_and_collect_positional<'a>(
    args: &'a [ExpandedCallArg],
    fn_name: &str,
    allowed_kwargs: &[&str],
) -> Result<PositionalArgs<'a>> {
    let mut positional: PositionalArgs<'a> = SmallVec::new();
    for arg in args {
        match &arg.name {
            None => positional.push(arg),
            Some(name) => {
                if !allowed_kwargs.contains(&name.as_str()) {
                    return Err(type_error(format!(
                        "{fn_name}() got an unexpected keyword argument '{name}'"
                    )));
                }
            }
        }
    }
    Ok(positional)
}

/// Bound on positional argument count — emits the CPython-style "too many"
/// error when violated.  Branches the wording on `min == max` so a builtin
/// with no defaults (`min == max == 1`) says `takes 1 positional argument
/// but 2 were given` rather than the nonsensical `takes from 1 to 1
/// positional arguments`.
///
/// Too-few-positional cases are caught downstream by `missing_arg` per
/// parameter (which knows the name); this function intentionally does not
/// check the lower bound.
pub(crate) fn check_positional_count(
    fn_name: &str,
    positional_len: usize,
    min: usize,
    max: usize,
) -> Result<()> {
    if positional_len > max {
        let msg = if min == max {
            let plural = if max == 1 { "argument" } else { "arguments" };
            format!("{fn_name}() takes {max} positional {plural} but {positional_len} were given",)
        } else {
            format!(
                "{fn_name}() takes from {min} to {max} positional arguments but {positional_len} were given",
            )
        };
        return Err(type_error(msg));
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

// ─── C-level arity wordings (#2331) ────────────────────────────────────────────
//
// CPython's hand-written C builtins do *not* use the argument-clinic
// "takes N positional arguments but M were given" / "missing required
// argument" wordings the default dialect emits; they raise distinct
// C-level messages.  The `#[arity_style(...)]` dialect attribute selects
// one of these so a migrated builtin reproduces CPython byte-for-byte.
// See `pyrust-derive`'s `ArityStyle` and the typed-prelude emit.

/// `takes exactly one argument (N given)` — the METH_O / one-argument
/// C-builtin wording (`len`, `repr`, `hash`, `ord`, `chr`, `abs`,
/// `math.sqrt`, …).  Used for both the too-few and too-many cases (any
/// `positional_len != 1`), so the per-parameter `missing_arg` path is
/// never reached for these functions.
pub(crate) fn check_exactly_one_argument(fn_name: &str, positional_len: usize) -> Result<()> {
    if positional_len != 1 {
        return Err(type_error(format!(
            "{fn_name}() takes exactly one argument ({positional_len} given)"
        )));
    }
    Ok(())
}

/// `NAME expected N arguments, got M` (and the `at least` / `at most`
/// variants) — the METH_VARARGS C-builtin wording used by `isinstance`,
/// `issubclass`, `divmod`, `hasattr`, … .  Note CPython prints the bare
/// function name **without** trailing `()` for this style (unlike every
/// other dialect message), so `fn_name` is interpolated raw.  Handles
/// both the lower and upper bound; the per-parameter `missing_arg` path
/// is unreachable when this guard is used.
pub(crate) fn check_arity_expected_got(
    fn_name: &str,
    positional_len: usize,
    min: usize,
    max: usize,
) -> Result<()> {
    if positional_len < min || positional_len > max {
        let bound = if min == max {
            let plural = if max == 1 { "argument" } else { "arguments" };
            format!("expected {max} {plural}")
        } else if positional_len < min {
            let plural = if min == 1 { "argument" } else { "arguments" };
            format!("expected at least {min} {plural}")
        } else {
            let plural = if max == 1 { "argument" } else { "arguments" };
            format!("expected at most {max} {plural}")
        };
        return Err(type_error(format!(
            "{fn_name} {bound}, got {positional_len}"
        )));
    }
    Ok(())
}

/// Construct the "no overload matched" error.  Emitted by the macro-
/// generated dispatcher of a typed-overload builtin when every declared
/// overload's parameter types failed `FromValue::matches` against the
/// actual call args.  Unreachable in practice when the overload set
/// includes a `PyValue` catch-all (whose `matches` is unconditional);
/// reachable otherwise — the user supplied types not covered by any
/// overload.
///
/// The wording follows CPython's binary-op `unsupported operand type(s)
/// for +: 'int' and 'str'` shape — terse, prints only the actual
/// argument types, omits the declared overload signatures.  Per the
/// design review on #395 (comment 4443208232): "actual types only, no
/// signature dump unless behind a debug flag."
///
/// `actuals` is the type-name list of the *call site*'s args (e.g.
/// `["str", "int"]`).
pub(crate) fn no_overload_matched<T>(
    fn_name: &str,
    actuals: &[std::borrow::Cow<'static, str>],
) -> Result<T> {
    let joined = actuals
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(type_error(format!(
        "{fn_name}(): unsupported argument type(s): ({joined})",
    )))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::approx_constant)] // 3.14 literals are deliberate test data, not π.
mod tests {
    //! Pin the strict-1:1 contract for every wrapper.  These tests double as
    //! the executable spec the `pyrust_module!` overload dispatcher relies on:
    //! if `PyFloat::matches` ever returns `true` for an `int` value, every
    //! `(PyInt, PyInt)` / `(PyFloat, PyFloat)` overload pair across the code
    //! base would silently shift behaviour, so the matches predicates get
    //! tighter coverage than `try_from_value` alone.

    use super::*;
    use crate::value::PyKey;

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

    // ── PyInt — accepts both inline `Int(i64)` and heap-stored `BigInt`,
    //          since Python's int is unbounded.  Rejects bool / float / str.
    #[test]
    fn pyint_accepts_inline_int() {
        let v = Value::int(42);
        let r = PyInt::try_from_value(&v, "f", "x").expect("int accepted");
        assert_eq!(r.as_i64(), Some(42));
        assert!(!r.is_big(), "Value::int produces the Small representation");
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

    #[test]
    fn pyint_bigint_that_fits_collapses_to_small() {
        // `pyrust-core::Value::kind()` automatically downgrades a
        // heap-stored BigInt back to `ValueKind::Int` whenever the
        // magnitude fits in i64.  PyInt sees the post-`kind()` view, so
        // a BigInt wrapping `i64::MAX` arrives as `Small`, not `Big`.
        // This means builtin bodies only encounter the `Big` path for
        // genuine overflow — the common-case fast path stays cheap.
        let v = Value::bigint(PyBigInt::from(i64::MAX));
        let r = PyInt::try_from_value(&v, "f", "x").expect("bigint accepted");
        assert!(
            !r.is_big(),
            "fits-in-i64 BigInt downgrades to Small via kind()"
        );
        assert_eq!(r.as_i64(), Some(i64::MAX));
        assert!(PyInt::matches(&v));
    }

    #[test]
    fn pyint_accepts_bigint_beyond_i64() {
        // The whole point of supporting BigInt: Python's int is
        // unbounded.  `2 ** 100` doesn't fit in i64, but PyInt must
        // accept it.  `as_i64()` returns None; `to_bigint()` recovers
        // the value.
        let huge = PyBigInt::from(1u128 << 100);
        let v = Value::bigint(huge.clone());
        let r = PyInt::try_from_value(&v, "f", "x").expect("bigint accepted");
        assert!(r.is_big());
        assert_eq!(r.as_i64(), None, "out-of-range bigint must not fit i64");
        assert_eq!(r.to_bigint(), huge);
        assert!(PyInt::matches(&v));
    }

    #[test]
    fn pyint_expect_i64_raises_overflow_on_bignum() {
        // The CPython-style `OverflowError` shape for builtins that
        // genuinely need an i64 (chr, range, sleep, …).  Pinned wording.
        let huge = PyBigInt::from(1u128 << 100);
        let v = Value::bigint(huge);
        let r = PyInt::try_from_value(&v, "f", "x").unwrap();
        let err = r.expect_i64("chr", "code_point").unwrap_err();
        assert_eq!(err_class(&err), "OverflowError");
        assert!(
            err_msg(&err).contains("chr()")
                && err_msg(&err).contains("'code_point'")
                && err_msg(&err).contains("too large to fit in i64"),
            "unexpected OverflowError wording: {:?}",
            err_msg(&err),
        );
    }

    #[test]
    fn pyint_to_bigint_works_for_small_repr_too() {
        // Symmetry check: `to_bigint()` upgrades a Small to a fresh
        // BigInt without information loss.  Builtins that mix small
        // and big inputs can normalise to BigInt up front.
        let v = Value::int(-42);
        let r = PyInt::try_from_value(&v, "f", "x").unwrap();
        assert!(!r.is_big());
        assert_eq!(r.to_bigint(), PyBigInt::from(-42i64));
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
        let v = Value::dict(pyrust_core::PyDict::default());
        assert!(PyDict::matches(&v));
        assert!(!PyDict::matches(&Value::list(vec![])));
    }

    #[test]
    fn pyset_strict() {
        let v = Value::set(pyrust_core::PySet::default());
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

    // ── PyIterable — anything iterable; structurally allocation-free `matches`,
    //                 materialising `try_from_value` via the iter callback.
    //
    // `iter_values_via_registry` reads a `OnceLock<IterValuesFn>` that the
    // interpreter installs in `Interpreter::default()`.  The unit tests below
    // run without an interpreter, so the helper here installs the same
    // callback once per test run — `OnceLock::set` ignores subsequent calls,
    // so this is harmless under cargo's parallel test scheduling.
    fn ensure_iter_registry_installed() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            pyrust_core::install_iter_values(crate::interpreter::iter_values);
        });
    }

    #[test]
    fn pyiterable_matches_builtin_iterables() {
        // Structural `matches` — no materialisation, no registry needed.
        // Each kind in the "known iterable" set must report `true`.
        let cases = [
            Value::list(vec![Value::int(1)]),
            Value::tuple(vec![Value::int(1)]),
            Value::dict(pyrust_core::PyDict::default()),
            Value::set(pyrust_core::PySet::default()),
            Value::string("abc"),
            Value::bytes(vec![1, 2, 3]),
            Value::range(0, 3, 1),
        ];
        for v in &cases {
            assert!(
                PyIterable::matches(v),
                "expected iterable: {:?}",
                pyrust_core::builtin_type_name(v),
            );
        }
    }

    #[test]
    fn pyiterable_rejects_scalars() {
        // `matches` must report `false` for the canonical non-iterable
        // kinds — guards the overload dispatcher from accidentally
        // routing `int`/`float`/`bool`/`None` through an iterable
        // overload.
        for v in [
            Value::int(0),
            Value::float(0.0),
            Value::bool_(true),
            Value::none(),
        ] {
            assert!(
                !PyIterable::matches(&v),
                "scalar should not match iterable: {:?}",
                pyrust_core::builtin_type_name(&v),
            );
        }
    }

    #[test]
    fn pyiterable_materialises_list() {
        ensure_iter_registry_installed();
        let v = Value::list(vec![Value::int(1), Value::int(2), Value::int(3)]);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        assert_eq!(it.len(), 3);
        assert!(!it.is_empty());
        let items = it.as_slice();
        assert_eq!(items[0].as_int(), Some(1));
        assert_eq!(items[2].as_int(), Some(3));
    }

    #[test]
    fn pyiterable_materialises_tuple() {
        ensure_iter_registry_installed();
        let v = Value::tuple(vec![Value::int(7), Value::int(8)]);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].as_int(), Some(8));
    }

    #[test]
    fn pyiterable_materialises_dict_yields_keys() {
        // CPython parity: iterating a dict yields its keys, not its
        // items.  The interpreter's `iter_values` already does this; the
        // wrapper inherits the behaviour.
        ensure_iter_registry_installed();
        let mut map = pyrust_core::PyDict::default();
        map.insert(PyKey::str_from("a"), Value::int(1));
        map.insert(PyKey::str_from("b"), Value::int(2));
        let v = Value::dict(map);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 2);
        // Keys arrive as strings, not (key, value) pairs.
        assert_eq!(items[0].as_str(), Some("a"));
        assert_eq!(items[1].as_str(), Some("b"));
    }

    #[test]
    fn pyiterable_materialises_set() {
        ensure_iter_registry_installed();
        let mut s = pyrust_core::PySet::default();
        s.insert(PyKey::Int(9));
        let v = Value::set(s);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        assert_eq!(it.len(), 1);
        assert_eq!(it.as_slice()[0].as_int(), Some(9));
    }

    #[test]
    fn pyiterable_materialises_str_to_chars() {
        ensure_iter_registry_installed();
        let v = Value::string("hi");
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_str(), Some("h"));
        assert_eq!(items[1].as_str(), Some("i"));
    }

    #[test]
    fn pyiterable_materialises_bytes_to_codepoints() {
        ensure_iter_registry_installed();
        let v = Value::bytes(vec![0x41, 0x42]);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].as_int(), Some(0x41));
        assert_eq!(items[1].as_int(), Some(0x42));
    }

    #[test]
    fn pyiterable_materialises_range() {
        ensure_iter_registry_installed();
        let v = Value::range(0, 3, 1);
        let it = PyIterable::try_from_value(&v, "list", "iterable").unwrap();
        let items = it.into_items();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_int(), Some(0));
        assert_eq!(items[2].as_int(), Some(2));
    }

    #[test]
    fn pyiterable_rejects_int_with_typeerror() {
        ensure_iter_registry_installed();
        let v = Value::int(5);
        let err = PyIterable::try_from_value(&v, "list", "iterable").unwrap_err();
        assert_eq!(err_class(&err), "TypeError");
        let msg = err_msg(&err);
        assert!(
            msg.contains("list()")
                && msg.contains("'iterable'")
                && msg.contains("must be iterable")
                && msg.contains("not int"),
            "unexpected error wording: {msg:?}",
        );
    }

    #[test]
    fn pyiterable_rejects_float() {
        ensure_iter_registry_installed();
        let v = Value::float(3.14);
        let err = PyIterable::try_from_value(&v, "sum", "iterable").unwrap_err();
        assert!(err_msg(&err).contains("not float"));
    }

    #[test]
    fn pyiterable_rejects_bool() {
        ensure_iter_registry_installed();
        let v = Value::bool_(false);
        let err = PyIterable::try_from_value(&v, "any", "iterable").unwrap_err();
        assert!(err_msg(&err).contains("not bool"));
    }

    #[test]
    fn pyiterable_rejects_none() {
        ensure_iter_registry_installed();
        let v = Value::none();
        let err = PyIterable::try_from_value(&v, "iter", "iterable").unwrap_err();
        assert!(err_msg(&err).contains("not NoneType"));
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
        assert_eq!(r.unwrap().as_i64(), Some(5));
        assert!(<Option<PyInt>>::matches(&v));
    }

    #[test]
    fn option_t_rejects_wrong_inner_type() {
        let v = Value::float(5.0);
        assert!(<Option<PyInt>>::try_from_value(&v, "f", "x").is_err());
        assert!(!<Option<PyInt>>::matches(&v));
    }

    #[test]
    fn option_t_error_wording_mentions_or_none() {
        // Regression for the PR-#396 review: `Option<T>` rejection used to
        // route through `T::try_from_value` and print "must be int, not str"
        // — strictly correct for `T = PyInt` but misleading for callers who
        // can also pass `None`.  The override now says "must be int or None".
        let v = Value::string("hi");
        let err = <Option<PyInt>>::try_from_value(&v, "pow", "exp").unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("must be int or None"),
            "expected the wording to mention 'or None'; got: {msg:?}",
        );
        assert!(
            msg.contains("not str"),
            "should include the actual type: {msg:?}"
        );
    }

    // ── From impls — ergonomic `#[default(literal)]` construction.
    #[test]
    fn from_impls_for_default_values() {
        // Each strict wrapper carries a `From<inner>` impl so the macro
        // can accept `#[default(0)]` / `#[default(0.0)]` / `#[default(true)]`
        // / `#[default("r")]` without forcing the author to write the
        // wrapper constructor explicitly.
        let i: PyInt = 42i64.into();
        assert_eq!(i.as_i64(), Some(42));
        let f: PyFloat = 3.14f64.into();
        assert_eq!(f.0, 3.14);
        let b: PyBool = true.into();
        assert!(b.0);
        let s: PyStr<'static> = "r".into();
        assert_eq!(&*s, "r");
        let s2: PyStr<'static> = String::from("w").into();
        assert_eq!(&*s2, "w");
    }

    #[test]
    fn pystr_borrows_zero_copy_from_value() {
        // Regression for the Cow refactor: `try_from_value` must produce
        // `Cow::Borrowed`, not `Cow::Owned`, when extracting from a
        // `Value` — that's the whole point of the lifetime-carrying
        // wrapper.  If a future change reverts to `s.to_string()` the
        // assertion below catches it.
        let v = Value::string("hello");
        let s = PyStr::try_from_value(&v, "f", "x").unwrap();
        assert!(
            matches!(s.0, Cow::Borrowed(_)),
            "PyStr should borrow from the Value, not allocate a fresh String",
        );
        assert_eq!(&*s, "hello");
    }

    #[test]
    fn pystr_default_via_into_is_zero_copy() {
        // `"r".into()` (used by `#[default("r".into())]`) creates a
        // `Cow::Borrowed(&'static str)` — also zero-copy.  Symmetric with
        // the Value-extraction path above.
        let s: PyStr<'static> = "r".into();
        assert!(
            matches!(s.0, Cow::Borrowed(_)),
            "PyStr::from(&'static str) should produce Cow::Borrowed",
        );
    }

    // ── no_overload_matched — error wording for the overload dispatcher.
    //
    // The wording follows CPython's `unsupported operand type(s) for +:
    // 'int' and 'str'` shape — terse, actual types only, no declared-
    // overload-signature dump.  See the design review on #395
    // (comment 4443208232): CPython doesn't list candidate signatures
    // for type-dispatch failures; including them here would be a
    // usability regression for end users.
    #[test]
    fn no_overload_matched_single_arg() {
        let err =
            no_overload_matched::<()>("abs", &[std::borrow::Cow::Borrowed("str")]).unwrap_err();
        let msg = err_msg(&err);
        assert_eq!(err_class(&err), "TypeError");
        assert!(
            msg.contains("abs(): unsupported argument type(s): ('str')"),
            "expected terse 'unsupported argument type(s)' wording: {msg:?}",
        );
        // The pre-revision wording (`"no overload matches"` /
        // `"expected one of"`) is explicitly *not* used anymore.
        assert!(
            !msg.contains("expected one of"),
            "signatures must not appear in the user-facing error: {msg:?}",
        );
        assert!(
            !msg.contains("no overload"),
            "must not say 'no overload': {msg:?}",
        );
    }

    #[test]
    fn no_overload_matched_multi_arg() {
        let err = no_overload_matched::<()>(
            "pow",
            &[
                std::borrow::Cow::Borrowed("int"),
                std::borrow::Cow::Borrowed("str"),
            ],
        )
        .unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("pow(): unsupported argument type(s): ('int', 'str')"),
            "multi-arg actuals should be quoted + parenthesised: {msg:?}",
        );
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
    fn check_positional_count_rejects_too_many_range() {
        // min < max — emit the "from M to N" range wording.
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
    fn check_positional_count_min_eq_max_singular() {
        // Regression for PR-#396 review feedback: a 1-required builtin
        // hit with 2 positional args used to print
        // "takes from 1 to 1 positional arguments but 2 were given" —
        // both nonsensical and divergent from CPython.  Now it prints
        // "takes 1 positional argument but 2 were given" with the
        // singular "argument" because max == 1.
        let err = check_positional_count("len", 2, 1, 1).unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("takes 1 positional argument but 2 were given"),
            "expected singular wording with no from-to range; got: {msg:?}",
        );
        assert!(
            !msg.contains("from 1"),
            "should not contain 'from 1 to 1': {msg:?}"
        );
    }

    #[test]
    fn check_positional_count_min_eq_max_plural() {
        // Plural wording when max > 1.
        let err = check_positional_count("divmod", 3, 2, 2).unwrap_err();
        let msg = err_msg(&err);
        assert!(
            msg.contains("takes 2 positional arguments but 3 were given"),
            "expected plural 'arguments'; got: {msg:?}",
        );
        assert!(
            !msg.contains("from 2 to 2"),
            "should not contain 'from 2 to 2': {msg:?}"
        );
    }

    #[test]
    fn check_exactly_one_argument_wording() {
        // METH_O wording (#2331): any count != 1 → "takes exactly one
        // argument (N given)"; count == 1 is accepted.
        assert!(check_exactly_one_argument("repr", 1).is_ok());
        let too_few = check_exactly_one_argument("repr", 0).unwrap_err();
        assert_eq!(
            err_msg(&too_few),
            "repr() takes exactly one argument (0 given)"
        );
        let too_many = check_exactly_one_argument("repr", 2).unwrap_err();
        assert_eq!(
            err_msg(&too_many),
            "repr() takes exactly one argument (2 given)"
        );
    }

    #[test]
    fn check_arity_expected_got_wording() {
        // METH_VARARGS wording (#2331): bare name, no trailing parens.
        assert!(check_arity_expected_got("isinstance", 2, 2, 2).is_ok());
        let fixed = check_arity_expected_got("isinstance", 1, 2, 2).unwrap_err();
        assert_eq!(err_msg(&fixed), "isinstance expected 2 arguments, got 1");
        let fixed_many = check_arity_expected_got("isinstance", 3, 2, 2).unwrap_err();
        assert_eq!(
            err_msg(&fixed_many),
            "isinstance expected 2 arguments, got 3"
        );
        // Range form: "at least" below min, "at most" above max.
        let at_least = check_arity_expected_got("getattr", 1, 2, 3).unwrap_err();
        assert_eq!(
            err_msg(&at_least),
            "getattr expected at least 2 arguments, got 1"
        );
        let at_most = check_arity_expected_got("getattr", 4, 2, 3).unwrap_err();
        assert_eq!(
            err_msg(&at_most),
            "getattr expected at most 3 arguments, got 4"
        );
        // Singular noun when the bound is 1.
        let one = check_arity_expected_got("f", 0, 1, 1).unwrap_err();
        assert_eq!(err_msg(&one), "f expected 1 argument, got 0");
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
