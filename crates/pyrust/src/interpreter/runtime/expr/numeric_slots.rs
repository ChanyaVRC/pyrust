/// Fallible `BigInt -> f64`.  Raises `OverflowError` (CPython parity:
/// `int too large to convert to float`) when the BigInt's magnitude is
/// outside f64's representable range, instead of silently returning
/// `f64::INFINITY` (which loses sign and produces nonsense `inf`
/// arithmetic).  Centralised here for the mixed BigInt±Float arms in
/// add/sub/mul (PR #484 Copilot review).
pub(super) fn bigint_to_float_or_overflow(b: &PyBigInt) -> Result<f64> {
    b.to_f64()
        .filter(|f| f.is_finite())
        .ok_or_else(|| pyrust_core::overflow_err!("int too large to convert to float"))
}

/// Sequence repeat helpers.  Each raises the appropriate Python error when
/// the repeat count or resulting allocation exceeds platform limits, matching
/// CPython 3.12 behaviour:
///
/// - `n <= 0`                                → empty result
/// - `BigInt` (any)                          → `OverflowError: cannot fit 'int' into an index-sized integer`
/// - `Int` and `char_count * n > isize::MAX` → `OverflowError: repeated string is too long`
/// - allocation fails (OOM)                  → `MemoryError`
fn seq_repeat_str(text: &str, n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(Value::string(String::new()));
    }
    let n = n as usize;
    // Fast path: if byte_total fits in isize::MAX then char_count * n ≤ byte_total
    // ≤ isize::MAX, so the CPython char-count overflow check cannot fire.  We only
    // pay for chars().count() when byte_total itself already approaches the limit.
    let byte_total = match text.len().checked_mul(n) {
        Some(b) => b,
        None => return Err(pyrust_core::py_err!("MemoryError", String::new())),
    };
    if byte_total > isize::MAX as usize {
        // Only compute char_count here; CPython raises OverflowError when
        // char_count * n > Py_ssize_t_MAX, MemoryError otherwise.
        let char_count = text.chars().count();
        if char_count
            .checked_mul(n)
            .is_none_or(|t| t > isize::MAX as usize)
        {
            return Err(pyrust_core::overflow_err!("repeated string is too long"));
        }
        // char_count * n fits, but byte_total doesn't — OOM.
        return Err(pyrust_core::py_err!("MemoryError", String::new()));
    }
    // Reserve the result buffer once (try_reserve catches OOM rather than
    // letting the allocator abort), then fill it in place with str::repeat's
    // O(log n) doubling.  The previous version reserved a throwaway `probe`
    // and *then* let `str::repeat` allocate a second buffer — a wasted
    // allocation per call (issue: str repeat ~2.2x slower than CPython).
    let mut result = String::new();
    if result.try_reserve_exact(byte_total).is_err() {
        return Err(pyrust_core::py_err!("MemoryError", String::new()));
    }
    result.push_str(text);
    while result.len() < byte_total {
        // Both `byte_total` and `result.len()` are multiples of `text.len()`,
        // so `copy` is too.
        let copy = (byte_total - result.len()).min(result.len());
        // SAFETY: `result` currently holds a whole number of `text` copies and
        // `copy` is a multiple of `text.len()`, so appending the first `copy`
        // bytes keeps the buffer valid UTF-8.  No reallocation: `byte_total`
        // capacity was reserved above.
        unsafe {
            result.as_mut_vec().extend_from_within(..copy);
        }
    }
    Ok(Value::string(result))
}

fn seq_repeat_list(items: &[Value], n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(Value::list(Vec::new()));
    }
    let n = n as usize;
    let total = match items.len().checked_mul(n) {
        Some(t) => t,
        None => return Err(pyrust_core::py_err!("MemoryError", String::new())),
    };
    let mut out: Vec<Value> = Vec::new();
    if out.try_reserve(total).is_err() {
        return Err(pyrust_core::py_err!("MemoryError", String::new()));
    }
    for _ in 0..n {
        out.extend_from_slice(items);
    }
    Ok(Value::list(out))
}

fn seq_repeat_bytes(data: &[u8], n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(Value::bytes(Vec::new()));
    }
    let n = n as usize;
    let total = match data.len().checked_mul(n) {
        Some(t) => t,
        None => return Err(pyrust_core::py_err!("MemoryError", String::new())),
    };
    let mut out: Vec<u8> = Vec::new();
    if out.try_reserve(total).is_err() {
        return Err(pyrust_core::py_err!("MemoryError", String::new()));
    }
    for _ in 0..n {
        out.extend_from_slice(data);
    }
    Ok(Value::bytes(out))
}

fn seq_repeat_bytearray(data: &[u8], n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(pyrust_builtins::bytearray::bytearray(Vec::new()));
    }
    let n = n as usize;
    let total = match data.len().checked_mul(n) {
        Some(t) => t,
        None => return Err(pyrust_core::py_err!("MemoryError", String::new())),
    };
    let mut out: Vec<u8> = Vec::new();
    if out.try_reserve(total).is_err() {
        return Err(pyrust_core::py_err!("MemoryError", String::new()));
    }
    for _ in 0..n {
        out.extend_from_slice(data);
    }
    Ok(pyrust_builtins::bytearray::bytearray(out))
}

