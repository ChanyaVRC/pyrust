/// CPython's default integer string-conversion digit limit (gh-95778). A value
/// of 0 disables the limit; any non-zero value must be >= `INT_MAX_STR_DIGITS_MIN`.
pub const INT_MAX_STR_DIGITS_DEFAULT: usize = 4300;
/// The smallest non-zero limit `sys.set_int_max_str_digits` accepts.
pub const INT_MAX_STR_DIGITS_MIN: usize = 640;

thread_local! {
    /// The active integer string-conversion length limit (decimal/non-power-of-2
    /// base conversions only). 0 means "no limit". This is a dynamic adapter
    /// installed by an execution owner; it is not authoritative interpreter
    /// configuration.
    static INT_MAX_STR_DIGITS: std::cell::Cell<usize> =
        const { std::cell::Cell::new(INT_MAX_STR_DIGITS_DEFAULT) };
}

/// Return the current `int_max_str_digits` limit (0 = unlimited).
pub fn get_int_max_str_digits() -> usize {
    INT_MAX_STR_DIGITS.with(|c| c.get())
}

/// Set the `int_max_str_digits` limit. Callers must validate the value first
/// (0, or >= [`INT_MAX_STR_DIGITS_MIN`]); this is a raw setter.
pub fn set_int_max_str_digits(n: usize) {
    INT_MAX_STR_DIGITS.with(|c| c.set(n));
}

