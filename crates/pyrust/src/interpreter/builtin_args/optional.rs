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

    fn try_from_value_with_interp(
        interp: &mut crate::interpreter::Interpreter,
        value: &'a Value,
        fn_name: &str,
        arg_name: &str,
    ) -> Result<Self> {
        if value.is_none() {
            return Ok(None);
        }
        if T::matches(value) {
            return T::try_from_value_with_interp(interp, value, fn_name, arg_name).map(Some);
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

// ─── OptionalArg<T> — distinguishes omitted argument from explicit None ───────
//
// Unlike `Option<T>`, which maps both an absent argument and an explicit
// Python `None` to `None`, `OptionalArg<T>` only produces `Omitted` via
// `#[default(OptionalArg::Omitted)]`; any supplied argument (including `None`)
// converts via `T::try_from_value` and yields `Provided(T)`.

#[derive(Debug, Clone, Copy)]
pub(crate) enum OptionalArg<T> {
    Omitted,
    Provided(T),
}

pub(crate) type OptValueRef<'a> = OptionalArg<PyValueRef<'a>>;

impl<'a> OptValueRef<'a> {
    #[inline(always)]
    pub(crate) fn as_value(&self) -> Option<&'a Value> {
        match self {
            OptionalArg::Omitted => None,
            OptionalArg::Provided(v) => Some(v.0),
        }
    }
}

impl<'a, T: FromValue<'a>> FromValue<'a> for OptionalArg<T> {
    const PY_TYPE_NAME: &'static str = T::PY_TYPE_NAME;

    #[inline(always)]
    fn try_from_value(value: &'a Value, fn_name: &str, arg_name: &str) -> Result<Self> {
        T::try_from_value(value, fn_name, arg_name).map(OptionalArg::Provided)
    }

    #[inline(always)]
    fn try_from_value_with_interp(
        interp: &mut crate::interpreter::Interpreter,
        value: &'a Value,
        fn_name: &str,
        arg_name: &str,
    ) -> Result<Self> {
        T::try_from_value_with_interp(interp, value, fn_name, arg_name).map(OptionalArg::Provided)
    }

    #[inline(always)]
    fn matches(value: &'a Value) -> bool {
        T::matches(value)
    }
}
