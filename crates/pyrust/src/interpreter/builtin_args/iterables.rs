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
                    || crate::interpreter::builtin_data_backing(value).is_some()
            }
            _ => false,
        }
    }
}