/// `true` when `v` is a `PyInstance` whose class defines its own `__ne__`
/// (i.e. the resolved `__ne__` is *not* the inherited `object.__ne__`).
///
/// Used by `BinaryOp::Ne` (issue #2645) to decide whether to dispatch a
/// user-defined `__ne__` or to fall back to negating `__eq__`.  The inherited
/// canonical `object.__ne__` only compares identity — exactly the slot CPython
/// replaces with `not __eq__`.
fn pyinstance_has_user_ne(v: &Value) -> bool {
    let ValueKind::PyInstance(inst) = v.kind() else {
        return false;
    };
    let class = Rc::clone(&inst.borrow().class);
    lookup_class_attr(&class, "__ne__")
        .as_ref()
        .is_some_and(|method| {
            !crate::interpreter::value_is_canonical_slot(
                method,
                crate::interpreter::CanonicalSlot::ObjectNe,
            )
        })
}

/// Correctly-rounded `int / int` true division, mirroring CPython's
/// `long_true_divide` (`Objects/longobject.c`).  Operands are never
/// converted to `f64` first — that would raise `OverflowError` for any
/// operand outside `f64` range even when the *quotient* is representable
/// (issue #1923).  Instead the correctly-rounded (round-half-to-even)
/// quotient is computed with exact integer arithmetic and only the
/// *result* is checked for overflow.
///
/// Returns `Err(OverflowError)` only when the quotient itself is too
/// large for `f64`, and `Err(ZeroDivisionError)` when `den == 0`.
fn bigint_true_divide(num: &PyBigInt, den: &PyBigInt) -> Result<f64> {
    // f64 layout constants (IEEE-754 double).
    const MANT_DIG: i64 = 53; // significand bits (incl. implicit leading 1)
    const MIN_EXP: i64 = -1021; // frexp's minimum normal exponent
    const MAX_EXP: i64 = 1024; // frexp's maximum exponent
    const SUBNORMAL_LSB: i64 = MIN_EXP - MANT_DIG; // -1074: lsb exponent of smallest subnormal

    if den.is_zero() {
        return Err(pyrust_core::zerodiv_err!("division by zero"));
    }
    let negate = (num.sign() == PyBigIntSign::Minus) != (den.sign() == PyBigIntSign::Minus);
    if num.is_zero() {
        return Ok(if negate { -0.0 } else { 0.0 });
    }
    // Work with magnitudes; the sign is reapplied at the end.
    let a = magnitude(num);
    let b = magnitude(den);
    let diff = a.bits() as i64 - b.bits() as i64;

    // q = a/b lies in [2^(diff-1), 2^(diff+1)).  Bail out early on the
    // extremes both to match CPython and to keep the working shifts bounded.
    if diff > MAX_EXP {
        return Err(pyrust_core::overflow_err!(
            "integer division result too large for a float"
        ));
    }
    if diff < SUBNORMAL_LSB - 1 {
        // Quotient is below half the smallest subnormal: rounds to 0.
        return Ok(if negate { -0.0 } else { 0.0 });
    }

    // Compute the integer quotient `x` whose lsb has exponent `shift`,
    // keeping at least two guard bits below the eventual rounding point so
    // the single round-half-even step below always drops >= 2 bits.
    let l0 = ((diff - 1) - (MANT_DIG - 1)).max(SUBNORMAL_LSB);
    let mut shift = l0 - 2;
    let (mut x, rem) = if shift <= 0 {
        let scaled = &a << ((-shift) as usize);
        (&scaled / &b, &scaled % &b)
    } else {
        let scaled_b = &b << (shift as usize);
        (&a / &scaled_b, &a % &scaled_b)
    };
    let inexact = !rem.is_zero();
    if x.is_zero() {
        return Ok(if negate { -0.0 } else { 0.0 });
    }

    // Refine the true exponent of the quotient from `x`, then round its lsb
    // to the exponent `l` allowed by the (normal or subnormal) result.
    let e = (x.bits() as i64 - 1) + shift;
    let l = (e - (MANT_DIG - 1)).max(SUBNORMAL_LSB);
    let drop = (l - shift) as usize; // >= 2 by construction
    let low = &x & ((PyBigInt::from(1) << drop) - 1);
    x >>= drop;
    let half = PyBigInt::from(1) << (drop - 1);
    let round_up =
        low > half || (low == half && ((&x & PyBigInt::from(1)) == PyBigInt::from(1) || inexact));
    if round_up {
        x += 1;
    }
    shift = l;

    // `x` now has at most MANT_DIG + 1 bits (after a possible rounding
    // carry), so it converts to f64 exactly; ldexp applies the exponent.
    let mantissa = x.to_f64().filter(|f| f.is_finite()).ok_or_else(|| {
        pyrust_core::overflow_err!("integer division result too large for a float")
    })?;
    let result = ldexp_f64(mantissa, shift);
    if !result.is_finite() {
        return Err(pyrust_core::overflow_err!(
            "integer division result too large for a float"
        ));
    }
    Ok(if negate { -result } else { result })
}