/// A dynamically-scoped override of the core integer string-conversion limit.
///
/// The core value/formatting layer intentionally has no dependency on an
/// `Interpreter`. Runtime owners install the active interpreter's value at an
/// execution boundary, and this guard restores the host thread's previous
/// value on every exit path. The `Rc` marker keeps the TLS guard on the thread
/// where it was created.
#[must_use = "the guard must be kept alive for the whole interpreter execution scope"]
pub struct IntMaxStrDigitsGuard {
    previous: usize,
    _not_send_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

/// Install `limit` for the current dynamic execution scope.
///
/// Scopes may be nested: dropping the returned guard restores the immediately
/// preceding value rather than the process default.
pub fn scoped_int_max_str_digits(limit: usize) -> IntMaxStrDigitsGuard {
    let previous = INT_MAX_STR_DIGITS.with(|current| current.replace(limit));
    IntMaxStrDigitsGuard {
        previous,
        _not_send_sync: std::marker::PhantomData,
    }
}

impl Drop for IntMaxStrDigitsGuard {
    fn drop(&mut self) {
        INT_MAX_STR_DIGITS.with(|current| current.set(self.previous));
    }
}

/// The error message CPython raises when an `int -> str` (base 10) conversion
/// exceeds the limit. The format-side message carries no digit count.
pub fn int_max_str_digits_format_error() -> PyError {
    PyError::named(
        "ValueError",
        format!(
            "Exceeds the limit ({} digits) for integer string conversion; \
             use sys.set_int_max_str_digits() to increase the limit",
            get_int_max_str_digits()
        ),
    )
}

/// Recursively verify that converting `value` (and any base-10 integer it
/// transitively contains) to a string does not exceed the active
/// `int_max_str_digits` limit. Mirrors CPython, which performs the check at the
/// point each contained `int` is rendered, so nested literals such as
/// `repr([10 ** 5000])` are rejected too. `int`/`Bool` (i64-backed) values are
/// always far below the limit and are skipped cheaply.
#[inline]
pub fn check_int_str_conversion(value: &Value) -> std::result::Result<(), PyError> {
    // Fast path: only `BigInt` (or a container that might hold one) can ever
    // exceed the limit. The overwhelmingly common `str(int)` / `f"{int}"` /
    // `repr("...")` cases bail out here without reading the thread-local limit
    // or recursing, keeping the str/format hot paths regression-free.
    if !value_may_exceed_int_str_limit(value) {
        return Ok(());
    }
    let limit = get_int_max_str_digits();
    if limit == 0 {
        return Ok(());
    }
    check_value_digits(value, limit)
}

/// Cheap, allocation-/borrow-free test: could converting `value` to a base-10
/// string ever exceed the digit limit?  True only for `BigInt` and the
/// container kinds that can transitively hold one.  The overwhelmingly common
/// `Int` / `Str` / `Float` values classify from their NaN-box tag alone with no
/// pointer dereference, so str/format hot paths can gate the (cold) recursive
/// check on this with no measurable cost.
#[inline]
pub fn value_may_exceed_int_str_limit(value: &Value) -> bool {
    match top16(value.0) {
        // Container tags: always inspect (may hold a BigInt).
        TAG_LIST | TAG_TUPLE => true,
        // BigInt, small tuples, dicts and sets all live behind TAG_OPAQUE; a
        // single pointer deref classifies them. `BuiltinObject` covers
        // `frozenset`, which can hold a BigInt; the recursive check's
        // `to_key` returns `None` for the other builtin-object kinds so the
        // over-approximation costs nothing beyond a cold `to_key` call.
        TAG_OPAQUE => matches!(
            unsafe { &*value.opaque_ptr() },
            Opaque::PyBigInt(_)
                | Opaque::SmallTuple2 { .. }
                | Opaque::SmallTuple3 { .. }
                | Opaque::Dict(_)
                | Opaque::Set(_)
                | Opaque::BuiltinObject { .. }
        ),
        // Every scalar tag (Int / Float / Str / Bool / None / …) is exempt.
        _ => false,
    }
}

fn check_value_digits(value: &Value, limit: usize) -> std::result::Result<(), PyError> {
    match value.kind() {
        ValueKind::BigInt(b) if bigint_exceeds_digit_limit(b, limit) => {
            return Err(int_max_str_digits_format_error());
        }
        ValueKind::List(items) => {
            // Reuse the `repr` cycle guard (#364): a self-referential container
            // (`d["k"] = d`) must not loop forever here. When the id is already
            // on the stack we've hit a cycle and stop descending — the eventual
            // render path emits the `[...]` placeholder for it anyway.
            let _guard = match value.value_id() {
                Some(id) => match ReprGuard::enter(id) {
                    Some(g) => Some(g),
                    None => return Ok(()),
                },
                None => None,
            };
            for item in items.iter() {
                check_value_digits(item, limit)?;
            }
        }
        ValueKind::Tuple(items) => {
            for item in items.iter() {
                check_value_digits(item, limit)?;
            }
        }
        ValueKind::Set(items) => {
            for key in items.iter() {
                check_key_digits(key, limit)?;
            }
        }
        ValueKind::Dict(entries) => {
            let _guard = match value.value_id() {
                Some(id) => match ReprGuard::enter(id) {
                    Some(g) => Some(g),
                    None => return Ok(()),
                },
                None => None,
            };
            for (k, v) in entries.iter() {
                check_key_digits(k, limit)?;
                check_value_digits(v, limit)?;
            }
        }
        // `frozenset` is a `BuiltinObject`, not an `Opaque::Set`, so a top-level
        // `repr(frozenset({10 ** 5000}))` would otherwise escape the check.
        // `to_key` yields the canonical `PyKey::FrozenSet` for hashable builtin
        // objects (and `None` for the rest), routing its elements through the
        // existing key-side walk. Non-frozenset builtin objects return `None`
        // here and are skipped cheaply.
        ValueKind::BuiltinObject { ops, state } => {
            if let Some(key) = ops.to_key(state) {
                check_key_digits(&key, limit)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn check_key_digits(key: &PyKey, limit: usize) -> std::result::Result<(), PyError> {
    match key {
        PyKey::BigInt(b) if bigint_exceeds_digit_limit(b, limit) => {
            return Err(int_max_str_digits_format_error());
        }
        PyKey::Tuple(items) => {
            for item in items.iter() {
                check_key_digits(item, limit)?;
            }
        }
        PyKey::FrozenSet(key) => {
            for item in key.items() {
                check_key_digits(item, limit)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Whether a numeric base is exempt from the `int_max_str_digits` limit.
/// CPython exempts power-of-two bases (2, 4, 8, 16, 32) because their
/// conversion is linear, not quadratic.
pub fn int_str_base_is_exempt(base: u32) -> bool {
    matches!(base, 2 | 4 | 8 | 16 | 32)
}

/// Enforce the `int_max_str_digits` limit on a `str -> int` parse for the given
/// `base` (0 means "auto-detect"; the caller passes the *detected* base for the
/// base-0 path). `digit_str` is the already-stripped digit run (sign and any
/// base prefix removed); underscores are tolerated and not counted, matching
/// CPython. Power-of-two bases are exempt. The parse-side message includes the
/// digit count, unlike the format-side message.
pub fn check_int_parse_digits(digit_str: &str, base: u32) -> std::result::Result<(), PyError> {
    let limit = get_int_max_str_digits();
    if limit == 0 || int_str_base_is_exempt(base) {
        return Ok(());
    }
    let digits = digit_str.bytes().filter(|b| *b != b'_').count();
    if digits > limit {
        return Err(PyError::named(
            "ValueError",
            format!(
                "Exceeds the limit ({limit} digits) for integer string conversion: \
                 value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
            ),
        ));
    }
    Ok(())
}

/// Whether a `BigInt`'s base-10 string would exceed the active
/// `int_max_str_digits` limit (0 = unlimited → never exceeds).
pub fn bigint_str_digits_exceed_limit(b: &BigInt) -> bool {
    let limit = get_int_max_str_digits();
    if limit == 0 {
        return false;
    }
    bigint_exceeds_digit_limit(b, limit)
}

/// Exact decimal-digit count check for a `BigInt`. Uses a cheap bit-length
/// estimate to skip values that are obviously within the limit, and only falls
/// back to the precise `to_string` count near the boundary (the limit defaults
/// to 4300, so the precise path is rare and bounded).
fn bigint_exceeds_digit_limit(b: &BigInt, limit: usize) -> bool {
    // log10(2) ≈ 0.30103. A number with `bits` significant bits has at most
    // floor(bits * log10(2)) + 1 decimal digits. We use this conservative upper
    // bound to short-circuit small values without materialising the string.
    let bits = b.bits();
    let upper = (bits.saturating_mul(30103) / 100000) as usize + 1;
    if upper <= limit {
        return false;
    }
    // Near or above the boundary: count exact digits (excluding the sign).
    let s = b.to_string();
    let digits = s.strip_prefix('-').unwrap_or(&s).len();
    digits > limit
}

#[cfg(test)]
mod int_max_str_digits_scope_tests {
    use super::{get_int_max_str_digits, scoped_int_max_str_digits, set_int_max_str_digits};

    #[test]
    fn scoped_limit_is_nested_and_restores_the_host_value() {
        let _host = scoped_int_max_str_digits(777);
        assert_eq!(get_int_max_str_digits(), 777);
        {
            let _outer = scoped_int_max_str_digits(888);
            assert_eq!(get_int_max_str_digits(), 888);
            {
                let _inner = scoped_int_max_str_digits(999);
                assert_eq!(get_int_max_str_digits(), 999);
            }
            assert_eq!(get_int_max_str_digits(), 888);
            // A runtime update inside a scope is visible until the scope ends.
            set_int_max_str_digits(1111);
            assert_eq!(get_int_max_str_digits(), 1111);
        }
        assert_eq!(get_int_max_str_digits(), 777);
    }
}
