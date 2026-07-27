// Python's shared `__index__` protocol boundary.

fn seq_index_type_error(label: &str, type_name: &str) -> String {
    if label == "string" {
        format!("string indices must be integers, not '{type_name}'")
    } else {
        format!("{label} indices must be integers or slices, not {type_name}")
    }
}

fn index_value_to_i64(value: &Value, overflow_msg: &str) -> Result<i64> {
    use crate::value::PyToPrimitive;
    match value.kind() {
        ValueKind::Int(number) => Ok(number),
        ValueKind::Bool(value) => Ok(value as i64),
        ValueKind::BigInt(number) => number
            .to_i64()
            .ok_or_else(|| pyrust_core::overflow_err!("{}", overflow_msg)),
        _ => unreachable!("value_to_index guarantees an integer"),
    }
}

fn normalize_index_result(result: Value) -> Result<Value> {
    if matches!(
        result.kind(),
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
    ) {
        return Ok(result);
    }
    // CPython still accepts an integer subclass returned by `__index__`
    // (with a DeprecationWarning). Only substitute an integer backing here:
    // a non-integer subclass must retain its original type in the error.
    if let Some(backing) = coerce_subclass_backing(&result, &[])
        && matches!(
            backing.kind(),
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
        )
    {
        return Ok(backing);
    }
    Err(pyrust_core::type_err!(
        "__index__ returned non-int (type {})",
        value_type_name_str(&result),
    ))
}

impl Interpreter {
    /// Resolve a single start/stop argument for `list.index` / `tuple.index`
    /// through the `__index__` protocol, matching CPython 3.12 semantics.
    ///
    /// Thin wrapper over [`Interpreter::value_to_index`]; on a non-integer
    /// non-`__index__` value it raises `TypeError: slice indices must be
    /// integers or have an __index__ method`.
    pub(super) fn resolve_index_arg(&mut self, val: Value) -> Result<Value> {
        self.value_to_index(&val, |_| {
            pyrust_core::type_err!("slice indices must be integers or have an __index__ method")
        })
    }
    /// **The single source of truth for CPython's index protocol** (`operator.index`
    /// / `PyNumber_Index`).  Resolve `val` to an integer `Value` (guaranteed
    /// `Int`, `Bool`, or `BigInt`), honoring `__index__` uniformly:
    ///
    /// - `Int` / `Bool` / `BigInt`: returned unchanged (the common, branch-cheap
    ///   path — checked first so plain-int indexing has no extra work).
    /// - `PyInstance` that is an `int`/`bool` subclass (#1929): its primitive
    ///   backing is returned directly (the object already *is* an int, so the
    ///   backing wins even over a user `__index__` override, matching CPython).
    /// - `PyInstance` with a user `__index__`, or `PyClass` whose metaclass
    ///   defines `__index__`: the slot is called; its result must be an integer
    ///   or integer subclass. Integer-subclass results are unwrapped to their
    ///   primitive backing (CPython also emits a `DeprecationWarning`, which
    ///   pyrust does not currently model); every other result raises
    ///   `TypeError: __index__ returned non-int (type X)`.
    /// - Anything else (incl. `float`, `__int__`-only objects, instances without
    ///   `__index__`): the caller-supplied `not_index_err` closure produces the
    ///   context-specific `TypeError` (CPython varies the message per context:
    ///   `"'X' object cannot be interpreted as an integer"`, `"list indices must
    ///   be integers or slices, not X"`, etc.).
    ///
    /// Replaced ~40 open-coded coercions (issue #2022); the thin wrappers below
    /// (`call_index_protocol`, `coerce_range_arg_big`, `resolve_index_arg`,
    /// `try_resolve_index_value`, `try_index_for_seq_repeat`) all route here.
    pub(crate) fn value_to_index(
        &mut self,
        val: &Value,
        not_index_err: impl FnOnce(&Value) -> PyError,
    ) -> Result<Value> {
        self.try_value_to_index(val)?
            .ok_or_else(|| not_index_err(val))
    }

    /// Resolve `val` through the shared index protocol when one exists.
    ///
    /// This is the optional counterpart to [`Interpreter::value_to_index`]:
    /// primitive integers, integer subclasses, objects defining `__index__`,
    /// and class objects whose metaclass defines `__index__` return
    /// `Some(integer)`. A non-index value, including a user instance or class
    /// object with no applicable slot, returns `None`. Exceptions raised by the
    /// slot and invalid slot results remain errors; `None` means only that the
    /// protocol is absent.
    ///
    /// Consumers whose Python API has a fallback after a missing index slot
    /// (for example iterable byte sources) use this method instead of encoding
    /// an internal sentinel as a Python-visible error name.
    pub(crate) fn try_value_to_index(&mut self, val: &Value) -> Result<Option<Value>> {
        // Fast path: a primitive integer is already its own index.  Checked
        // before any class/dunder probe so plain `a[i]` stays branch-cheap.
        match val.kind() {
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
                return Ok(Some(val.clone()));
            }
            ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {}
            _ => return Ok(None),
        }
        // Issue #1929: an int/bool subclass *is* the int it backs, so the
        // backing value is used directly (it wins over a user `__index__`
        // override, matching CPython's C-level int reuse).
        if let Some(backing) = coerce_subclass_backing(val, &[])
            && matches!(
                backing.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return Ok(Some(backing));
        }
        let Some(method_val) = lookup_value_special_method(val, "__index__") else {
            return Ok(None);
        };
        let result = invoke_class_method(self, method_val, val.clone(), &[])?;
        normalize_index_result(result).map(Some)
    }

    /// Resolve an index argument for **sequence item access** (`a[i]`, `a[i] = v`,
    /// `del a[i]`) through the `__index__` protocol, matching CPython 3.12.
    ///
    /// Thin wrapper over [`Interpreter::value_to_index`] that supplies the
    /// per-type error message via [`seq_index_type_error`]:
    /// - list/tuple/bytes: `"X indices must be integers or slices, not Y"`
    /// - string: `"string indices must be integers, not 'Y'"` (different!)
    pub(crate) fn call_index_protocol(&mut self, val: &Value, label: &str) -> Result<Value> {
        self.value_to_index(val, |v| {
            pyrust_core::type_err!(seq_index_type_error(label, &value_type_name_str(v)))
        })
    }

    /// Resolve `val` to an `i64` (a Py_ssize_t-sized count/index) through the
    /// shared index protocol ([`Interpreter::value_to_index`]).  A non-integer
    /// raises `'X' object cannot be interpreted as an integer`; a `BigInt` that
    /// doesn't fit raises `OverflowError(overflow_msg)`.  Used by counted APIs
    /// (`range`, `itertools.repeat`, …) that need a concrete `i64` (#2022).
    pub(crate) fn value_to_isize(&mut self, val: &Value, overflow_msg: &str) -> Result<i64> {
        let resolved = self.value_to_index(val, |v| {
            pyrust_core::type_err!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(v)
            )
        })?;
        index_value_to_i64(&resolved, overflow_msg)
    }
}