/// Absolute value of a `PyBigInt` (CPython's true division works on
/// magnitudes, reapplying the sign at the end).
fn magnitude(b: &PyBigInt) -> PyBigInt {
    if b.sign() == PyBigIntSign::Minus {
        -b
    } else {
        b.clone()
    }
}

/// `m * 2^exp` for finite `m` — a faithful C `ldexp`.  Applying the whole
/// exponent as a single `2^exp` factor would underflow to `0.0` for a
/// subnormal *result* (e.g. `2^52 * 2^-1110` whose `2^-1110` factor
/// vanishes even though the product `2^-1058` is representable).  The
/// exponent is therefore applied in steps small enough that each
/// intermediate factor stays in the normal range; the final step performs
/// the single correct rounding into the subnormal range.  Out-of-range
/// exponents over/underflow to `inf` / `0.0` (the overflow case is caught
/// by the caller).
fn ldexp_f64(mut m: f64, mut exp: i64) -> f64 {
    if m == 0.0 || !m.is_finite() {
        return m;
    }
    // Each factor of 2^512 is safely within f64's normal range, so stepping
    // by ±512 never under/overflows an intermediate while still converging.
    const STEP: i64 = 512;
    const STEP_FACTOR: f64 = 1.340_780_792_994_259_7e154; // 2^512
    while exp > STEP {
        m *= STEP_FACTOR;
        exp -= STEP;
    }
    while exp < -STEP {
        m /= STEP_FACTOR;
        exp += STEP;
    }
    // |exp| <= 512 now: a single multiply by 2^exp rounds correctly, even
    // when the result is subnormal.
    m * 2.0_f64.powi(exp as i32)
}

/// Python-style `(quotient, remainder)` for `a // b` and `a % b` where
/// both operands are BigInts.  Unlike Rust's `/` / `%` (truncate-toward-
/// zero), CPython uses floor division: the quotient is rounded toward
/// negative infinity and the remainder has the same sign as the
/// divisor.  Caller must guarantee `b != 0`.
///
/// Shared with `builtin_modules/bodies/builtins.rs` (the `divmod()`
/// builtin) to avoid divergence in sign-adjustment logic (issue #493).
pub(crate) fn bigint_divmod_floor(a: &PyBigInt, b: &PyBigInt) -> (PyBigInt, PyBigInt) {
    let mut q = a / b;
    let mut r = a % b;
    // Adjust if the truncated remainder's sign disagrees with the
    // divisor: subtract one from the quotient and add `b` back into the
    // remainder so it matches the divisor's sign (CPython semantics).
    if !r.is_zero() && (r.sign() != b.sign()) {
        q -= 1;
        r += b;
    }
    (q, r)
}

/// Coerce `Int` / `BigInt` / `Bool` to `PyBigInt` for cross-type
/// arithmetic.  Returns `None` for anything else so callers can fall
/// through to the float / TypeError path.
///
/// Shared with `builtin_modules/bodies/builtins.rs` (the `divmod()`
/// builtin) to avoid divergence in coercion logic (issue #493).
/// Normalize a `BigInt` to a `Value`: a plain `Value::int` when it fits in
/// `i64` (which itself collapses small values to the inline TAG_INT form),
/// otherwise a heap `Value::bigint`.  Mirrors the int-overflow promotion used
/// throughout numeric arithmetic (#2118).
pub(crate) fn value_from_bigint(n: PyBigInt) -> Value {
    match n.to_i64() {
        Some(i) => Value::int(i),
        None => Value::bigint(n),
    }
}

pub(crate) fn value_to_bigint(v: &Value) -> Option<PyBigInt> {
    match v.kind() {
        ValueKind::Int(n) => Some(PyBigInt::from(n)),
        ValueKind::BigInt(b) => Some(b.clone()),
        ValueKind::Bool(b) => Some(PyBigInt::from(b as i64)),
        _ => None,
    }
}

/// Result of validating a shift count: either a concrete `usize`
/// (small enough to apply directly), or a marker that the count is
/// non-negative but exceeds `usize::MAX`.  Each shift arm decides how
/// to handle the saturating case — `<<` raises `OverflowError` only
/// when the LHS is non-zero (CPython would actually allocate the
/// bits), while `>>` collapses to `0` / `-1` (CPython parity).
enum ShiftCount {
    Fits(usize),
    Saturated,
}

/// Maximum left-shift count we are willing to materialise at runtime.
/// CPython raises `OverflowError` ("too many digits in integer") for
/// results that would exceed `sys.maxsize` digits; we are more
/// conservative and cap at 2^30 ≈ 10^9 bits (~128 MiB worst-case),
/// which is large enough for any realistic computation.
const MAX_SHIFT: usize = 1 << 30;

