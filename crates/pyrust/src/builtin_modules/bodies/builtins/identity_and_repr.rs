use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: ascii(object) — ASCII-only escaped repr.
    /// <https://docs.python.org/3/library/functions.html#ascii>
    ///
    /// Migrated to the typed-signature dialect (#400): like `repr`,
    /// `ascii` accepts every Python object, so `PyValue` is the natural
    /// wrapper.  It can run user code by dispatching `__repr__`
    /// for `PyInstance` values, which may invoke arbitrary user code.
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `ascii() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn ascii(#[positional_only] obj: PyValue) -> Result<Value> {
        Ok(Value::string(ascii_repr_interp(_interp, &obj.0)?))
    }

    /// CPython: id(object) — identity (CPython returns memory address).
    /// <https://docs.python.org/3/library/functions.html#id>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` is the
    /// catch-all wrapper since `id` accepts every Python object.
    ///
    /// The identity itself is [`Value::object_id`] — the single definition
    /// `is` is built on.  This body used to re-derive one per kind and fell
    /// back to `0` for everything it had not enumerated, which handed every
    /// float and complex the same id (#2956); deriving it in one place is
    /// what keeps `id()` and `is` from disagreeing.  It returns the exact
    /// non-negative Python integer directly, including the wide float and
    /// complex namespaces.
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `id() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn id(#[positional_only] obj: PyValue) -> Result<Value> {
        Ok(obj.0.object_id())
    }

    /// CPython: repr(object) — printable representation string.
    /// <https://docs.python.org/3/library/functions.html#repr>
    ///
    /// Migrated to the typed-signature dialect (#400): the macro-emitted
    /// prelude validates positional count, rejects unknown kwargs, and
    /// binds `obj` as a typed local.  `PyValue` is the catch-all wrapper
    /// — `repr` accepts every Python object, so type-checking the input
    /// is exactly the prelude's "validate arity / reject kwargs" job.
    ///
    /// This can run user code by dispatching `__repr__` for
    /// `PyInstance` values (and transitively for instances inside containers),
    /// which may invoke arbitrary user code.
    #[arity_style(takes_exactly_one)]
    fn repr(#[positional_only] obj: PyValue) -> Result<Value> {
        // Fast path (#alloc): `repr(int)` == the digits, formatted straight into
        // the string Value (one allocation, no intermediate heap `String`).
        if let ValueKind::Int(n) = obj.0.kind() {
            return Ok(Value::int_string(n));
        }
        pyrust_core::check_int_str_conversion(&obj.0)?;
        let s = render_value_repr(_interp, &obj.0)?;
        Ok(Value::string(s))
    }

    /// CPython: hash(object) — hash value if hashable.
    /// <https://docs.python.org/3/library/functions.html#hash>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue`
    /// accepts every input; the body's per-kind match preserves the
    /// existing CPython-compatible numeric hashing (int / bool / float
    /// with `1.0 == 1` parity), FNV-1a-style string hashing, and the
    /// per-kind "unhashable type: 'X'" errors for list / dict / set.
    ///
    /// This can run user code by dispatching `__hash__` for
    /// `PyInstance` values, which may invoke arbitrary user code.
    #[arity_style(takes_exactly_one)]
    fn hash(#[positional_only] obj: PyValue) -> Result<Value> {
        let value = obj.0;
        let hash_val = hash_value_with_interp(_interp, &value)?;
        Ok(Value::int(hash_val))
    }
}