/// Validate a shift count and convert it to `ShiftCount`.  Returns
/// `Err(ValueError)` for negative shifts and `Err(TypeError)` if the
/// operand isn't an int / bool.  Call sites replace the TypeError message
/// with the operand-specific "unsupported operand type(s) for OP: 'X' and 'Y'"
/// format via `map_err`.
fn shift_count(v: &Value) -> Result<ShiftCount> {
    let big = value_to_bigint(v).ok_or_else(|| {
        // Caller replaces this message via map_err; see LShift / RShift arms.
        pyrust_core::type_err!(String::new())
    })?;
    match big.sign() {
        PyBigIntSign::Minus => Err(pyrust_core::value_err!("negative shift count")),
        PyBigIntSign::NoSign => Ok(ShiftCount::Fits(0)),
        PyBigIntSign::Plus => Ok(match big.to_usize() {
            Some(n) => ShiftCount::Fits(n),
            None => ShiftCount::Saturated,
        }),
    }
}

/// Repeat a tuple slice `n` times, matching CPython 3.12 `tuplerepeat`
/// semantics:
///
/// - `n <= 0` → empty tuple (no allocation).
/// - `items.len() * n > isize::MAX` → `MemoryError` (catches overflow
///   before any allocation attempt, preventing an allocator abort).
/// - allocation failure (Vec::try_reserve) → `MemoryError`.
fn seq_repeat_tuple(items: &[Value], n: i64) -> Result<Value> {
    if n <= 0 {
        return Ok(Value::tuple(Vec::new()));
    }
    let n = n as usize;
    let total = items
        .len()
        .checked_mul(n)
        .filter(|&t| t <= isize::MAX as usize);
    let total = total.ok_or_else(|| pyrust_core::py_err!("MemoryError", String::new()))?;
    let mut out: Vec<Value> = Vec::new();
    out.try_reserve(total)
        .map_err(|_| pyrust_core::py_err!("MemoryError", String::new()))?;
    for _ in 0..n {
        out.extend_from_slice(items);
    }
    Ok(Value::tuple(out))
}

/// Numeric protocol slots, analogous to CPython's `PyNumberMethods`
/// (issue #458).  Each numeric kind (`Int`, `Float`, `BigInt`, `Bool`)
/// is handled by one canonical slot implementation; binary dispatch
/// (`dispatch_numeric_binop`) routes through the LHS slot and treats a
/// `None` return as CPython's `NotImplemented` — the caller then falls
/// through to sequence / container handling and finally `TypeError`.
///
/// The goal is a single canonical site per `(op)` covering all numeric
/// type pairs, so that adding a numeric type means extending this slot
/// table once rather than touching every `match (lhs, rhs)` arm.  All
/// binary numeric operators route here — arithmetic (`+ - * / // % **`),
/// shifts (`<< >>`), and bitwise (`& | ^`).  The pure-`Int`/`Int` and
/// `Float`/`Float` VM hot paths (`fast_path.rs::int_int_fast`,
/// `float_float_fast`) are intentionally left untouched — they
/// short-circuit before any slot lookup.
///
/// Dispatch is a flat `match value.kind()` (see `numeric_slot`), never a
/// `Box<dyn NumericOps>`, so there is no heap allocation or virtual call
/// on the arithmetic path.
trait NumericOps {
    fn nb_add(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_sub(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_mul(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_div(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_floordiv(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_mod(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_pow(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_lshift(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_rshift(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_and(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_or(&self, rhs: &Value) -> Option<Result<Value>>;
    fn nb_xor(&self, rhs: &Value) -> Option<Result<Value>>;
}

/// One numeric operand, classified by kind so a single canonical slot
/// implementation can be selected without virtual dispatch.
enum NumericSlot {
    Int(i64),
    Float(f64),
    BigInt(PyBigInt),
}

/// Classify a value as a numeric operand, or `None` for non-numeric
/// kinds (which dispatch through the sequence / container / dunder paths
/// instead).  `Bool` coerces to `Int`, exactly as CPython treats `bool`
/// as a subtype of `int` in arithmetic.
fn numeric_slot(v: &Value) -> Option<NumericSlot> {
    match v.kind() {
        ValueKind::Int(n) => Some(NumericSlot::Int(n)),
        ValueKind::Bool(b) => Some(NumericSlot::Int(b as i64)),
        ValueKind::Float(f) => Some(NumericSlot::Float(f)),
        ValueKind::BigInt(b) => Some(NumericSlot::BigInt(b.clone())),
        _ => None,
    }
}

impl NumericOps for NumericSlot {
    fn nb_add(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        Some(match (self, &r) {
            (NumericSlot::Int(a), NumericSlot::Int(b)) => Ok(match a.checked_add(*b) {
                Some(s) => Value::int(s),
                None => Value::bigint(PyBigInt::from(*a) + PyBigInt::from(*b)),
            }),
            (NumericSlot::Int(a), NumericSlot::Float(b)) => Ok(Value::float((*a as f64) + b)),
            (NumericSlot::Float(a), NumericSlot::Int(b)) => Ok(Value::float(a + (*b as f64))),
            (NumericSlot::Float(a), NumericSlot::Float(b)) => Ok(Value::float(a + b)),
            (NumericSlot::BigInt(a), NumericSlot::BigInt(b)) => Ok(Value::bigint(a + b)),
            (NumericSlot::BigInt(a), NumericSlot::Int(b)) => {
                Ok(Value::bigint(a + PyBigInt::from(*b)))
            }
            (NumericSlot::Int(a), NumericSlot::BigInt(b)) => {
                Ok(Value::bigint(PyBigInt::from(*a) + b))
            }
            (NumericSlot::BigInt(a), NumericSlot::Float(b)) => {
                bigint_to_float_or_overflow(a).map(|a| Value::float(a + b))
            }
            (NumericSlot::Float(a), NumericSlot::BigInt(b)) => {
                bigint_to_float_or_overflow(b).map(|b| Value::float(a + b))
            }
        })
    }

    fn nb_sub(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        Some(match (self, &r) {
            (NumericSlot::Int(a), NumericSlot::Int(b)) => Ok(match a.checked_sub(*b) {
                Some(s) => Value::int(s),
                None => Value::bigint(PyBigInt::from(*a) - PyBigInt::from(*b)),
            }),
            (NumericSlot::Int(a), NumericSlot::Float(b)) => Ok(Value::float((*a as f64) - b)),
            (NumericSlot::Float(a), NumericSlot::Int(b)) => Ok(Value::float(a - (*b as f64))),
            (NumericSlot::Float(a), NumericSlot::Float(b)) => Ok(Value::float(a - b)),
            (NumericSlot::BigInt(a), NumericSlot::BigInt(b)) => Ok(Value::bigint(a - b)),
            (NumericSlot::BigInt(a), NumericSlot::Int(b)) => {
                Ok(Value::bigint(a - PyBigInt::from(*b)))
            }
            (NumericSlot::Int(a), NumericSlot::BigInt(b)) => {
                Ok(Value::bigint(PyBigInt::from(*a) - b))
            }
            (NumericSlot::BigInt(a), NumericSlot::Float(b)) => {
                bigint_to_float_or_overflow(a).map(|a| Value::float(a - b))
            }
            (NumericSlot::Float(a), NumericSlot::BigInt(b)) => {
                bigint_to_float_or_overflow(b).map(|b| Value::float(a - b))
            }
        })
    }

    fn nb_mul(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        Some(match (self, &r) {
            (NumericSlot::Int(a), NumericSlot::Int(b)) => Ok(match a.checked_mul(*b) {
                Some(s) => Value::int(s),
                None => Value::bigint(PyBigInt::from(*a) * PyBigInt::from(*b)),
            }),
            (NumericSlot::Int(a), NumericSlot::Float(b)) => Ok(Value::float((*a as f64) * b)),
            (NumericSlot::Float(a), NumericSlot::Int(b)) => Ok(Value::float(a * (*b as f64))),
            (NumericSlot::Float(a), NumericSlot::Float(b)) => Ok(Value::float(a * b)),
            (NumericSlot::BigInt(a), NumericSlot::BigInt(b)) => Ok(Value::bigint(a * b)),
            (NumericSlot::BigInt(a), NumericSlot::Int(b)) => {
                Ok(Value::bigint(a * PyBigInt::from(*b)))
            }
            (NumericSlot::Int(a), NumericSlot::BigInt(b)) => {
                Ok(Value::bigint(PyBigInt::from(*a) * b))
            }
            (NumericSlot::BigInt(a), NumericSlot::Float(b)) => {
                bigint_to_float_or_overflow(a).map(|a| Value::float(a * b))
            }
            (NumericSlot::Float(a), NumericSlot::BigInt(b)) => {
                bigint_to_float_or_overflow(b).map(|b| Value::float(a * b))
            }
        })
    }

    fn nb_div(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        // CPython true division always yields a float.  The int/int case must
        // NOT convert operands to f64 first (that overflows for any operand
        // beyond f64 range even when the quotient is representable, #1923) —
        // route through the correctly-rounded integer divider.  The small
        // int/int fast path also goes here but avoids any BigInt allocation
        // for typical magnitudes; only operands actually beyond f64 range pay
        // for the full algorithm.
        Some(match (self, &r) {
            // Small int/int fast path: when both operands are exactly
            // representable as f64 (|n| < 2^53), IEEE-754 division is itself
            // correctly rounded and matches CPython byte-for-byte — no BigInt
            // allocation.  Larger magnitudes route through the exact divider.
            (NumericSlot::Int(a), NumericSlot::Int(b)) => {
                if *b == 0 {
                    Err(pyrust_core::zerodiv_err!("division by zero"))
                } else if a.unsigned_abs() < (1 << 53) && b.unsigned_abs() < (1 << 53) {
                    Ok(Value::float(*a as f64 / *b as f64))
                } else {
                    bigint_true_divide(&PyBigInt::from(*a), &PyBigInt::from(*b)).map(Value::float)
                }
            }
            (
                NumericSlot::Int(_) | NumericSlot::BigInt(_),
                NumericSlot::Int(_) | NumericSlot::BigInt(_),
            ) => bigint_true_divide(&slot_to_bigint(self), &slot_to_bigint(&r)).map(Value::float),
            // At least one Float operand: CPython divides as doubles and the
            // ZeroDivisionError wording is "float division by zero".
            _ => (|| {
                let a = slot_to_float(self)?;
                let b = slot_to_float(&r)?;
                if b == 0.0 {
                    return Err(pyrust_core::zerodiv_err!("float division by zero"));
                }
                Ok(Value::float(a / b))
            })(),
        })
    }

    fn nb_floordiv(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        Some(match (self, &r) {
            (NumericSlot::Int(a), NumericSlot::Int(b)) => {
                if *b == 0 {
                    Err(zero_div("integer division or modulo by zero"))
                } else {
                    let modulo = py_mod_i64(*a, *b);
                    // `a - modulo` / `a_adj / b` can overflow near i64::MIN;
                    // promote to BigInt for exact floor division (#485).
                    if let Some(q) = a
                        .checked_sub(modulo)
                        .and_then(|a_adj| a_adj.checked_div(*b))
                    {
                        Ok(Value::int(q))
                    } else {
                        let (q, _) = bigint_divmod_floor(&PyBigInt::from(*a), &PyBigInt::from(*b));
                        Ok(Value::bigint(q))
                    }
                }
            }
            // Any BigInt operand (with int/bigint partner): exact floor div.
            (NumericSlot::BigInt(_), NumericSlot::Int(_) | NumericSlot::BigInt(_))
            | (NumericSlot::Int(_), NumericSlot::BigInt(_)) => {
                let a = slot_to_bigint(self);
                let b = slot_to_bigint(&r);
                if b.is_zero() {
                    Err(zero_div("integer division or modulo by zero"))
                } else {
                    let (q, _) = bigint_divmod_floor(&a, &b);
                    Ok(Value::bigint(q))
                }
            }
            // At least one Float operand: float floor division.  Use CPython's
            // fmod-based float_divmod so infinities/signed zeros match and
            // `a // b == divmod(a, b)[0]` (#2025).
            _ => (|| {
                let a = slot_to_float(self)?;
                let b = slot_to_float(&r)?;
                if b == 0.0 {
                    return Err(zero_div("float floor division by zero"));
                }
                let (div, _) = float_divmod(a, b);
                Ok(Value::float(div))
            })(),
        })
    }

    fn nb_mod(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        Some(match (self, &r) {
            (NumericSlot::Int(a), NumericSlot::Int(b)) => {
                if *b == 0 {
                    Err(zero_div("integer modulo by zero"))
                } else {
                    Ok(Value::int(py_mod_i64(*a, *b)))
                }
            }
            (NumericSlot::BigInt(_), NumericSlot::Int(_) | NumericSlot::BigInt(_))
            | (NumericSlot::Int(_), NumericSlot::BigInt(_)) => {
                let a = slot_to_bigint(self);
                let b = slot_to_bigint(&r);
                if b.is_zero() {
                    Err(zero_div("integer modulo by zero"))
                } else {
                    let (_, rem) = bigint_divmod_floor(&a, &b);
                    Ok(Value::bigint(rem))
                }
            }
            _ => (|| {
                let a = slot_to_float(self)?;
                let b = slot_to_float(&r)?;
                if b == 0.0 {
                    return Err(zero_div("float modulo"));
                }
                let mut rem = a % b;
                if rem == 0.0 {
                    // CPython float_rem: zero result copies sign of divisor.
                    rem = rem.copysign(b);
                } else if rem.signum() != b.signum() {
                    rem += b;
                }
                Ok(Value::float(rem))
            })(),
        })
    }

    fn nb_pow(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        Some(match (self, &r) {
            // Integer ** non-negative integer stays in the int domain
            // (BigInt promotion on overflow, #421).
            (NumericSlot::Int(a), NumericSlot::Int(b)) if *b >= 0 => Ok(int_pow_promoting(*a, *b)),
            (NumericSlot::BigInt(a), NumericSlot::Int(b)) if *b >= 0 => {
                Ok(Value::bigint(PyPow::pow(a.clone(), *b as u64)))
            }
            (NumericSlot::Int(a), NumericSlot::BigInt(b)) if *b >= PyBigInt::from(0) => {
                // BigInt exponent: astronomically large for |a| > 1.
                match b.to_u64_digits().1.as_slice() {
                    [exp] => Ok(Value::bigint(PyPow::pow(PyBigInt::from(*a), *exp))),
                    [] => Ok(Value::int(1)), // a ** 0
                    _ => Err(pyrust_core::overflow_err!(
                        "exponent too large for ** to compute"
                    )),
                }
            }
            (NumericSlot::BigInt(a), NumericSlot::BigInt(b)) if *b >= PyBigInt::from(0) => {
                match b.to_u64_digits().1.as_slice() {
                    [exp] => Ok(Value::bigint(PyPow::pow(a.clone(), *exp))),
                    [] => Ok(Value::int(1)),
                    _ => Err(pyrust_core::overflow_err!(
                        "exponent too large for ** to compute"
                    )),
                }
            }
            // Float base/exponent, or integer with a negative exponent:
            // float exponentiation (with CPython's negative-real → complex
            // promotion and the 0.0 ** negative ZeroDivisionError).
            _ => (|| {
                let a = slot_to_float(self)?;
                let b = slot_to_float(&r)?;
                if a == 0.0 && b < 0.0 && b.is_finite() {
                    return Err(pyrust_core::zerodiv_err!(
                        "0.0 cannot be raised to a negative power"
                    ));
                }
                let result = a.powf(b);
                if a < 0.0 && result.is_nan() {
                    let abs_val = a.abs().powf(b);
                    let angle = std::f64::consts::PI * b;
                    Ok(Value::complex(abs_val * angle.cos(), abs_val * angle.sin()))
                } else {
                    Ok(Value::float(result))
                }
            })(),
        })
    }

    fn nb_lshift(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        // Shift operands must both be integers; a Float operand is not a
        // valid shift operand, so return None to fall through to the
        // operand-type TypeError raised by the caller.
        if matches!(self, NumericSlot::Float(_)) || matches!(r, NumericSlot::Float(_)) {
            return None;
        }
        Some(slot_lshift(self, rhs))
    }

    fn nb_rshift(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        if matches!(self, NumericSlot::Float(_)) || matches!(r, NumericSlot::Float(_)) {
            return None;
        }
        Some(slot_rshift(self, rhs))
    }

    fn nb_and(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        if matches!(self, NumericSlot::Float(_)) || matches!(r, NumericSlot::Float(_)) {
            return None;
        }
        Some(Ok(collapse_bigint(Value::bigint(
            slot_to_bigint(self) & slot_to_bigint(&r),
        ))))
    }

    fn nb_or(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        if matches!(self, NumericSlot::Float(_)) || matches!(r, NumericSlot::Float(_)) {
            return None;
        }
        Some(Ok(collapse_bigint(Value::bigint(
            slot_to_bigint(self) | slot_to_bigint(&r),
        ))))
    }

    fn nb_xor(&self, rhs: &Value) -> Option<Result<Value>> {
        let r = numeric_slot(rhs)?;
        if matches!(self, NumericSlot::Float(_)) || matches!(r, NumericSlot::Float(_)) {
            return None;
        }
        Some(Ok(collapse_bigint(Value::bigint(
            slot_to_bigint(self) ^ slot_to_bigint(&r),
        ))))
    }
}

/// Coerce a classified numeric slot to f64, raising `OverflowError` for
/// a `BigInt` too large to represent (CPython parity).
fn slot_to_float(s: &NumericSlot) -> Result<f64> {
    match s {
        NumericSlot::Int(n) => Ok(*n as f64),
        NumericSlot::Float(f) => Ok(*f),
        NumericSlot::BigInt(b) => bigint_to_float_or_overflow(b),
    }
}

/// Coerce a classified `Int` / `BigInt` slot to `PyBigInt`.  Caller must
/// guarantee the slot is not `Float` (shift / bitwise paths gate on that).
fn slot_to_bigint(s: &NumericSlot) -> PyBigInt {
    match s {
        NumericSlot::Int(n) => PyBigInt::from(*n),
        NumericSlot::BigInt(b) => b.clone(),
        NumericSlot::Float(_) => unreachable!("slot_to_bigint on Float"),
    }
}

/// Collapse a `BigInt` result back to a small `Int` when it fits, so that
/// `7 & 3` yields `int` rather than a BigInt-tagged value (CPython has no
/// such distinction, but pyrust's fast paths key off the `Int` tag).
fn collapse_bigint(v: Value) -> Value {
    if let ValueKind::BigInt(b) = v.kind()
        && let Some(n) = b.to_i64()
    {
        return Value::int(n);
    }
    v
}

/// `ZeroDivisionError` with the given message.
fn zero_div(msg: &str) -> PyError {
    pyrust_core::zerodiv_err!(msg.to_string())
}

/// Canonical `<<` for two integer slots (Int / BigInt), promoting to
/// BigInt and collapsing back to Int when the result fits.  Mirrors the
/// CPython overflow / saturation behaviour from `eval_binary`.
fn slot_lshift(lhs: &NumericSlot, rhs: &Value) -> Result<Value> {
    let a = slot_to_bigint(lhs);
    match shift_count(rhs)? {
        ShiftCount::Fits(n) => {
            if n > MAX_SHIFT && !a.is_zero() {
                return Err(pyrust_core::overflow_err!("too many digits in integer"));
            }
            Ok(collapse_bigint(Value::bigint(a << n)))
        }
        ShiftCount::Saturated => {
            if a.is_zero() {
                Ok(Value::int(0))
            } else {
                Err(pyrust_core::overflow_err!("too many digits in integer"))
            }
        }
    }
}

/// Canonical `>>` for two integer slots.  Right shift never overflows;
/// a count larger than the value's bit length collapses to the sign.
fn slot_rshift(lhs: &NumericSlot, rhs: &Value) -> Result<Value> {
    let a = slot_to_bigint(lhs);
    match shift_count(rhs)? {
        ShiftCount::Fits(n) => Ok(collapse_bigint(Value::bigint(a >> n))),
        ShiftCount::Saturated => Ok(Value::int(if a < PyBigInt::from(0) { -1 } else { 0 })),
    }
}

/// Build the CPython operand-type `TypeError` for a binary numeric op when
/// at least one operand is non-numeric:
/// `unsupported operand type(s) for OP: 'lt' and 'rt'`.  `op_sym` is the
/// operator token CPython uses in the message (e.g. `/`, `//`, `%`,
/// `** or pow()`).
fn unsupported_operand(op_sym: &str, left: &Value, right: &Value) -> PyError {
    let lt = value_type_name_str(left);
    let rt = value_type_name_str(right);
    pyrust_core::type_err!("unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'")
}

/// The plain and augmented operator tokens an operand-type `TypeError` should
/// carry for `op`, or `None` for operators that never appear in an augmented
/// assignment (comparisons, `in`, `is`, …).
///
/// `base` is the token CPython embeds in the *plain* binary message (note `**`
/// uses `** or pow()`); `aug` is the in-place form (`+=`, `**=`, …) CPython
/// uses for `a op= b`.  See issue #2561.
fn aug_operand_symbols(op: BinaryOp) -> Option<(&'static str, &'static str)> {
    Some(match op {
        BinaryOp::Add => ("+", "+="),
        BinaryOp::Sub => ("-", "-="),
        BinaryOp::Mul => ("*", "*="),
        BinaryOp::MatMul => ("@", "@="),
        BinaryOp::Div => ("/", "/="),
        BinaryOp::FloorDiv => ("//", "//="),
        BinaryOp::Mod => ("%", "%="),
        BinaryOp::Pow => ("** or pow()", "**="),
        BinaryOp::BitAnd => ("&", "&="),
        BinaryOp::BitOr => ("|", "|="),
        BinaryOp::BitXor => ("^", "^="),
        BinaryOp::LShift => ("<<", "<<="),
        BinaryOp::RShift => (">>", ">>="),
        _ => return None,
    })
}

/// Rewrite the operand-type `TypeError` produced by the plain `eval_binary`
/// path so it carries the augmented operator symbol (issue #2561).
///
/// Only the exact `unsupported operand type(s) for {base}: ` prefix is
/// rewritten — the sequence-specific messages (`can only concatenate …`,
/// `'int' object is not iterable`) are left verbatim, matching CPython, which
/// does not append `=` to those.
fn rewrite_aug_operand_error(op: BinaryOp, err: PyError) -> PyError {
    let Some((base, aug)) = aug_operand_symbols(op) else {
        return err;
    };
    let PyError::Named(class, msg) = &err else {
        return err;
    };
    if class.as_ref() != "TypeError" {
        return err;
    }
    let prefix = format!("unsupported operand type(s) for {base}: ");
    let Some(rest) = msg.strip_prefix(&prefix) else {
        return err;
    };
    pyrust_core::type_err!("unsupported operand type(s) for {aug}: {rest}")
}

/// Dispatch a numeric binary op through the LHS's slot table.  Returns
/// `Some(result)` when both operands are numeric (the canonical numeric
/// arithmetic applies), or `None` (CPython `NotImplemented`) when either
/// operand is non-numeric — the caller then handles sequence repetition,
/// container concatenation, and `TypeError`.
///
/// Both operands are classified once; the slot impl handles every numeric
/// type pair, so this is the single canonical site for `Add` / `Sub` /
/// `Mul` numeric arithmetic.  Adding a numeric type means extending
/// `NumericSlot` / `numeric_slot` and the slot `match` arms in one place.
pub(crate) fn dispatch_numeric_binop(
    op: BinaryOp,
    lhs: &Value,
    rhs: &Value,
) -> Option<Result<Value>> {
    let l = numeric_slot(lhs)?;
    match op {
        BinaryOp::Add => l.nb_add(rhs),
        BinaryOp::Sub => l.nb_sub(rhs),
        BinaryOp::Mul => l.nb_mul(rhs),
        BinaryOp::Div => l.nb_div(rhs),
        BinaryOp::FloorDiv => l.nb_floordiv(rhs),
        BinaryOp::Mod => l.nb_mod(rhs),
        BinaryOp::Pow => l.nb_pow(rhs),
        BinaryOp::LShift => l.nb_lshift(rhs),
        BinaryOp::RShift => l.nb_rshift(rhs),
        BinaryOp::BitAnd => l.nb_and(rhs),
        BinaryOp::BitOr => l.nb_or(rhs),
        BinaryOp::BitXor => l.nb_xor(rhs),
        _ => None,
    }
}
