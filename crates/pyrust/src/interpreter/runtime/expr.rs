/// Fallible `BigInt -> f64`.  Raises `OverflowError` (CPython parity:
/// `int too large to convert to float`) when the BigInt's magnitude is
/// outside f64's representable range, instead of silently returning
/// `f64::INFINITY` (which loses sign and produces nonsense `inf`
/// arithmetic).  Centralised here for the mixed BigInt±Float arms in
/// add/sub/mul (PR #484 Copilot review).
fn bigint_to_float_or_overflow(b: &PyBigInt) -> Result<f64> {
    b.to_f64()
        .filter(|f| f.is_finite())
        .ok_or_else(|| {
            pyrust_core::overflow_err!("int too large to convert to float")
        })
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
        if char_count.checked_mul(n).is_none_or(|t| t > isize::MAX as usize) {
            return Err(pyrust_core::overflow_err!("repeated string is too long"));
        }
        // char_count * n fits, but byte_total doesn't — OOM.
        return Err(pyrust_core::py_err!("MemoryError", String::new()));
    }
    // Use try_reserve to catch OOM rather than letting the allocator abort,
    // then delegate to str::repeat for its O(log n) doubling strategy.
    let mut probe = String::new();
    if probe.try_reserve(byte_total).is_err() {
        return Err(pyrust_core::py_err!("MemoryError", String::new()));
    }
    Ok(Value::string(text.repeat(n)))
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

/// Outcome of the borrow-only sequence equality fast path used by
/// `values_user_eq` for `List`/`Tuple` pairs.
///
/// `Resolved(v)` means every element comparison was settled by
/// `Value::eq` without needing user `__eq__` dispatch, so `v` is the
/// final answer.  `NeedsDispatch` means at least one element pair could
/// only be resolved by recursing into `values_user_eq` (e.g. it
/// contains a `PyInstance` or a nested container that itself may hold
/// one); the caller must drop the borrow, snapshot the elements, and
/// take the slow recursion path.
enum SeqFast {
    Resolved(bool),
    NeedsDispatch,
}

/// Returns `true` iff comparing `a` against `b` via `Value::eq` could
/// give a different answer than user `__eq__` dispatch would.  Used to
/// keep `[1,2,3] == [1,2,4]` (flat primitive sequences) on a
/// zero-allocation walk.
///
/// Conservative: any `PyInstance`, container (`List`/`Tuple`/`Dict`/
/// `Set`), or `BuiltinObject` (e.g. `frozenset`) returns `true`,
/// because each may itself contain a `PyInstance` for which `Value::eq`
/// would fall back to `Rc::ptr_eq`.  Leaf primitives
/// (`Int`/`Float`/`Bool`/`Str`/`Bytes`/`None`/`Complex`/`BigInt`/
/// `Range`) return `false`.
fn pair_may_need_dispatch(a: &Value, b: &Value) -> bool {
    matches!(
        a.kind(),
        ValueKind::PyInstance(_)
            | ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::BuiltinObject { .. }
    ) || matches!(
        b.kind(),
        ValueKind::PyInstance(_)
            | ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::BuiltinObject { .. }
    )
}

/// `true` when `v` is a `PyInstance` whose class defines its own `__ne__`
/// (i.e. the resolved `__ne__` is *not* the inherited `object.__ne__`).
///
/// Used by `BinaryOp::Ne` (issue #2645) to decide whether to dispatch a
/// user-defined `__ne__` or to fall back to negating `__eq__`.  Built-in
/// `__ne__` slots inherited from `object` resolve to
/// `BuiltinFunction("object.__ne__")` (see `lookup_class_attr`'s object
/// fallback), which only compares identity — exactly the slot CPython
/// replaces with `not __eq__`.
fn pyinstance_has_user_ne(v: &Value) -> bool {
    let ValueKind::PyInstance(inst) = v.kind() else {
        return false;
    };
    let class = Rc::clone(&inst.borrow().class);
    match lookup_class_attr(&class, "__ne__").as_ref().map(|m| m.kind()) {
        Some(ValueKind::BuiltinFunction("object.__ne__")) | None => false,
        Some(_) => true,
    }
}

/// Element-wise equality over two equal-length slices, returning
/// [`SeqFast::Resolved`] as soon as every pair can be decided by
/// `Value::eq` alone.  As soon as a pair that could need user dispatch
/// is encountered (and didn't already compare equal), bail to
/// [`SeqFast::NeedsDispatch`] so the caller can take the snapshot+
/// recurse path.
fn try_seq_fast_eq(av: &[Value], bv: &[Value]) -> SeqFast {
    debug_assert_eq!(av.len(), bv.len());
    for (x, y) in av.iter().zip(bv.iter()) {
        // CPython compares list/tuple elements with `PyObject_RichCompareBool`,
        // which short-circuits on identity before `__eq__`; a NaN at the same
        // index in both sequences (e.g. `[n] == [n]`) is therefore equal even
        // though `n == n` is False.
        if x == y || x.is_identical_nan(y) {
            continue;
        }
        if pair_may_need_dispatch(x, y) {
            return SeqFast::NeedsDispatch;
        }
        return SeqFast::Resolved(false);
    }
    SeqFast::Resolved(true)
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
    let total = items.len().checked_mul(n).filter(|&t| t <= isize::MAX as usize);
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
                    if let Some(q) = a.checked_sub(modulo).and_then(|a_adj| a_adj.checked_div(*b)) {
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
                    _ => Err(pyrust_core::overflow_err!("exponent too large for ** to compute")),
                }
            }
            (NumericSlot::BigInt(a), NumericSlot::BigInt(b)) if *b >= PyBigInt::from(0) => {
                match b.to_u64_digits().1.as_slice() {
                    [exp] => Ok(Value::bigint(PyPow::pow(a.clone(), *exp))),
                    [] => Ok(Value::int(1)),
                    _ => Err(pyrust_core::overflow_err!("exponent too large for ** to compute")),
                }
            }
            // Float base/exponent, or integer with a negative exponent:
            // float exponentiation (with CPython's negative-real → complex
            // promotion and the 0.0 ** negative ZeroDivisionError).
            _ => (|| {
                let a = slot_to_float(self)?;
                let b = slot_to_float(&r)?;
                if a == 0.0 && b < 0.0 && b.is_finite() {
                    return Err(pyrust_core::zerodiv_err!("0.0 cannot be raised to a negative power"));
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
        && let Some(n) = b.to_i64() {
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

/// Hash-bucket probe for collecting `PyKey::Object` / `PyKey::None` lookup
/// candidates in O(bucket) instead of O(n) (issue #2060).
///
/// `PyKey::Object` keys can never resolve via `IndexMap::get` (their `PartialEq`
/// is `Rc::ptr_eq`), so a distinct-but-`__eq__`-equal key always misses the
/// fast path.  The old slow path then linear-scanned *every* entry to find
/// same-hash candidates — O(n) per access, O(n²) to build a dict/set keyed on
/// custom objects.
///
/// Instead, this probe drives `IndexMap`/`IndexSet::get_index_of`, which hashes
/// the probe (placing it in the matching entries' bucket) and calls
/// [`Equivalent::equivalent`] for each entry sharing that bucket's hash.  The
/// probe records every entry its `is_match` predicate accepts as a side effect
/// and always reports "not equivalent", so the walk covers the whole collision
/// chain and returns `None`; the collected Vec then holds the candidates, which
/// the caller confirms via user `__eq__`.  Only bucket entries are visited.
///
/// `probe_key` drives the hash: `PyKey::Object { hash, .. }` and (for the None
/// cross-variant case, issue #906) `PyKey::None` both hash on a Python-level
/// hash value, so matching entries collide into the same bucket.
struct ObjectBucketProbe<'a, F: Fn(&PyKey) -> bool> {
    probe_key: &'a PyKey,
    is_match: F,
    collected: std::cell::RefCell<Vec<PyKey>>,
}

impl<F: Fn(&PyKey) -> bool> std::hash::Hash for ObjectBucketProbe<'_, F> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.probe_key.hash(state);
    }
}

impl<F: Fn(&PyKey) -> bool> indexmap::Equivalent<PyKey> for ObjectBucketProbe<'_, F> {
    fn equivalent(&self, key: &PyKey) -> bool {
        if (self.is_match)(key) {
            self.collected.borrow_mut().push(key.clone());
        }
        // Never report equality: force the bucket walk to continue so we see
        // every collision, and dispatch the real user `__eq__` afterwards.
        false
    }
}

/// Collect the candidate keys in a dict's hash bucket that `is_match` accepts,
/// for later user-`__eq__` dispatch.  Returns each candidate's cloned key (an
/// O(1) RC bump for `Object`/`None`); the caller recovers the entry on a hit
/// via `get_full` (one O(bucket) probe).  See [`ObjectBucketProbe`].
fn collect_object_bucket_keys_map(
    dict: &PyDict,
    probe_key: &PyKey,
    is_match: impl Fn(&PyKey) -> bool,
) -> Vec<PyKey> {
    let probe = ObjectBucketProbe {
        probe_key,
        is_match,
        collected: std::cell::RefCell::new(Vec::new()),
    };
    let _ = dict.get_index_of(&probe);
    probe.collected.into_inner()
}

/// `IndexSet` counterpart of [`collect_object_bucket_keys_map`].
fn collect_object_bucket_keys_set(
    set: &PySet,
    probe_key: &PyKey,
    is_match: impl Fn(&PyKey) -> bool,
) -> Vec<PyKey> {
    let probe = ObjectBucketProbe {
        probe_key,
        is_match,
        collected: std::cell::RefCell::new(Vec::new()),
    };
    let _ = set.get_index_of(&probe);
    probe.collected.into_inner()
}

/// Extract the Python-level `Value` from an `Object`/`None` candidate key for
/// dispatching user `__eq__`.  Bucket candidates are always `Object` or `None`.
fn pykey_object_or_none_value(key: &PyKey) -> Value {
    match key {
        PyKey::Object { value, .. } => value.clone(),
        _ => Value::none(),
    }
}

/// True if `key` is — or transitively contains — a `PyKey::Object` (a user
/// instance whose equality requires `__eq__` dispatch).  Recurses into
/// `Tuple` / `FrozenSet` keys so a nested object inside a tuple key is found
/// (issue #2059).  Primitive keys (`Int`, `Str`, …) and tuples of primitives
/// return `false` and stay on the raw `IndexSet`/`IndexMap` fast path.
fn key_contains_object(key: &PyKey) -> bool {
    match key {
        PyKey::Object { .. } => true,
        PyKey::Tuple(items) | PyKey::FrozenSet(items) => items.iter().any(key_contains_object),
        _ => false,
    }
}

/// Returns `true` when `key` is a `Tuple` / `FrozenSet` key that nests a
/// user-object element (so the raw identity-based `PyKey` equality misses an
/// `__eq__`-equal-but-distinct match).  A bare top-level `PyKey::Object` is
/// *not* included here — that case is already handled by the dedicated
/// `Object`-key slow paths.  Issue #2059.
fn nested_object_tuple_key(key: &PyKey) -> bool {
    matches!(key, PyKey::Tuple(_) | PyKey::FrozenSet(_)) && key_contains_object(key)
}

impl Interpreter {
    pub(crate) fn eval_index(&mut self, target: &Value, index: Value) -> Result<Value> {
        // If the index is a `slice` object (built by `eval_slice` and passed
        // into a `__getitem__` call, which then subscripts a built-in sequence
        // with it), extract the bounds and delegate to `eval_slice` so that
        // `self.data[slice_arg]` inside a `__getitem__` works correctly.
        //
        // Dicts and BuiltinObjects are excluded: they may accept slice objects
        // as legitimate hashable keys (e.g. `d = {}; d[slice(1,3)] = "a"`).
        // Only sequence-like targets (List, Tuple, Str, Bytes, PyInstance) need
        // the redirect.
        // Slice-object subscript redirect.  Probe the *index* first: for the
        // hot integer-index path the BuiltinObject match misses immediately and
        // no target type_name() probe runs.  Only when the index is actually a
        // slice object do we consult the target type (#1908 adds bytearray to
        // the sequence-like set; bytearray slices are always slice ops, never
        // hashable keys, so the redirect is safe).
        if let ValueKind::BuiltinObject { ops, state } = index.kind()
            && ops.type_name() == pyrust_builtins::slice::TYPE_NAME
        {
            let target_is_sequence_like = matches!(
                target.kind(),
                ValueKind::List(_)
                    | ValueKind::Tuple(_)
                    | ValueKind::Str(_)
                    | ValueKind::Bytes(_)
                    | ValueKind::PyInstance(_)
                    // Issue #2399: `range(5).__getitem__(slice(1, None))` reaches
                    // `eval_index` with a slice *object* (not a slice expression),
                    // so it must redirect to `eval_slice` — which already handles
                    // `Range`/`BigRange` arithmetically — exactly as `range(5)[1:]`
                    // does.  Without this, the slot-dunder form raised "range
                    // indices must be integers or slices, not slice".
                    | ValueKind::Range { .. }
                    | ValueKind::BigRange { .. }
            ) || matches!(
                target.kind(),
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME
            );
            if target_is_sequence_like {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<pyrust_builtins::slice::SliceState>()
                    .expect("SliceOps: bad state");
                let lo = if s.start.is_none() { None } else { Some(s.start.clone()) };
                let hi = if s.stop.is_none() { None } else { Some(s.stop.clone()) };
                let st = if s.step.is_none() { None } else { Some(s.step.clone()) };
                drop(borrow);
                return self.eval_slice(target, lo, hi, st);
            }
        }
        // Handle Dict separately so the temporary `&IndexMap` from
        // `target.kind()` doesn't outlive the call into `dict_lookup`
        // (which may run user `__eq__` that mutates the dict — see the
        // aliasing notes on `Value::as_dict_mut`).
        if target.as_dict().is_some() {
            // Fast path for string keys (issue #506): probe via `StrKey` to
            // skip constructing a `PyKey::Str(Value)` (which bumps the RC).
            let lookup = if let Some(s) = index.as_str() {
                self.dict_str_lookup(target, s)?
            } else {
                let key = self.value_to_pykey(&index)?;
                self.dict_lookup(target, &key)?
            };
            return match lookup {
                Some((_, v)) => Ok(v),
                None => Err(PyError::key_error(index)),
            };
        }
        // Resolve the __index__ protocol for sequence targets before the borrow
        // from target.kind() is held across the match arms (which call &mut self
        // helpers that cannot coexist with an active kind() borrow).
        let seq_label: Option<&'static str> = match target.kind() {
            ValueKind::List(_) => Some("list"),
            ValueKind::Tuple(_) => Some("tuple"),
            ValueKind::Str(_) => Some("string"),
            ValueKind::Bytes(_) => Some("bytes"),
            ValueKind::Range { .. } => Some("range"),
            _ => None,
        };
        let index = if let Some(label) = seq_label {
            self.call_index_protocol(&index, label)?
        } else {
            index
        };
        match target.kind() {
            ValueKind::List(items) => {
                let idx = normalize_index(&index, items.len(), "list")?;
                Ok(items[idx].clone())
            }
            ValueKind::Tuple(items) => {
                let idx = normalize_index(&index, items.len(), "tuple")?;
                Ok(items[idx].clone())
            }
            ValueKind::Str(text) => {
                // ASCII fast path (#2032 / #2116 / #2136): when every byte is
                // ASCII, char index == byte index, so length is `text.len()` and
                // the i-th char is a single byte — O(1) index instead of an
                // O(idx) char scan.  ASCII-ness is cached on the string header
                // (#2124), so the check is O(1) — no per-op rescan, no penalty
                // for non-ASCII strings.  The fast-path body lives in
                // `fast_path.rs::fast_str_ascii_index`.
                if target.str_is_ascii() {
                    return fast_str_ascii_index(text, &index);
                }
                let char_count = text.chars().count();
                let idx = normalize_index(&index, char_count, "string")?;
                // Use nth() to avoid collecting a Vec<char>; normalize_index
                // guarantees idx < char_count so unwrap is safe.
                let ch = text.chars().nth(idx).expect("normalize_index bounds check");
                // Stack-encode to a &str to avoid an intermediate String allocation.
                let mut buf = [0u8; 4];
                Ok(Value::string(ch.encode_utf8(&mut buf) as &str))
            }
            ValueKind::Bytes(rc) => {
                let idx = normalize_index(&index, rc.len(), "bytes")?;
                Ok(Value::int(rc[idx] as i64))
            }
            ValueKind::Range { start, stop, step } => {
                let len = range_len(start, stop, step);
                // call_index_protocol (via seq_label) has already resolved any
                // __index__ on the subscript; the value is now Int/Bool/BigInt.
                // Cannot use normalize_index because its error message is
                // "range index out of range", but CPython says
                // "range object index out of range".
                let mut i = match index.kind() {
                    ValueKind::Int(v) => v,
                    ValueKind::Bool(b) => b as i64,
                    // BigInt is a valid integer but will always be out of range
                    // for any realistic range length.
                    ValueKind::BigInt(_) => {
                        return Err(pyrust_core::index_err!("range object index out of range"));
                    }
                    _ => unreachable!("call_index_protocol guarantees an integer"),
                };
                if i < 0 {
                    i += len;
                }
                if i < 0 || i >= len {
                    return Err(pyrust_core::index_err!("range object index out of range"));
                }
                Ok(Value::int(start + i * step))
            }
            ValueKind::BigRange { start, stop, step } => {
                // Arbitrary-precision range indexing (#2118).  `call_index_protocol`
                // already resolved any `__index__`, so the subscript is an int-like
                // value; widen it to BigInt for the negative-wrap + bounds check.
                let len = pyrust_core::bigrange_len(start, stop, step);
                let mut i = value_to_bigint(&index)
                    .expect("call_index_protocol guarantees an integer");
                if i.sign() == pyrust_core::PyBigIntSign::Minus {
                    i += &len;
                }
                if i.sign() == pyrust_core::PyBigIntSign::Minus || i >= len {
                    return Err(pyrust_core::index_err!("range object index out of range"));
                }
                Ok(value_from_bigint(start + i * step))
            }
            ValueKind::Dict(_) => unreachable!("handled above"),
            ValueKind::BuiltinObject { ops, state } => {
                // Built-in object types opt in to subscripting via
                // `BuiltinTypeOps::get_item`.  The default impl returns a
                // TypeError shaped like the legacy "object is not
                // subscriptable" message, so non-subscriptable types
                // don't need per-type plumbing.  bytearray's __index__ subscript
                // resolution is handled by callers (exec_get_item and the slice
                // redirect above) so the int-index hot path stays untouched.
                ops.get_item(state, &index)
            }
            ValueKind::PyClass(class_rc) => {
                let class = Rc::clone(class_rc);
                // PEP 585: `type[int]` → `types.GenericAlias`.  CPython does NOT
                // expose `__class_getitem__` as an attribute on `type`, so the
                // subscript is special-cased here by pointer-identity rather than
                // via the sentinel-attribute path used by `list`/`dict`/…
                // (`hasattr(type, '__class_getitem__')` stays False and
                // `type.__class_getitem__(int)` raises AttributeError).
                if Rc::ptr_eq(&class, &type_class_singleton()) {
                    let index_is_tuple = matches!(index.kind(), ValueKind::Tuple(_));
                    let type_args = if index_is_tuple {
                        index
                    } else {
                        Value::tuple(vec![index])
                    };
                    return Ok(pyrust_builtins::generic_alias::generic_alias(
                        Value::py_class(class),
                        type_args,
                    ));
                }
                // A metaclass `__getitem__` (e.g. `EnumMeta.__getitem__`, which
                // implements `Color['RED']` name lookup) is a type-level slot
                // that takes precedence over the class's own
                // `__class_getitem__` (#2611).
                if let Some(getitem_fn) = metaclass_dunder(&class, "__getitem__") {
                    return invoke_class_method(
                        self,
                        getitem_fn,
                        Value::py_class(Rc::clone(&class)),
                        &[ExpandedCallArg {
                            name: None,
                            value: index,
                        }],
                    );
                }
                // Look up `__class_getitem__` along the MRO (issue #2698).
                // Built-in collection types have a
                // `BuiltinFunction("<type>.__class_getitem__")` sentinel
                // registered by `build_primitive_classes`.  User-defined
                // classes may define it as a classmethod, or *inherit* one —
                // e.g. `class Stack(Generic[T])` inherits
                // `Generic.__class_getitem__`, and `class Sub(Base)` inherits a
                // user-defined `Base.__class_getitem__`.  Walking the MRO (not
                // just the class's own dict) is what makes those subscriptable.
                // Classes without it anywhere in the MRO raise TypeError
                // (matching CPython 3.12).
                let cgitem = lookup_class_attr(&class, "__class_getitem__");
                if let Some(method_val) = cgitem {
                    // Distinguish between the built-in sentinel and a
                    // user-defined classmethod, and pick out the `Union` /
                    // `Optional` special forms that need per-form normalisation
                    // (issue #2524).
                    enum Sentinel {
                        Generic,
                        Union,
                        Optional,
                        None,
                    }
                    let sentinel = match method_val.kind() {
                        ValueKind::BuiltinFunction("typing.Union.__class_getitem__") => {
                            Sentinel::Union
                        }
                        ValueKind::BuiltinFunction("typing.Optional.__class_getitem__") => {
                            Sentinel::Optional
                        }
                        ValueKind::BuiltinFunction(name)
                            if name.contains(".__class_getitem__") =>
                        {
                            Sentinel::Generic
                        }
                        _ => Sentinel::None,
                    };
                    if !matches!(sentinel, Sentinel::None) {
                        // `Union`/`Optional` flatten nested unions, lower `None`,
                        // de-dup, and collapse singletons before the alias is
                        // built.  The typing module owns those semantics.
                        match sentinel {
                            Sentinel::Union => {
                                return crate::builtin_modules::typing::union_or_optional_getitem(
                                    "Union", index,
                                );
                            }
                            Sentinel::Optional => {
                                return crate::builtin_modules::typing::union_or_optional_getitem(
                                    "Optional", index,
                                );
                            }
                            _ => {}
                        }
                        // Built-in sentinel: create a `GenericAlias` directly.
                        // Normalise the subscript into a tuple for
                        // `GenericAlias.__args__`:
                        //   `list[int]`       → args = (int,)
                        //   `dict[str, int]`  → args = (str, int) [tuple index]
                        let is_tuple = matches!(index.kind(), ValueKind::Tuple(_));
                        let type_args = if is_tuple {
                            index
                        } else {
                            Value::tuple(vec![index])
                        };
                        Ok(pyrust_builtins::generic_alias::generic_alias(
                            Value::py_class(class),
                            type_args,
                        ))
                    } else {
                        // User-defined `__class_getitem__` (typically a
                        // classmethod): call it with the class as the
                        // implicit receiver and the subscript as the arg.
                        let class_val = Value::py_class(class);
                        invoke_class_method(
                            self,
                            method_val,
                            class_val,
                            &[ExpandedCallArg {
                                name: None,
                                value: index,
                            }],
                        )
                    }
                } else if class
                    .borrow()
                    .attrs
                    .get("__type_params__")
                    .is_some_and(|tp| matches!(tp.kind(), ValueKind::Tuple(items) if !items.is_empty()))
                {
                    // PEP 695 generic class (`class C[T]: ...`): CPython gives it
                    // an implicit `__class_getitem__` that returns a generic
                    // alias, so `C[int]` is subscriptable and `C[int]()`
                    // constructs an instance.  We detect the generic class via a
                    // non-empty `__type_params__` tuple and build the alias
                    // directly, mirroring the built-in-collection path above.
                    let index_is_tuple = matches!(index.kind(), ValueKind::Tuple(_));
                    let type_args = if index_is_tuple {
                        index
                    } else {
                        Value::tuple(vec![index])
                    };
                    Ok(pyrust_builtins::generic_alias::generic_alias(
                        Value::py_class(class),
                        type_args,
                    ))
                } else {
                    Err(pyrust_core::type_err!("type '{}' is not subscriptable", class.borrow().name))
                }
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                // PEP 695: a generic `type X[T] = ...` alias is subscriptable —
                // `Pair[int]` returns a `types.GenericAlias` with the alias as
                // origin (CPython 3.12 reprs it `Pair[int]`, not the substituted
                // value).  A non-generic alias raises CPython's specific
                // "Only generic type aliases are subscriptable" (issue #2779).
                if is_type_alias_class(&class) {
                    let has_params = inst_rc
                        .borrow()
                        .attrs
                        .get("__type_params__")
                        .is_some_and(|p| matches!(p.kind(), ValueKind::Tuple(t) if !t.is_empty()));
                    if !has_params {
                        return Err(pyrust_core::type_err!(
                            "Only generic type aliases are subscriptable"
                        ));
                    }
                    let type_args = if matches!(index.kind(), ValueKind::Tuple(_)) {
                        index
                    } else {
                        Value::tuple(vec![index])
                    };
                    return Ok(pyrust_builtins::generic_alias::generic_alias(
                        Value::py_instance(inst_rc),
                        type_args,
                    ));
                }
                // Issue #1134: check for a user-defined __getitem__ on the
                // class *before* falling back to the backing primitive fast
                // path.  A dict subclass that overrides __getitem__ must have
                // the override called, not the raw backing-dict lookup.
                // The BuiltinFunction sentinel `dict.__getitem__` registered
                // on the dict base class itself is excluded — it is the base
                // implementation, not an override.  Any other __getitem__
                // (UserFunction from user code, or BuiltinFunction from a
                // builtin class like Counter) is treated as an override.
                let user_getitem = lookup_class_attr(&class, "__getitem__").filter(|v| {
                    !matches!(
                        v.kind(),
                        ValueKind::BuiltinFunction(
                            "dict.__getitem__"
                                | "list.__getitem__"
                                | "tuple.__getitem__"
                                | "bytes.__getitem__"
                        )
                    )
                });
                if let Some(method_val) = user_getitem {
                    return invoke_class_method(
                        self,
                        method_val,
                        Value::py_instance(inst_rc),
                        &[ExpandedCallArg {
                            name: None,
                            value: index,
                        }],
                    );
                }
                // No user __getitem__: delegate to the backing primitive when
                // present.  For dict backing, also honour __missing__ on a
                // missing key (issue #1134).
                if let Some(backing) = builtin_data_backing(target) {
                    if backing.as_dict().is_some() {
                        let lookup = if let Some(s) = index.as_str() {
                            self.dict_str_lookup(&backing, s)?
                        } else {
                            let key = self.value_to_pykey(&index)?;
                            self.dict_lookup(&backing, &key)?
                        };
                        return match lookup {
                            Some((_, v)) => Ok(v),
                            None => {
                                if let Some(missing_fn) =
                                    lookup_class_attr(&class, "__missing__")
                                {
                                    invoke_class_method(
                                        self,
                                        missing_fn,
                                        Value::py_instance(inst_rc),
                                        &[ExpandedCallArg {
                                            name: None,
                                            value: index,
                                        }],
                                    )
                                } else {
                                    Err(PyError::key_error(index))
                                }
                            }
                        };
                    }
                    return self.eval_index(&backing, index);
                }
                Err(pyrust_core::type_err!("'{}' object is not subscriptable", class.borrow().name))
            }
            _ => Err(pyrust_core::type_err!("'{}' object is not subscriptable",
                    pyrust_core::builtin_type_name(target))),
        }
    }

    /// Try to call a binary dunder method on `left` (named `method`), then on
    /// `right` (named `rmethod`).  Returns `Some(result)` if a dunder was found
    /// and called, or `None` if neither operand has the method.
    ///
    /// Routes both `UserFunction` (pure-Python class methods) and
    /// `BuiltinFunction` (methods defined via `pyrust_module!`'s
    /// `class { … }` block, e.g. `Counter.__add__`) through
    /// `invoke_class_method` so operator-overloading works for both
    /// kinds of class — issue #331.
    fn try_dunder_binary(
        &mut self,
        left: &Value,
        right: &Value,
        method: &str,
        rmethod: &str,
    ) -> Option<Result<Value>> {
        // Subtype priority (mirrors CPython `binary_op1`): when `rmethod` is a
        // reflected arithmetic slot (e.g. `__radd__`, starts with `__r`) and
        // `right`'s class is a *proper* subtype of `left`'s class AND `right`'s
        // resolved `rmethod` slot (via MRO) differs from `left`'s resolved slot
        // (one is None, or they're different functions), try
        // `right.rmethod(left)` before `left.method(right)`.  This mirrors
        // CPython's `slotw != slotv` check in `binary_op1`: a right type that
        // inherits a different `__radd__` from an intermediate class gets
        // priority, not only types that directly define `rmethod` in their own
        // `__dict__`.  Comparison reflected ops (`__gt__`, `__ge__`, …) do not
        // start with `__r`, so they are unaffected by this check.
        let right_has_subtype_priority = rmethod.starts_with("__r") && {
            if let (ValueKind::PyInstance(li), ValueKind::PyInstance(ri)) =
                (left.kind(), right.kind())
            {
                let lc = Rc::clone(&li.borrow().class);
                let rc_class = Rc::clone(&ri.borrow().class);
                if !Rc::ptr_eq(&lc, &rc_class) && class_is_subclass_of(&rc_class, &lc) {
                    let right_slot = lookup_class_attr(&rc_class, rmethod);
                    let left_slot = lookup_class_attr(&lc, rmethod);
                    right_slot.is_some() && right_slot != left_slot
                } else {
                    false
                }
            } else {
                false
            }
        };

        if right_has_subtype_priority
            && let ValueKind::PyInstance(inst) = right.kind() {
                let class = Rc::clone(&inst.borrow().class);
                if let Some(m) = lookup_class_attr(&class, rmethod) {
                    match self.dispatch_binary_slot(m, right, inst, left) {
                        Some(Ok(v)) if is_not_implemented(&v) => {}
                        Some(result) => return Some(result),
                        None => {}
                    }
                }
            }

        if let ValueKind::PyInstance(inst) = left.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, method) {
                match self.dispatch_binary_slot(m, left, inst, right) {
                    Some(Ok(v)) if is_not_implemented(&v) => {}
                    Some(result) => return Some(result),
                    None => {}
                }
            }
        }

        // Same-type skip (mirrors CPython `binary_op1`): when the forward slot
        // already ran and returned NotImplemented, and both operands are the
        // *same* type, CPython sets the reflected slot `slotw` to NULL because
        // it would be identical to the forward slot already tried — so the
        // reflected method is not called and the op falls through to TypeError.
        // Scoped to reflected arithmetic slots (`__r*`): comparison reflected
        // ops (`__gt__`, `__ge__`, …) do not start with `__r`, and CPython
        // *does* try both sides for same-type comparisons, so they must stay
        // unaffected.  See issue #2092.
        let same_type_reflected_arith = rmethod.starts_with("__r")
            && matches!(
                (left.kind(), right.kind()),
                (ValueKind::PyInstance(li), ValueKind::PyInstance(ri))
                    if Rc::ptr_eq(&li.borrow().class, &ri.borrow().class)
            );
        if !right_has_subtype_priority && !same_type_reflected_arith
            && let ValueKind::PyInstance(inst) = right.kind() {
                let class = Rc::clone(&inst.borrow().class);
                if let Some(m) = lookup_class_attr(&class, rmethod) {
                    match self.dispatch_binary_slot(m, right, inst, left) {
                        Some(Ok(v)) if is_not_implemented(&v) => {}
                        Some(result) => return Some(result),
                        None => {}
                    }
                }
            }
        None
    }

    /// Evaluate one operand's `__ne__` step in CPython's `do_richcompare(Py_NE)`
    /// for a `PyInstance` `owner` against `other` (issue #2648).
    ///
    /// Returns:
    /// - `Some(Ok(v))` — the step produced a definitive (non-NotImplemented)
    ///   result `v` (the caller returns it);
    /// - `Some(Err(..))` — the slot raised;
    /// - `None` — the step yielded NotImplemented, so the caller should try the
    ///   next step (reflected operand, then identity).
    ///
    /// A *user-defined* `__ne__` is dispatched directly.  The inherited default
    /// (`object.__ne__`) is `not owner.__eq__(other)` *single-sided*: it negates
    /// only `owner`'s own `__eq__` (not the full `==`, which would also try the
    /// reflected `__eq__`), and stays NotImplemented when that `__eq__` is
    /// NotImplemented.  Verified against python3.12 `object.__ne__`.
    fn pyinstance_ne_step(&mut self, owner: &Value, other: &Value) -> Option<Result<Value>> {
        let ValueKind::PyInstance(inst) = owner.kind() else {
            return None;
        };
        let class = Rc::clone(&inst.borrow().class);
        // User-defined `__ne__` wins outright.
        if !matches!(
            lookup_class_attr(&class, "__ne__").as_ref().map(|m| m.kind()),
            Some(ValueKind::BuiltinFunction("object.__ne__")) | None
        ) {
            let m = lookup_class_attr(&class, "__ne__")?;
            return match self.dispatch_binary_slot(m, owner, inst, other) {
                Some(Ok(v)) if is_not_implemented(&v) => None,
                other => other,
            };
        }
        // Default `object.__ne__`: negate `owner.__eq__(other)` single-sided.
        let eq = lookup_class_attr(&class, "__eq__")?;
        match self.dispatch_binary_slot(eq, owner, inst, other) {
            Some(Ok(v)) if is_not_implemented(&v) => None,
            Some(Ok(v)) => Some(Ok(Value::bool_(!v.truthy_raw()))),
            other => other,
        }
    }

    /// Dispatch a resolved binary-operator slot `m` (the value of e.g.
    /// `type(owner).__add__`) found on `owner` (a `PyInstance` backed by `inst`)
    /// with `other` as the single operand argument.
    ///
    /// Returns:
    /// - `Some(Ok(v))` when the slot was invoked (the result may be
    ///   `NotImplemented`, which the caller treats as "try the next slot");
    /// - `Some(Err(..))` when the slot raised, OR when the slot exists but is
    ///   *non-callable* (issue #2055: `__add__ = 5` → `TypeError: 'int' object
    ///   is not callable`);
    /// - `None` is never returned (the slot was already found by the caller);
    ///   it is kept in the signature only so callers read uniformly.
    fn dispatch_binary_slot(
        &mut self,
        m: Value,
        owner: &Value,
        inst: &Rc<RefCell<PyInstance>>,
        other: &Value,
    ) -> Option<Result<Value>> {
        if !slot_is_callable(&m) {
            return Some(Err(PyError::named(
                "TypeError",
                format!("'{}' object is not callable", value_type_name_str(&m)),
            )));
        }
        // BuiltinFunction dunders (e.g. `int.__radd__`) operate on the backing
        // primitive value.  Pass the coerced value so `eval_binary` inside the
        // dunder doesn't re-dispatch to the same method on the still-wrapped
        // PyInstance (infinite loop).
        let self_val = if matches!(m.kind(), ValueKind::BuiltinFunction(_)) {
            coerce_numeric(owner)
        } else {
            Value::py_instance(Rc::clone(inst))
        };
        let arg = ExpandedCallArg {
            name: None,
            value: other.clone(),
        };
        Some(invoke_class_method(self, m, self_val, &[arg]))
    }

    /// Try to call a unary dunder method on a PyInstance.  Routes both
    /// `UserFunction` and `BuiltinFunction` class methods through
    /// `invoke_class_method` — same parity with `try_dunder_binary`.
    pub(crate) fn try_dunder_unary(&mut self, val: &Value, method: &str) -> Option<Result<Value>> {
        if let ValueKind::PyInstance(inst) = val.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(m) = lookup_class_attr(&class, method) {
                // Issue #2055: a slot that exists but is non-callable
                // (`__neg__ = 5`) raises `TypeError: 'int' object is not
                // callable`, matching CPython, rather than silently skipping.
                if !slot_is_callable(&m) {
                    return Some(Err(PyError::named(
                        "TypeError",
                        format!("'{}' object is not callable", value_type_name_str(&m)),
                    )));
                }
                // BuiltinFunction dunders operate on the backing primitive value;
                // pass the coerced value so they don't reject the PyInstance wrapper.
                let self_val = if matches!(m.kind(), ValueKind::BuiltinFunction(_)) {
                    coerce_numeric(val)
                } else {
                    Value::py_instance(Rc::clone(inst))
                };
                return Some(invoke_class_method(self, m, self_val, &[]));
            }
        }
        None
    }

    /// Three-way ordering comparison that dispatches `__lt__` / `__gt__` on
    /// user instances before falling back to `compare_values` for primitives.
    ///
    /// Used by `sorted()` and `min()` — always tries `__lt__` first,
    /// matching CPython's `Py_LT`-primary reduction for those builtins.
    /// For `max()` call `richcmp_order_gt` instead, which mirrors CPython's
    /// `Py_GT`-primary reduction and emits `'>' not supported` on error.
    ///
    /// Algorithm:
    /// 1. If neither operand is a `PyInstance`, delegate to `compare_values`
    ///    (fast primitive path — zero overhead for the common int/str case).
    /// 2. Try `a < b` via `__lt__` on `a` / `__gt__` on `b`.  If truthy →
    ///    `Less`.
    /// 3. If step 2 returned falsy, try `b < a` to distinguish `Equal` from
    ///    `Greater`.
    /// 4. If no dunder was found, fall through to `compare_values` (which
    ///    raises `TypeError: '<' not supported …`, matching CPython `min` /
    ///    `sorted` behaviour).
    pub(crate) fn richcmp_order(
        &mut self,
        a: &Value,
        b: &Value,
    ) -> Result<std::cmp::Ordering> {
        use std::cmp::Ordering;

        // Fast path: neither operand is a user instance.
        if !matches!(a.kind(), ValueKind::PyInstance(_))
            && !matches!(b.kind(), ValueKind::PyInstance(_))
        {
            return compare_values(a, b);
        }

        // Issue #1934/#1939: a builtin-subclass operand (int/float/str/bytes/
        // list/tuple/… subclass) with no user comparison override inherits the
        // base type's ordering, so `min`/`max`/`sorted` must compare via the
        // backing value (`min(F(1.0), F(2.0))`, `sorted([L([2]), [1]])`).
        // Coerce each side to its backing when the subclass doesn't override
        // `__lt__`/`__gt__`/`__le__`/`__ge__`, then recurse so the primitive
        // fast path runs.  A genuine user comparison dunder is left intact and
        // dispatched below.
        const ORDER_OVERRIDES: &[&str] = &["__lt__", "__gt__", "__le__", "__ge__"];
        let a_b = coerce_subclass_backing(a, ORDER_OVERRIDES);
        let b_b = coerce_subclass_backing(b, ORDER_OVERRIDES);
        if a_b.is_some() || b_b.is_some() {
            let a_c = a_b.unwrap_or_else(|| a.clone());
            let b_c = b_b.unwrap_or_else(|| b.clone());
            return self.richcmp_order(&a_c, &b_c);
        }

        // Try a < b (dispatches __lt__ on a, then __gt__ on b).
        match self.try_dunder_binary(a, b, "__lt__", "__gt__") {
            Some(Ok(v)) => {
                let lt = self.truthy_value(&v)?;
                if lt {
                    return Ok(Ordering::Less);
                }
                // a is not less than b; try b < a to tell Equal from Greater.
                match self.try_dunder_binary(b, a, "__lt__", "__gt__") {
                    Some(Ok(v2)) => {
                        Ok(if self.truthy_value(&v2)? {
                            Ordering::Greater
                        } else {
                            Ordering::Equal
                        })
                    }
                    Some(Err(e)) => Err(e),
                    // No reverse dunder found: incomparable pair — raise
                    // TypeError just as CPython does for these builtins.
                    None => compare_values(a, b),
                }
            }
            Some(Err(e)) => Err(e),
            // No __lt__/__gt__ on either operand; fall through to primitive
            // comparison, which raises TypeError for incomparable instance
            // pairs — matches CPython's behaviour when no comparison dunder
            // is defined.
            None => compare_values(a, b),
        }
    }

    /// Three-way ordering comparison for `max()` — tries `__gt__` first,
    /// matching CPython's `Py_GT`-primary reduction for `max`.
    ///
    /// Emits `TypeError: '>' not supported …` (not `'<'`) when no comparison
    /// dunder is found, matching CPython 3.12 parity for `max()`.
    ///
    /// Algorithm mirrors `richcmp_order` but with primary/reflected dunders
    /// swapped and the fallback error using `>` instead of `<`.
    pub(crate) fn richcmp_order_gt(
        &mut self,
        a: &Value,
        b: &Value,
    ) -> Result<std::cmp::Ordering> {
        use std::cmp::Ordering;

        // Fast path: neither operand is a user instance.
        if !matches!(a.kind(), ValueKind::PyInstance(_))
            && !matches!(b.kind(), ValueKind::PyInstance(_))
        {
            return compare_values(a, b);
        }

        // Issue #1934/#1939: coerce builtin-subclass operands to their backing
        // (mirrors `richcmp_order`) so `max(...)` over subclass elements
        // compares via the inherited base-type ordering.
        const ORDER_OVERRIDES: &[&str] = &["__lt__", "__gt__", "__le__", "__ge__"];
        let a_b = coerce_subclass_backing(a, ORDER_OVERRIDES);
        let b_b = coerce_subclass_backing(b, ORDER_OVERRIDES);
        if a_b.is_some() || b_b.is_some() {
            let a_c = a_b.unwrap_or_else(|| a.clone());
            let b_c = b_b.unwrap_or_else(|| b.clone());
            return self.richcmp_order_gt(&a_c, &b_c);
        }

        // Try a > b (dispatches __gt__ on a, then __lt__ on b).
        match self.try_dunder_binary(a, b, "__gt__", "__lt__") {
            Some(Ok(v)) => {
                let gt = self.truthy_value(&v)?;
                if gt {
                    return Ok(Ordering::Greater);
                }
                // a is not greater than b; try b > a to tell Equal from Less.
                match self.try_dunder_binary(b, a, "__gt__", "__lt__") {
                    Some(Ok(v2)) => {
                        Ok(if self.truthy_value(&v2)? {
                            Ordering::Less
                        } else {
                            Ordering::Equal
                        })
                    }
                    Some(Err(e)) => Err(e),
                    // No reverse dunder found: incomparable pair — raise
                    // TypeError just as CPython does for max().
                    None => Err(pyrust_core::type_err!("'>' not supported between instances of '{}' and '{}'",
                            value_type_name_str(a),
                            value_type_name_str(b),)),
                }
            }
            Some(Err(e)) => Err(e),
            // No __gt__/__lt__ on either operand; emit '>' error matching
            // CPython's max() TypeError wording.
            None => Err(pyrust_core::type_err!("'>' not supported between instances of '{}' and '{}'",
                    value_type_name_str(a),
                    value_type_name_str(b),)),
        }
    }

    /// Convert a `Value` to a `PyKey`, dispatching the user's `__hash__`
    /// when the value is a `PyInstance` so user-defined classes can be used
    /// as dict/set keys (issue #368).
    ///
    /// For values that already map cleanly to a hashable `PyKey` variant
    /// via `Value::to_key`, this is a thin wrapper that surfaces the
    /// canonical "unhashable type" error.  For `PyInstance`, it looks up
    /// `__hash__` on the class, invokes it, and packages the `u64` hash
    /// (Mersenne-prime reduction + `-1 → -2` sentinel remap, matching the
    /// `hash()` builtin — issue #503) into a `PyKey::Object` along with the
    /// instance value.
    pub(crate) fn value_to_pykey(&mut self, value: &Value) -> Result<PyKey> {
        // Tuples need special handling: the core `Value::to_key` cannot
        // recurse through `PyInstance` elements (it has no interpreter
        // reference), and on an unhashable inner element it collapses the
        // error to a generic "unhashable type: 'tuple'".  CPython instead
        // surfaces the offending inner type (e.g. `unhashable type: 'list'`
        // for `{([1], 2): 0}`).  Recurse element-wise here so user
        // `__hash__` dispatch and precise error messages both work.
        if let ValueKind::Tuple(items) = value.kind() {
            let mut keys = Vec::with_capacity(items.len());
            for item in items {
                keys.push(self.value_to_pykey(item)?);
            }
            return Ok(PyKey::Tuple(keys));
        }
        // Slices with PyInstance components need interpreter access to dispatch
        // `__hash__`.  The pure `SliceOps::to_key()` path (via `value.to_key()`)
        // returns `None` for any instance component, producing a misleading
        // "unhashable type: 'slice'" error.  Intercept here when any component
        // is a PyInstance and compute the hash via `hash_value_with_interp`,
        // then store it in a `PyKey::Object` consistent with what `hash()`
        // returns for the same slice (issue #850).
        //
        // When a component is a plain unhashable primitive (list, dict, set),
        // `SliceOps::to_key()` also returns `None` but the fall-through error
        // at the end of this function would blame `'slice'` rather than the
        // actual offending component.  Detect that case here too and surface
        // the correct type name (issue #893).
        if let ValueKind::BuiltinObject { ops, state } = value.kind()
            && ops.type_name() == pyrust_builtins::slice::TYPE_NAME {
                let borrow = state.borrow();
                let s = borrow
                    .downcast_ref::<pyrust_builtins::slice::SliceState>()
                    .expect("SliceOps: bad state");
                let needs_interp =
                    crate::builtin_modules::builtins::value_needs_interp(&s.start)
                        || crate::builtin_modules::builtins::value_needs_interp(&s.stop)
                        || crate::builtin_modules::builtins::value_needs_interp(&s.step);
                // Check whether any component is an unhashable primitive so we
                // can name it precisely in the error rather than blaming 'slice'.
                // Use recursive descent so that a tuple-inside-slice (or
                // further nesting) names the leaf type, matching CPython.
                let unhashable_component: Option<String> = if !needs_interp {
                    [&s.start, &s.stop, &s.step].iter().find_map(|c| {
                        if c.to_key().is_none() {
                            Some(pyrust_builtins::set::leaf_unhashable_type_name(c))
                        } else {
                            None
                        }
                    })
                } else {
                    None
                };
                drop(borrow);
                if let Some(component_name) = unhashable_component {
                    return Err(pyrust_core::type_err!("unhashable type: '{component_name}'"));
                }
                // All slices (instance or primitive components) go through
                // hash_value_with_interp to get the CPython-compatible slice hash
                // and to dispatch user __hash__ on PyInstance components.
                let hash =
                    crate::builtin_modules::builtins::hash_value_with_interp(self, value)? as u64;
                return Ok(PyKey::Object {
                    hash,
                    value: value.clone(),
                });
            }
        if let Some(k) = value.to_key() {
            return Ok(k);
        }
        // Range objects are hashable (issue #937).  `Value::to_key` returns
        // `None` for ranges (they have no `PyKey` variant), so we handle them
        // here: compute the hash via `hash_value_with_interp` (which calls the
        // `ValueKind::Range` arm in `hash_value`) and store it in `PyKey::Object`
        // so that `range == range` lookup uses `Value`'s `PartialEq`.
        if matches!(value.kind(), ValueKind::Range { .. } | ValueKind::BigRange { .. }) {
            let hash =
                crate::builtin_modules::builtins::hash_value_with_interp(self, value)? as u64;
            return Ok(PyKey::Object {
                hash,
                value: value.clone(),
            });
        }
        if let ValueKind::PyInstance(inst) = value.kind() {
            // Issue #1936: a builtin-subclass instance (int/str/float/bytes/
            // tuple/frozenset subclass) with no user `__hash__` inherits the
            // base type's `__hash__`, so it must key identically to its backing
            // value (`hash(I(5)) == hash(5)`, `{1: "a"}[I(1)]`, `len({1, I(1)})
            // == 1`).  `coerce_subclass_backing` excludes a user `__hash__`
            // override and the `__hash__ = None` unhashable case (handled
            // below), and skips the inherited `object.__hash__`/`int.__hash__`
            // sentinels.  Only hashable (immutable) backings key by value;
            // list/dict/set backings fall through to the unhashable handling.
            if let Some(backing) = coerce_subclass_backing(value, &["__hash__"]) {
                let hashable = matches!(
                    backing.kind(),
                    ValueKind::Int(_)
                        | ValueKind::BigInt(_)
                        | ValueKind::Bool(_)
                        | ValueKind::Float(_)
                        | ValueKind::Str(_)
                        | ValueKind::Bytes(_)
                        | ValueKind::Tuple(_)
                ) || pyrust_builtins::frozenset::as_items(&backing).is_some();
                if hashable {
                    // A user `__eq__` override means equality must NOT be
                    // decided structurally by the backing's PyKey (that path
                    // never dispatches the override on lookup).  Keep the
                    // instance as a `PyKey::Object` so the dict/set runtime
                    // dispatches the user comparison, but reuse the backing's
                    // value-based hash so `hash(E(5)) == hash(5)` still holds and
                    // same-value keys land in the same bucket (CPython parity).
                    // Dict/set membership uses `__eq__` only (not `__ne__`), so a
                    // `__ne__`-only subclass stays backing-keyed/interchangeable.
                    if coerce_subclass_backing(value, &["__eq__"]).is_none() {
                        let hash = crate::builtin_modules::builtins::hash_value_with_interp(
                            self, &backing,
                        )? as u64;
                        return Ok(PyKey::Object {
                            hash,
                            value: value.clone(),
                        });
                    }
                    return self.value_to_pykey(&backing);
                }
            }
            let (class, has_builtin_data) = {
                let b = inst.borrow();
                (
                    Rc::clone(&b.class),
                    b.attrs
                        .contains_key(crate::interpreter::BUILTIN_DATA_ATTR),
                )
            };
            // Issue #2324: an instance of a subclass of an unhashable builtin
            // (`list`/`dict`/`set`/`bytearray`) with no `__hash__`-re-enabling
            // override is unhashable as a dict/set key — exactly like
            // `hash(obj)`, which routes through `class_hash_inherits_builtin_none`.
            // The `__hash__ = None` carried by those builtins is injected at
            // attribute-resolution time (`env.rs::get_attr_class`), not stored
            // in `attrs`, so the `lookup_class_attr` probe below never observes
            // it.  Without this check `{L([1])}`, `d[L([1])] = …` and
            // `{BA(b"a")}` silently succeeded (the direct `hash()` path already
            // rejected them).  A class that re-enables hashing
            // (`__hash__ = object.__hash__`) defines `__hash__` in its own dict,
            // so the helper returns `false` and that case stays hashable.
            //
            // Gate on `__builtin_data__`: only a builtin-subclass instance can
            // inherit the implicit `__hash__ = None`, and such instances always
            // carry the backing-data attr.  A plain user-class instance (the hot
            // dict/set-key case) never does, so it skips the MRO-walking helper
            // entirely (avoids a ~7% regression on user-instance keys).
            if has_builtin_data
                && crate::interpreter::class_hash_inherits_builtin_none(&class)
            {
                let class_name = class.borrow().name.clone();
                return Err(pyrust_core::type_err!("unhashable type: '{class_name}'"));
            }
            // CPython treats a class that explicitly sets `__hash__ = None`
            // as unhashable.  In pyrust we treat the absence of `__hash__`
            // the same way for now.
            if let Some(hash_method) = lookup_class_attr(&class, "__hash__") {
                if matches!(hash_method.kind(), ValueKind::None) {
                    let class_name = class.borrow().name.clone();
                    return Err(pyrust_core::type_err!("unhashable type: '{class_name}'"));
                }
                // Issue #2299/#2386: the unhashable builtins (list/dict/set/
                // bytearray) carry `__hash__ = None` implicitly, so a subclass
                // that does not override `__hash__` resolves to the inherited
                // `object.__hash__` sentinel and would otherwise key by
                // identity.  Mirror the `hash()` builtin path
                // (`hash_value_with_interp`) and reject it as unhashable so a
                // `bytearray` subclass cannot be used as a set element / dict
                // key, matching CPython.
                if matches!(
                    hash_method.kind(),
                    ValueKind::BuiltinFunction("object.__hash__")
                ) && class_hash_inherits_builtin_none(&class)
                {
                    let class_name = class.borrow().name.clone();
                    return Err(pyrust_core::type_err!("unhashable type: '{class_name}'"));
                }
                // Issue #2055: a non-callable `__hash__` slot (`__hash__ = 5`)
                // raises `TypeError: 'int' object is not callable` when hashed,
                // matching CPython, instead of silently falling back to the
                // identity hash.  A callable instance / bound method is invoked
                // (issue #2054) via `invoke_class_method`.
                if !slot_is_callable(&hash_method) {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object is not callable",
                            value_type_name_str(&hash_method)
                        ),
                    ));
                }
                {
                    let result = invoke_class_method(
                        self,
                        hash_method,
                        Value::py_instance(Rc::clone(inst)),
                        &[],
                    )?;
                    // Mirror CPython's slot_tp_hash semantics (issue #503):
                    //
                    // When `__hash__` returns an integer that fits in ssize_t
                    // (i64), CPython takes it as-is, applying only the
                    // `-1 → -2` sentinel remap (`-1` is the C-level tp_hash
                    // error indicator and must never appear as a hash value).
                    //
                    // When `__hash__` returns a value larger than ssize_t can
                    // hold (BigInt here), CPython calls `long_hash` on the
                    // returned Python int, applying Mersenne-prime reduction
                    // (mod 2^61-1) before the remap.  `py_hash_bigint` does
                    // exactly that.
                    //
                    // The stored `u64` must match what `hash(obj)` returns so
                    // that direct-hash probes into the table find their entry.
                    let raw: i64 = match result.kind() {
                        ValueKind::Int(n) => if n == -1 { -2 } else { n },
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::BigInt(n) => py_hash_bigint(n),
                        _ => {
                            return Err(pyrust_core::type_err!("__hash__ method should return an integer"));
                        }
                    };
                    return Ok(PyKey::Object {
                        hash: raw as u64,
                        value: value.clone(),
                    });
                }
            }
            // No usable __hash__: fall back to the default object-identity
            // hash so `class Foo: pass` instances remain hashable just like
            // CPython's default `object.__hash__`.
            let ptr = Rc::as_ptr(inst) as usize as u64;
            return Ok(PyKey::Object {
                hash: ptr,
                value: value.clone(),
            });
        }
        // Class objects are hashable by identity (CPython: type.__hash__).
        // Both user-defined classes and built-in primitive classes (`int`,
        // `str`, etc.) are `ValueKind::PyClass`, so this arm covers all of
        // them.  The hash is the Rc pointer, matching the `id()` value and
        // giving stable, unique hashes for distinct class objects.
        if let ValueKind::PyClass(class_rc) = value.kind() {
            let ptr = Rc::as_ptr(class_rc) as usize as u64;
            return Ok(PyKey::Object {
                hash: ptr,
                value: value.clone(),
            });
        }
        // User-defined functions, lambdas, and built-in functions are hashable
        // by identity (CPython: function.__hash__).  Use the Rc pointer for user
        // functions and the static name pointer for built-in functions, matching
        // the hash computed by hash_value for the same values.
        if let ValueKind::UserFunction(rc) = value.kind() {
            let ptr = Rc::as_ptr(rc) as usize as u64;
            return Ok(PyKey::Object {
                hash: ptr,
                value: value.clone(),
            });
        }
        if let ValueKind::BuiltinFunction(name) = value.kind() {
            let ptr = name.as_ptr() as usize as u64;
            return Ok(PyKey::Object {
                hash: ptr,
                value: value.clone(),
            });
        }
        // Bound methods: hash as hash(func) ^ hash(self), using Rc pointer
        // identity for both components, matching CPython method.__hash__.
        if let ValueKind::BoundMethod { function, receiver } = value.kind() {
            let func_ptr = Rc::as_ptr(function) as usize as u64;
            let recv_ptr = Rc::as_ptr(receiver) as usize as u64;
            let h = func_ptr ^ recv_ptr;
            return Ok(PyKey::Object {
                hash: h,
                value: value.clone(),
            });
        }
        // Class-bound methods (classmethods): same XOR pattern using the class
        // Rc pointer instead of an instance pointer.
        if let ValueKind::ClassBoundMethod { function, class } = value.kind() {
            let func_ptr = Rc::as_ptr(function) as usize as u64;
            let class_ptr = Rc::as_ptr(class) as usize as u64;
            let h = func_ptr ^ class_ptr;
            return Ok(PyKey::Object {
                hash: h,
                value: value.clone(),
            });
        }
        let type_name = value_type_name_str(value);
        Err(pyrust_core::type_err!("unhashable type: '{type_name}'"))
    }

    /// `__eq__`-aware comparison of two `PyKey`s that may nest user objects.
    /// Converts both keys back to their Python `Value` and dispatches through
    /// [`Self::values_user_eq`], which already recurses element-wise into
    /// tuples / frozensets and fires user `__eq__` for `PyInstance` elements.
    /// Used to confirm a same-hash-bucket candidate matches a tuple/frozenset
    /// lookup key whose nested object compares by `__eq__`, not identity
    /// (issue #2059).
    fn nested_object_keys_eq(&mut self, stored: &PyKey, probe: &PyKey) -> Result<bool> {
        let stored_val = crate::interpreter::key_to_value(stored.clone());
        let probe_val = crate::interpreter::key_to_value(probe.clone());
        self.values_user_eq(&stored_val, &probe_val)
    }

    /// Look up a key in a dict where the key may be a `PyKey::Object`.
    ///
    /// IndexMap's `get` will find entries whose `PyKey` matches by
    /// pointer-identity (because `PyKey::Object`'s `PartialEq` defers to
    /// `Value::eq`, which uses `Rc::ptr_eq` for `PyInstance`).  When the
    /// fast path misses and the key is an `Object`, we linearly scan
    /// entries with the same precomputed hash and dispatch user `__eq__`
    /// for full Python semantics.  Returns `Ok(Some((index, value)))` on
    /// a hit (index returned so callers can implement `pop`/`del`).
    ///
    /// Takes the receiver `&Value` (rather than `&IndexMap`) so the dict
    /// borrow can be scoped tightly: the fast path borrows for `get_full`
    /// only, and the `__eq__`-dispatching slow path borrows only long
    /// enough to extract the same-hash candidate list before dropping the
    /// borrow and running user code.  This avoids the O(N) whole-dict
    /// snapshot that callers used to have to make for soundness.
    pub(crate) fn dict_lookup(
        &mut self,
        receiver: &Value,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        // Fast path — dict borrow scoped to this block.
        {
            let dict = receiver
                .as_dict()
                .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
            if let Some((idx, _, v)) = dict.get_full(key) {
                return Ok(Some((idx, v.clone())));
            }
        }
        // Slow path — `Object` keys (and cross-variant None/Object matching,
        // issue #906).  Probe only the lookup key's hash bucket (issue #2060),
        // collecting candidate keys under a narrow borrow, then drop the borrow
        // before user `__eq__` runs.
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                collect_object_bucket_keys_map(dict, key, |k| match k {
                    PyKey::Object { hash, .. } => hash == target_hash,
                    // PyKey::None has Python-level hash py_hash_none().  When
                    // the Object key hashes to the same value, check whether
                    // __eq__ considers them equal (issue #906).
                    PyKey::None => *target_hash == none_hash,
                    _ => false,
                })
            };
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&cand_val, target)? {
                    return self.dict_entry_by_key(receiver, &cand);
                }
            }
        }
        // Cross-variant slow path: lookup key is PyKey::None but a stored
        // PyKey::Object with hash py_hash_none() may __eq__-match None (issue #906).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                collect_object_bucket_keys_map(dict, key, |k| {
                    matches!(k, PyKey::Object { hash, .. } if *hash == none_hash)
                })
            };
            let none_val = Value::none();
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&none_val, &cand_val)? {
                    return self.dict_entry_by_key(receiver, &cand);
                }
            }
        }
        // Nested-object slow path (issue #2059): a Tuple/FrozenSet key that
        // nests a user object compares its nested element by `__eq__`, not the
        // raw `PyKey` identity used by `get_full`.  Probe the lookup key's hash
        // bucket for same-shape candidates and dispatch element-wise `__eq__`.
        if nested_object_tuple_key(key) {
            let candidate_keys = {
                let dict = receiver
                    .as_dict()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                collect_object_bucket_keys_map(dict, key, nested_object_tuple_key)
            };
            for cand in candidate_keys {
                if self.nested_object_keys_eq(&cand, key)? {
                    return self.dict_entry_by_key(receiver, &cand);
                }
            }
        }
        Ok(None)
    }

    /// Recover the `(index, value)` of an entry by its exact stored key.
    /// `key` must be a key cloned from the dict's own bucket (so `Object`
    /// matches by `Rc::ptr_eq` and `None` matches `None`), making this a
    /// single O(bucket) probe rather than a full scan.
    fn dict_entry_by_key(
        &self,
        receiver: &Value,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        let dict = receiver
            .as_dict()
            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        Ok(dict.get_full(key).map(|(idx, _, v)| (idx, v.clone())))
    }

    /// `dict_lookup` variant that takes the `IndexMap` directly.  Used by
    /// callers that already hold a `&IndexMap` (typically because they
    /// own/snapshotted the dict, so aliasing with mutable access is
    /// impossible).  Prefer [`Self::dict_lookup`] for new call sites — it
    /// scopes the dict borrow tightly without a whole-dict clone.
    pub(crate) fn dict_lookup_in(
        &mut self,
        dict: &PyDict,
        key: &PyKey,
    ) -> Result<Option<(usize, Value)>> {
        if let Some((idx, _, v)) = dict.get_full(key) {
            return Ok(Some((idx, v.clone())));
        }
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            // Probe only the lookup key's hash bucket (issue #2060).
            let candidate_keys = collect_object_bucket_keys_map(dict, key, |k| match k {
                PyKey::Object { hash, .. } => hash == target_hash,
                // Cross-variant: PyKey::None has Python-level hash py_hash_none();
                // include it as a candidate when the Object also hashes to that
                // value so that __eq__ can confirm the match (issue #906).
                PyKey::None => *target_hash == none_hash,
                _ => false,
            });
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&cand_val, target)? {
                    return Ok(dict.get_full(&cand).map(|(idx, _, v)| (idx, v.clone())));
                }
            }
        }
        // Cross-variant slow path: None key vs Object entries with hash py_hash_none()
        // (issue #906).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = collect_object_bucket_keys_map(dict, key, |k| {
                matches!(k, PyKey::Object { hash, .. } if *hash == none_hash)
            });
            let none_val = Value::none();
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&none_val, &cand_val)? {
                    return Ok(dict.get_full(&cand).map(|(idx, _, v)| (idx, v.clone())));
                }
            }
        }
        // Nested-object slow path (issue #2059).
        if nested_object_tuple_key(key) {
            let candidate_keys =
                collect_object_bucket_keys_map(dict, key, nested_object_tuple_key);
            for cand in candidate_keys {
                if self.nested_object_keys_eq(&cand, key)? {
                    return Ok(dict.get_full(&cand).map(|(idx, _, v)| (idx, v.clone())));
                }
            }
        }
        Ok(None)
    }

    /// Zero-allocation string key lookup in a dict receiver (issue #506).
    ///
    /// Probes the `PyDict` using `StrKey`, which hashes
    /// identically to `PyKey::Str` without constructing a `PyKey` (zero RC
    /// bump, zero allocation).  Use this in place of
    /// `dict_lookup(&PyKey::str_from(s))` whenever the lookup key is already
    /// a `&str`.  The `PyKey::Object` slow path is omitted: a `&str` can
    /// never match an `Object` key.
    pub(crate) fn dict_str_lookup(
        &mut self,
        receiver: &Value,
        key: &str,
    ) -> Result<Option<(usize, Value)>> {
        let dict = receiver
            .as_dict()
            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        Ok(dict
            .get_full(&StrKey(key))
            .map(|(idx, _, v)| (idx, v.clone())))
    }

    /// Check whether a set contains `key`, dispatching user `__eq__` for
    /// `PyKey::Object` keys (issue #368).  Returns the entry index so
    /// callers can implement `discard`/`remove`.
    ///
    /// Takes the receiver `&Value` so the set borrow is scoped tightly —
    /// see [`Self::dict_lookup`] for the rationale.
    pub(crate) fn set_lookup(
        &mut self,
        receiver: &Value,
        key: &PyKey,
    ) -> Result<Option<usize>> {
        {
            let set = receiver
                .as_set()
                .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
            if let Some(idx) = set.get_index_of(key) {
                return Ok(Some(idx));
            }
        }
        // Slow path: probe only the lookup key's hash bucket (issue #2060),
        // dispatching user __eq__ to the few candidates that share its hash.
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = {
                let set = receiver
                    .as_set()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                collect_object_bucket_keys_set(set, key, |k| match k {
                    PyKey::Object { hash, .. } => hash == target_hash,
                    // PyKey::None has Python-level hash py_hash_none(); include it
                    // as a candidate when the Object key hashes to the same value
                    // so that __eq__ can confirm the match (issue #906).
                    PyKey::None => *target_hash == none_hash,
                    _ => false,
                })
            };
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&cand_val, target)? {
                    return self.set_index_by_key(receiver, &cand);
                }
            }
        }
        // Cross-variant slow path: None key vs Object entries with hash py_hash_none()
        // (issue #906).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = {
                let set = receiver
                    .as_set()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                collect_object_bucket_keys_set(set, key, |k| {
                    matches!(k, PyKey::Object { hash, .. } if *hash == none_hash)
                })
            };
            let none_val = Value::none();
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&none_val, &cand_val)? {
                    return self.set_index_by_key(receiver, &cand);
                }
            }
        }
        // Nested-object slow path (issue #2059): a Tuple/FrozenSet element key
        // nesting a user object compares that element by `__eq__`.
        if nested_object_tuple_key(key) {
            let candidate_keys = {
                let set = receiver
                    .as_set()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                collect_object_bucket_keys_set(set, key, nested_object_tuple_key)
            };
            for cand in candidate_keys {
                if self.nested_object_keys_eq(&cand, key)? {
                    return self.set_index_by_key(receiver, &cand);
                }
            }
        }
        Ok(None)
    }

    /// Recover the index of a set entry by its exact stored key (a key cloned
    /// from the set's own bucket).  A single O(bucket) probe, not a full scan.
    fn set_index_by_key(&self, receiver: &Value, key: &PyKey) -> Result<Option<usize>> {
        let set = receiver
            .as_set()
            .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
        Ok(set.get_index_of(key))
    }

    /// `set_lookup` variant that takes the `IndexSet` directly — for
    /// callers that already hold a `&IndexSet`.  Prefer
    /// [`Self::set_lookup`] for new call sites.
    pub(crate) fn set_lookup_in(
        &mut self,
        set: &PySet,
        key: &PyKey,
    ) -> Result<Option<usize>> {
        if let Some(idx) = set.get_index_of(key) {
            return Ok(Some(idx));
        }
        if let PyKey::Object {
            hash: target_hash,
            value: target,
        } = key
        {
            let none_hash = pyrust_core::py_hash_none() as u64;
            // Probe only the lookup key's hash bucket (issue #2060).
            let candidate_keys = collect_object_bucket_keys_set(set, key, |k| match k {
                PyKey::Object { hash, .. } => hash == target_hash,
                // Cross-variant: PyKey::None has Python-level hash py_hash_none();
                // include it as a candidate when the Object hashes to the same
                // value (issue #906).
                PyKey::None => *target_hash == none_hash,
                _ => false,
            });
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&cand_val, target)? {
                    return Ok(set.get_index_of(&cand));
                }
            }
        }
        // Cross-variant slow path: None key vs Object entries with hash py_hash_none()
        // (issue #906).
        if matches!(key, PyKey::None) {
            let none_hash = pyrust_core::py_hash_none() as u64;
            let candidate_keys = collect_object_bucket_keys_set(set, key, |k| {
                matches!(k, PyKey::Object { hash, .. } if *hash == none_hash)
            });
            let none_val = Value::none();
            for cand in candidate_keys {
                let cand_val = pykey_object_or_none_value(&cand);
                if self.values_user_eq(&none_val, &cand_val)? {
                    return Ok(set.get_index_of(&cand));
                }
            }
        }
        // Nested-object slow path (issue #2059).
        if nested_object_tuple_key(key) {
            let candidate_keys =
                collect_object_bucket_keys_set(set, key, nested_object_tuple_key);
            for cand in candidate_keys {
                if self.nested_object_keys_eq(&cand, key)? {
                    return Ok(set.get_index_of(&cand));
                }
            }
        }
        Ok(None)
    }

    /// Insert `(key, value)` into a dict that lives at register/local
    /// `dict_value`, dispatching user `__eq__` to deduplicate against an
    /// existing entry when `key` is a `PyKey::Object` or `PyKey::None`.
    /// The None case handles cross-variant dedup (issue #906): inserting
    /// None into a dict that already holds an Object key with hash py_hash_none()
    /// that __eq__-matches None should overwrite the existing entry, not add a
    /// second one.
    pub(crate) fn dict_insert(
        &mut self,
        dict: &mut PyDict,
        key: PyKey,
        value: Value,
    ) -> Result<()> {
        // `PyKey::Object` keys may collide with another Object entry (or with a
        // stored `PyKey::None`) and require `__eq__` dedup via `dict_lookup_in`.
        // `PyKey::None` is the cross-variant case (issue #906): a stored
        // `PyKey::Object{hash == py_hash_none()}` that `__eq__`-matches `None`
        // must be overwritten rather than creating a second entry.
        //
        // Fast path for `PyKey::None` (issue #934): IndexMap already deduplicates
        // `None == None` natively via `Hash`+`PartialEq`.  We only need the slow
        // `dict_lookup_in` path when the dict contains a `PyKey::Object` with hash
        // `py_hash_none()` — an extremely rare cross-variant scenario.  Skip the
        // entire lookup call in the common case.
        let needs_dedup = match &key {
            PyKey::Object { .. } => true,
            // Issue #2059: a tuple/frozenset key nesting a user object must
            // dedup against an `__eq__`-equal-but-distinct existing key.
            PyKey::Tuple(_) | PyKey::FrozenSet(_) if nested_object_tuple_key(&key) => true,
            PyKey::None => {
                let none_hash = pyrust_core::py_hash_none() as u64;
                dict.keys()
                    .any(|k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash))
            }
            _ => false,
        };
        if needs_dedup
            && let Some((idx, _)) = self.dict_lookup_in(dict, &key)? {
                // Replace value in-place via index access to preserve order.
                let existing_key = dict.get_index(idx).map(|(k, _)| k.clone());
                if let Some(k) = existing_key {
                    dict.insert(k, value);
                    return Ok(());
                }
            }
        dict.insert(key, value);
        Ok(())
    }

    /// Bulk-insert `(key, value)` pairs into a dict with last-value-wins
    /// dedup, dispatching user `__eq__` for `PyKey::Object` keys (issues
    /// #1914 / #1919).  This is the shared mechanism behind `dict.update`,
    /// `|`/`|=`, `dict.fromkeys`, `dict(pairs)`, and the collections
    /// `Counter`/`defaultdict` bulk paths.
    ///
    /// Fast path: when neither the destination map nor any incoming key is a
    /// `PyKey::Object` (the overwhelmingly common primitive-key case), this is
    /// a plain `IndexMap::extend` — no `__eq__` dispatch, no per-key scan.  The
    /// slow path engages only when an `Object` key is present on either side,
    /// routing each insert through `dict_insert` (which dedups via
    /// `dict_lookup_in`'s `__hash__`-then-`__eq__` scan).
    pub(crate) fn dict_extend_dedup(
        &mut self,
        dict: &mut PyDict,
        pairs: Vec<(PyKey, Value)>,
    ) -> Result<()> {
        let dest_has_object = dict.keys().any(key_contains_object);
        let src_has_object = pairs.iter().any(|(k, _)| key_contains_object(k));
        if !dest_has_object && !src_has_object {
            // Primitive-key fast path: raw IndexMap::extend (last value wins).
            dict.extend(pairs);
            return Ok(());
        }
        for (key, value) in pairs {
            self.dict_insert(dict, key, value)?;
        }
        Ok(())
    }

    /// In-place bulk-update of a dict *receiver* `Value` with last-value-wins
    /// dedup, dispatching user `__eq__` for `PyKey::Object` keys (issue #1914).
    /// This is the receiver-based companion to [`Self::dict_extend_dedup`],
    /// used where the dict must be mutated in place (`dict.update`, `|=`) so
    /// aliasing references observe the change.
    ///
    /// Fast path: when neither the receiver nor any incoming key is a
    /// `PyKey::Object`, a single `dict_with_mut` raw `extend` (last value wins).
    /// Slow path: per-pair `dict_lookup` on the receiver (which drops the dict
    /// borrow before running user `__eq__`) followed by a scoped `dict_with_mut`
    /// insert that overwrites the `__eq__`-equal entry in place.
    pub(crate) fn dict_extend_value_dedup(
        &mut self,
        receiver: &Value,
        pairs: Vec<(PyKey, Value)>,
    ) -> Result<()> {
        let dest_has_object = receiver
            .dict_with(|d| d.keys().any(key_contains_object))
            .unwrap_or(false);
        let src_has_object = pairs.iter().any(|(k, _)| key_contains_object(k));
        if !dest_has_object && !src_has_object {
            // Primitive-key fast path: raw IndexMap::extend (last value wins).
            receiver
                .dict_with_mut(|dict| dict.extend(pairs))
                .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
            return Ok(());
        }
        for (key, value) in pairs {
            let existing = self.dict_lookup(receiver, &key)?;
            receiver
                .dict_with_mut(|dict| {
                    if let Some((idx, _)) = existing {
                        // Overwrite the matching entry in place, keeping the
                        // existing (stored) key object and its position.
                        if let Some(k) = dict.get_index(idx).map(|(k, _)| k.clone()) {
                            dict.insert(k, value);
                            return;
                        }
                    }
                    dict.insert(key, value);
                })
                .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        }
        Ok(())
    }

    /// Extract `(PyKey, Value)` pairs from a `**mapping` source value, matching
    /// the duck-typed mapping protocol the `DictUpdate` instruction accepts
    /// (dict / `instance_dict` proxy / `mappingproxy` / any `PyInstance` with
    /// `keys()` + `__getitem__`).  Shared by the `DictUpdate` and
    /// `DictMergeKwCall` VM arms.
    pub(crate) fn mapping_splat_pairs(
        &mut self,
        src_val: &Value,
    ) -> Result<Vec<(PyKey, Value)>> {
        match src_val.kind() {
            ValueKind::Dict(d) => Ok(d.clone().into_iter().collect()),
            ValueKind::BuiltinObject { ops, .. }
                if ops.type_name() == pyrust_builtins::instance_dict::TYPE_NAME =>
            {
                match pyrust_builtins::instance_dict::as_instance_dict_items(src_val) {
                    Some(pairs) => Ok(pairs),
                    None => Err(PyError::Runtime(
                        "internal: bad instance_dict state in DictUpdate".to_string(),
                    )),
                }
            }
            ValueKind::BuiltinObject { ops, .. }
                if ops.type_name() == pyrust_builtins::mapping_proxy::TYPE_NAME =>
            {
                if let Some(cls_rc) = pyrust_builtins::mapping_proxy::as_class_rc(src_val) {
                    Ok(cls_rc
                        .borrow()
                        .attrs
                        .iter()
                        .map(|(k, v)| (PyKey::str_from(k), v.clone()))
                        .collect())
                } else if let Some(dict_rc) =
                    pyrust_builtins::mapping_proxy::as_dict_rc(src_val)
                {
                    // Dict-backed mappingproxy (`d.keys().mapping`, #2679).
                    Ok(dict_rc.borrow().clone().into_iter().collect())
                } else {
                    Err(PyError::Runtime(
                        "internal: bad mappingproxy state in DictUpdate".to_string(),
                    ))
                }
            }
            ValueKind::PyInstance(_) => {
                match mapping_pairs_via_protocol(self, src_val)? {
                    Some(pairs) => Ok(pairs),
                    None => Err(pyrust_core::type_err!(
                        "'{}' object is not a mapping",
                        value_type_name_str(src_val)
                    )),
                }
            }
            _ => Err(pyrust_core::type_err!(
                "'{}' object is not a mapping",
                value_type_name_str(src_val)
            )),
        }
    }

    /// In-place merge of a `**d` keyword-splat `pairs` into a call's kwargs
    /// `receiver` (CPython `DICT_MERGE`).  On the first duplicate key, inserts
    /// nothing further and returns `Ok(Some(kw))` naming the colliding keyword
    /// so the caller can raise `… got multiple values for keyword argument …`
    /// with a lazily-resolved function name (no allocation on the happy path).
    pub(crate) fn dict_merge_kwcall(
        &mut self,
        receiver: &Value,
        pairs: Vec<(PyKey, Value)>,
    ) -> Result<Option<String>> {
        for (key, value) in pairs {
            if self.dict_lookup(receiver, &key)?.is_some() {
                return Ok(Some(kwkey_name(&key)));
            }
            receiver
                .dict_with_mut(|dict| dict.insert(key, value))
                .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        }
        Ok(None)
    }

    /// `R[dict][key] = val` for a named call argument when a `**d` splat is also
    /// present.  Returns `Ok(Some(kw))` (without inserting) if `key` already
    /// exists, mirroring [`Self::dict_merge_kwcall`].
    pub(crate) fn dict_setitem_kwcall(
        &mut self,
        receiver: &Value,
        key: PyKey,
        value: Value,
    ) -> Result<Option<String>> {
        if self.dict_lookup(receiver, &key)?.is_some() {
            return Ok(Some(kwkey_name(&key)));
        }
        receiver
            .dict_with_mut(|dict| dict.insert(key, value))
            .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
        Ok(None)
    }

    /// Resolve the callee's `<module>.<qualname>` for a `DictMergeKwCall` /
    /// `SetItemKwCall` duplicate-key error.  Returns `None` when the name can't
    /// be cheaply recovered, in which case the error omits the function prefix.
    /// Best-effort and side-effect-free: the method branch reads the method off
    /// the receiver's class via `lookup_class_attr` (no descriptor dispatch).
    pub(crate) fn kwcall_func_name(
        &self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        name: &crate::bytecode::KwCallName,
        code: &crate::bytecode::FnCode,
    ) -> Option<String> {
        match name {
            crate::bytecode::KwCallName::Callee(reg) => {
                let cv = vm_read(regs, *reg, num_locals).ok()?;
                callee_function_str(&cv)
            }
            crate::bytecode::KwCallName::Method { obj, name_idx } => {
                let recv = vm_read(regs, *obj, num_locals).ok()?;
                let method = code.names.get(*name_idx as usize)?.as_str();
                let class = match recv.kind() {
                    ValueKind::PyInstance(inst) => Rc::clone(&inst.borrow().class),
                    ValueKind::PyClass(cls) => Rc::clone(cls),
                    _ => return None,
                };
                let unbound = lookup_class_attr(&class, method)?;
                callee_function_str(&unbound)
            }
        }
    }

    /// Insert `key` into a set, dispatching user `__eq__` for dedup.
    /// Handles both `Object` keys and `None` keys for cross-variant dedup
    /// (issue #906): inserting None into a set that already holds an Object
    /// with hash py_hash_none() that __eq__-matches None must not create a
    /// duplicate.
    pub(crate) fn set_insert(
        &mut self,
        set: &mut PySet,
        key: PyKey,
    ) -> Result<()> {
        // Same fast pre-check as `dict_insert` (issue #934): for `PyKey::None`,
        // only call `set_lookup_in` when the set contains a `PyKey::Object` with
        // hash `py_hash_none()` (rare cross-variant case, issue #906).
        let needs_dedup = match &key {
            PyKey::Object { .. } => true,
            // Issue #2059: dedup a tuple/frozenset key nesting a user object
            // against an `__eq__`-equal-but-distinct existing element.
            PyKey::Tuple(_) | PyKey::FrozenSet(_) if nested_object_tuple_key(&key) => true,
            PyKey::None => {
                let none_hash = pyrust_core::py_hash_none() as u64;
                set.iter()
                    .any(|k| matches!(k, PyKey::Object { hash, .. } if *hash == none_hash))
            }
            _ => false,
        };
        if needs_dedup && self.set_lookup_in(set, &key)?.is_some() {
            return Ok(());
        }
        set.insert(key);
        Ok(())
    }

    /// Compare two values via `__eq__`, used by the dict/set runtime when
    /// resolving `PyKey::Object` collisions and by `BinaryOp::Eq`/`Ne`'s
    /// container fall-through path (issue #436).
    ///
    /// Dispatch order, structured to keep the flat-primitive hot path
    /// allocation-free:
    ///
    /// 1. Same-kind sequence pair (`List`/`List` or `Tuple`/`Tuple`):
    ///    `try_seq_fast_eq` walks the borrow pairwise and resolves any
    ///    pair that doesn't transitively need user dispatch via
    ///    `Value::eq`.  This avoids the double-walk an upfront
    ///    `a == b` would cause and matches pre-#436 perf for primitive
    ///    sequences.  When a pair could need dispatch (`PyInstance` or
    ///    nested container), snapshot both sides and recurse.
    /// 2. Primitive / identity fast path: `a == b` for the non-sequence
    ///    cases (`Int`/`Float`/`Bool`/`Str`/`Bytes`/`Complex`/`None`
    ///    and identity-equal `Dict`/`Set`).
    /// 3. Same-kind `Dict`/`Set`: snapshot keys and dispatch via
    ///    `dict_lookup`/`set_lookup`, which already route
    ///    `PyKey::Object` through user `__hash__`/`__eq__` (issue #368).
    /// 4. Both sides are `frozenset` (`BuiltinObject`): same membership
    ///    check as Set but via `set_lookup_in`, so `PyKey::Object`
    ///    elements (user-class instances) dispatch `__eq__` correctly.
    /// 5. `PyInstance` on either side: `try_dunder_binary` for
    ///    `__eq__`/reflected `__eq__`.
    ///
    /// Cycle detection mirrors `Value::eq`'s `EqGuard`: a recursive call
    /// for the same `(value_id(a), value_id(b))` pair returns true (the
    /// recursion bottoms out as "we've already proven the prefix equal"),
    /// so `a.append(a); b.append(b); a == b` doesn't blow the stack.
    pub(crate) fn values_user_eq(&mut self, a: &Value, b: &Value) -> Result<bool> {
        // Same-kind sequence containers come first.  For `List`/`Tuple`
        // pairs an upfront `Value::eq` would double-walk: `Vec::eq`
        // already iterates element-wise, and the recursion below would
        // repeat the walk.  Going straight to `try_seq_fast_eq`
        // resolves flat primitive sequences (`[1,2,3] == [1,2,4]`) in
        // a single borrow-only pass with no allocation — matching
        // pre-#436 perf.  Mixed-kind pairs (e.g. list vs tuple) fall
        // through to the primitive/identity fast path below.
        let needs_seq_dispatch = match (a.kind(), b.kind()) {
            (ValueKind::List(la), ValueKind::List(lb)) => {
                if la.len() != lb.len() {
                    return Ok(false);
                }
                match try_seq_fast_eq(&la, &lb) {
                    SeqFast::Resolved(v) => return Ok(v),
                    SeqFast::NeedsDispatch => true,
                }
            }
            (ValueKind::Tuple(la), ValueKind::Tuple(lb)) => {
                if la.len() != lb.len() {
                    return Ok(false);
                }
                match try_seq_fast_eq(la, lb) {
                    SeqFast::Resolved(v) => return Ok(v),
                    SeqFast::NeedsDispatch => true,
                }
            }
            _ => false,
        };
        if needs_seq_dispatch {
            // Slow path: snapshot both sides to drop the borrow before
            // recursing into user code, then walk element-wise through
            // `values_user_eq` so `PyInstance` elements dispatch
            // `__eq__`.  Element clones are cheap (Rc/NaN-box copy).
            let (av, bv): (Vec<Value>, Vec<Value>) = match (a.kind(), b.kind()) {
                (ValueKind::List(la), ValueKind::List(lb)) => {
                    (la.iter().cloned().collect(), lb.iter().cloned().collect())
                }
                (ValueKind::Tuple(la), ValueKind::Tuple(lb)) => {
                    (la.to_vec(), lb.to_vec())
                }
                _ => unreachable!("needs_seq_dispatch implies a sequence pair"),
            };
            if self.eq_cycle_enter(a, b) {
                // Already comparing this pair further up the stack —
                // treat as equal to terminate the recursion (matching
                // `Value::eq`'s `EqGuard` policy).
                return Ok(true);
            }
            let result = (|| -> Result<bool> {
                for (x, y) in av.iter().zip(bv.iter()) {
                    if !self.values_user_eq(x, y)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            })();
            self.eq_cycle_exit(a, b);
            return result;
        }

        // Primitive / identity fast path: `Value::eq` handles
        // Int/Float/Bool/Str/Bytes/Complex/None and identity-equal
        // Dict/Set without dunder dispatch.  (List/Tuple were already
        // handled above to avoid the double-walk.)
        if a == b {
            return Ok(true);
        }

        match (a.kind(), b.kind()) {
            (ValueKind::Dict(da), ValueKind::Dict(db)) => {
                if da.len() != db.len() {
                    return Ok(false);
                }
                if self.eq_cycle_enter(a, b) {
                    return Ok(true);
                }
                // Snapshot (PyKey, Value) pairs from `a` so user `__eq__`
                // (run while looking up in `b`) can't invalidate the dict
                // borrow.  We pass the snapshotted `PyKey` straight to
                // `dict_lookup` so `__hash__` / `__eq__` dispatch on
                // `PyKey::Object` keys still works (issue #368).
                let entries: Vec<(PyKey, Value)> = da
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                let result = (|| -> Result<bool> {
                    for (pk, v_lhs) in entries {
                        match self.dict_lookup(b, &pk)? {
                            Some((_, v_rhs)) => {
                                if v_lhs == v_rhs || v_lhs.is_identical_nan(&v_rhs) {
                                    // same object or NaN-identity: treat as equal
                                    // (mirrors CPython PyObject_RichCompareBool)
                                    continue;
                                }
                                if !self.values_user_eq(&v_lhs, &v_rhs)? {
                                    return Ok(false);
                                }
                            }
                            None => return Ok(false),
                        }
                    }
                    Ok(true)
                })();
                self.eq_cycle_exit(a, b);
                return result;
            }
            (ValueKind::Set(sa), ValueKind::Set(sb)) => {
                if sa.len() != sb.len() {
                    return Ok(false);
                }
                if self.eq_cycle_enter(a, b) {
                    return Ok(true);
                }
                let keys: Vec<PyKey> = sa.iter().cloned().collect();
                let result = (|| -> Result<bool> {
                    for pk in keys {
                        if self.set_lookup(b, &pk)?.is_none() {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                })();
                self.eq_cycle_exit(a, b);
                return result;
            }
            _ => {}
        }

        // Frozenset — same membership logic as Set above, but the items
        // live inside a BuiltinObject.  `set_lookup_in` handles
        // `PyKey::Object` elements by dispatching user `__eq__`, so
        // `frozenset({a}) == frozenset({b})` works correctly when
        // `a.__eq__(b)` returns True.  Non-frozenset BuiltinObject pairs
        // fall through to `try_dunder_binary` (the PyInstance path); if
        // that also yields nothing, we return false — identical to
        // `Value::eq`'s behaviour for unrecognised BuiltinObject pairs.
        if let (Some(lhs_rc), Some(rhs_rc)) = (
            pyrust_builtins::frozenset::as_items(a),
            pyrust_builtins::frozenset::as_items(b),
        ) {
            if lhs_rc.len() != rhs_rc.len() {
                return Ok(false);
            }
            if self.eq_cycle_enter(a, b) {
                return Ok(true);
            }
            let lhs_keys: Vec<PyKey> = lhs_rc.iter().cloned().collect();
            let rhs_snap: PySet = rhs_rc.iter().cloned().collect();
            let result = (|| -> Result<bool> {
                for pk in lhs_keys {
                    if self.set_lookup_in(&rhs_snap, &pk)?.is_none() {
                        return Ok(false);
                    }
                }
                Ok(true)
            })();
            self.eq_cycle_exit(a, b);
            return result;
        }

        // Issue #1891: the set-like dict views `dict_keys` / `dict_items`
        // compare as sets against any other set-like operand (`set`,
        // `frozenset`, or another set-like view).  CPython's view `__eq__`
        // returns `False` (not TypeError) when the other operand is *not*
        // set-like — including `dict_values`, lists, and dicts.  `dict_items`
        // with an unhashable value raises `TypeError: unhashable type: …`,
        // which `coerce_set_operand` surfaces.
        if is_setlike_view(a) || is_setlike_view(b) {
            let a_set = self.coerce_set_operand(a);
            let b_set = self.coerce_set_operand(b);
            match (a_set, b_set) {
                (Some(a_res), Some(b_res)) => {
                    let (sa, _) = a_res?;
                    let (sb, _) = b_res?;
                    if sa.len() != sb.len() {
                        return Ok(false);
                    }
                    let needs_eq = set_has_object_key(&sa) || set_has_object_key(&sb);
                    if !needs_eq {
                        return Ok(sa.iter().all(|k| sb.contains(k)));
                    }
                    for k in sa.iter() {
                        if self.set_lookup_in(&sb, k)?.is_none() {
                            return Ok(false);
                        }
                    }
                    return Ok(true);
                }
                // A view vs a non-set-like operand: not equal (CPython returns
                // False without building the set, so an unhashable `dict_items`
                // value does *not* raise here — `items == [..]` is just False).
                (Some(_), None) | (None, Some(_)) => return Ok(false),
                (None, None) => unreachable!("is_setlike_view implies a set-like operand"),
            }
        }

        // Issue #1939: a container subclass (list/tuple/dict/set/frozenset
        // subclass) with no user `__eq__` override inherits the base type's
        // equality, so `L([1,2]) == [1,2]`, `D({1:'a'}) == {1:'a'}`, and
        // `St({1,2}) == {1,2}` compare by backing value.  The concrete-
        // container fast paths above have already returned, so this only runs
        // when a `PyInstance` operand reaches the bottom — no cost on the hot
        // `[1,2,3] == [1,2,3]` path.  Coerce the container backing(s) and
        // recurse so the List/Tuple/Dict/Set/Frozenset arms above run; a user
        // `__eq__` override is excluded by `coerce_subclass_backing`.
        let a_cont = coerce_container_backing_for_eq(a);
        let b_cont = coerce_container_backing_for_eq(b);
        if a_cont.is_some() || b_cont.is_some() {
            let a_c = a_cont.unwrap_or_else(|| a.clone());
            let b_c = b_cont.unwrap_or_else(|| b.clone());
            return self.values_user_eq(&a_c, &b_c);
        }

        // PyInstance (either side) — dispatch `__eq__`/reflected
        // `__eq__`.  This is the original `values_user_eq` body.
        if let Some(r) = self.try_dunder_binary(a, b, "__eq__", "__eq__") {
            return Ok(r?.truthy_raw());
        }
        // Issue #1204: if a PyInstance has a scalar primitive backing
        // (e.g. MyInt subclass) and no user __eq__ was found, compare the
        // backing values so `MyInt(5) == 5` returns True.
        let a_cmp = coerce_numeric(a);
        let b_cmp = coerce_numeric(b);
        if !matches!(a_cmp.kind(), ValueKind::PyInstance(_))
            || !matches!(b_cmp.kind(), ValueKind::PyInstance(_))
        {
            // At least one side was coerced out of PyInstance.
            return Ok(a_cmp == b_cmp);
        }
        Ok(false)
    }

    /// Enter equality recursion for the `(value_id(a), value_id(b))`
    /// pair.  Returns `true` when a cycle is detected (the caller should
    /// short-circuit to "equal" without pushing); returns `false`
    /// otherwise after pushing the pair onto the recursion stack.  Each
    /// `false` return must be matched by an `eq_cycle_exit` call.
    ///
    /// Primitives (no `value_id`) can't form cycles, so we return
    /// `false` without recording anything — the missing push is paired
    /// with a no-op `eq_cycle_exit`.
    fn eq_cycle_enter(&mut self, a: &Value, b: &Value) -> bool {
        let (Some(a_id), Some(b_id)) = (a.value_id(), b.value_id()) else {
            return false;
        };
        let pair = (a_id, b_id);
        if self.eq_in_progress.contains(&pair) {
            return true;
        }
        self.eq_in_progress.push(pair);
        false
    }

    /// Pop the matching pair from the recursion stack.  No-op when the
    /// pair wasn't pushed (one operand was a primitive without
    /// `value_id`).
    fn eq_cycle_exit(&mut self, a: &Value, b: &Value) {
        let (Some(a_id), Some(b_id)) = (a.value_id(), b.value_id()) else {
            return;
        };
        if let Some(pos) = self
            .eq_in_progress
            .iter()
            .rposition(|p| *p == (a_id, b_id))
        {
            self.eq_in_progress.remove(pos);
        }
    }

    /// Dispatch any dict method.  Methods that read or write keys
    /// (`get`/`pop`/`setdefault`/`__contains__`) route through
    /// `dict_lookup`/`dict_insert` so user-defined `__hash__`/`__eq__`
    /// fire (issue #368).  Everything else delegates to the
    /// interpreter-free `pyrust_builtins::dict::call`.
    ///
    /// Callers don't need to know which methods are which — this is the
    /// single entry point for dict method dispatch.
    pub(crate) fn call_dict_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
        kwargs: &PyDict,
    ) -> Result<Value> {
        // Issue #2500: every dict method except `update` (which accepts
        // `**kwargs`) rejects keyword arguments — the receiver-only
        // `pyrust_builtins::dict::call` otherwise discards them silently.
        if let Some(err) = reject_container_method_kwargs("dict", method, kwargs) {
            return Err(err);
        }
        match method {
            "get" | "__contains__" | "pop" | "setdefault" => {
                let mut iter = args.into_iter();
                let key_val = iter.next().ok_or_else(|| {
                    PyError::Runtime(format!("dict.{method}() requires at least 1 argument"))
                })?;
                let pk = self.value_to_pykey(&key_val)?;
                match method {
                    "get" => {
                        let default = iter.next().unwrap_or_else(Value::none);
                        Ok(self
                            .dict_lookup(&receiver, &pk)?
                            .map(|(_, v)| v)
                            .unwrap_or(default))
                    }
                    "__contains__" => Ok(Value::bool_(
                        self.dict_lookup(&receiver, &pk)?.is_some(),
                    )),
                    "pop" => match self.dict_lookup(&receiver, &pk)? {
                        Some((idx, v)) => {
                            // `dict_lookup` already dropped its borrow before
                            // running user code, so the index is still valid.
                            receiver.dict_with_mut(|dict| dict.shift_remove_index(idx));
                            Ok(v)
                        }
                        None => {
                            if let Some(default) = iter.next() {
                                Ok(default)
                            } else {
                                Err(PyError::key_error(key_val.clone()))
                            }
                        }
                    },
                    "setdefault" => {
                        let default = iter.next().unwrap_or_else(Value::none);
                        if let Some((_, v)) = self.dict_lookup(&receiver, &pk)? {
                            return Ok(v);
                        }
                        receiver
                            .dict_with_mut(|dict| dict.insert(pk, default.clone()))
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected dict".to_string())
                            })?;
                        Ok(default)
                    }
                    _ => unreachable!(),
                }
            }
            // `update` with non-primitive iterables (range, generators,
            // user-defined iterables) — the builtins crate has no interpreter
            // access and falls to its `_` arm raising "'X' object is not
            // iterable" for these types.  Intercept here when the positional
            // arg is not one of the five primitive types the builtins crate
            // already handles (Dict/List/Tuple/Str/Bytes).  Delegate for those
            // types to preserve existing behaviour (including the self-alias
            // snapshot logic in snapshot_update_arg).
            "update" => {
                if args.len() > 1 {
                    return Err(pyrust_core::type_err!("update expected at most 1 argument, got {}", args.len()));
                }
                // #1914: when the update source is a `dict` (the common
                // `d.update(other_dict)` and `**kwargs`-into-dict path), route
                // through `dict_extend_value_dedup` so `PyKey::Object` keys
                // deduplicate via user `__eq__` (last value wins).  The helper
                // keeps the raw fast path for all-primitive keys.  kwargs are
                // string keys, always primitive — append them after.
                if let Some(arg) = args.first() {
                    // Snapshot the source pairs in a scoped block so the `Dict`
                    // `Ref` borrow is dropped before `dict_extend_value_dedup`
                    // takes a `borrow_mut` — critical for the self-aliased
                    // `d.update(d)` case where `arg` IS `receiver` (#448).
                    let src_pairs: Option<Vec<(PyKey, Value)>> = match arg.kind() {
                        ValueKind::Dict(src) => {
                            Some(src.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        }
                        _ => None,
                    };
                    if let Some(pairs) = src_pairs {
                        self.dict_extend_value_dedup(&receiver, pairs)?;
                        if !kwargs.is_empty() {
                            let kw_pairs: Vec<(PyKey, Value)> = kwargs
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                            self.dict_extend_value_dedup(&receiver, kw_pairs)?;
                        }
                        return Ok(Value::none());
                    }
                }
                // Check whether we need to intercept.  If the single positional
                // arg is a primitive type that pyrust_builtins::dict::call already
                // handles correctly, delegate.  `List`/`Tuple` (iterable of
                // pairs) go through the interpreter slow path below so that
                // PyInstance keys hash via user `__hash__` and dedup via user
                // `__eq__` (#1914); `Str`/`Bytes` pairs are always char/byte
                // primitives and stay on the fast builtin path.
                let needs_interp = match args.first() {
                    None => false,
                    Some(arg) => !matches!(
                        arg.kind(),
                        ValueKind::Dict(_) | ValueKind::Str(_) | ValueKind::Bytes(_)
                    ),
                };
                if !needs_interp {
                    return pyrust_builtins::dict::call("update", &receiver, args, kwargs);
                }
                // Intercept: the arg is a non-primitive iterable (Range,
                // Generator, BuiltinObject, PyInstance, …).
                let arg = args.into_iter().next().unwrap();
                // #2222: CPython's `dict.update` first checks for a `keys()`
                // method and, when present, treats the arg as a *mapping*
                // (iterate `keys()` and subscript via `__getitem__`) rather
                // than an iterable of pairs.  Route any keys()-bearing mapping
                // (ChainMap / OrderedDict / Counter / UserDict / custom)
                // through the same protocol helper used by `dict()` and
                // `**`-unpack (#2190), so the two paths stay consistent.
                if let Some(pairs) = crate::interpreter::mapping_pairs_via_protocol(self, &arg)? {
                    self.dict_extend_value_dedup(&receiver, pairs)?;
                    for (k, v) in kwargs {
                        receiver
                            .dict_with_mut(|dict| {
                                dict.insert(k.clone(), v.clone());
                            })
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected dict".to_string())
                            })?;
                    }
                    return Ok(Value::none());
                }
                // `mappingproxy` is a mapping (it has `keys()`), but it is a
                // BuiltinObject, not a PyInstance, so the protocol helper above
                // returns `None` for it.  Treat both proxy variants as mappings
                // and copy their pairs verbatim, matching `dict()` / `{**m}`
                // (#2679, and the pre-existing class-backed `vars(C)` form).
                let proxy_pairs: Option<Vec<(PyKey, Value)>> = if let Some(cls_rc) =
                    pyrust_builtins::mapping_proxy::as_class_rc(&arg)
                {
                    Some(
                        cls_rc
                            .borrow()
                            .attrs
                            .iter()
                            .map(|(k, v)| (PyKey::str_from(k), v.clone()))
                            .collect(),
                    )
                } else {
                    pyrust_builtins::mapping_proxy::as_dict_rc(&arg)
                        .map(|dict_rc| dict_rc.borrow().clone().into_iter().collect())
                };
                if let Some(pairs) = proxy_pairs {
                    self.dict_extend_value_dedup(&receiver, pairs)?;
                    for (k, v) in kwargs {
                        receiver
                            .dict_with_mut(|dict| {
                                dict.insert(k.clone(), v.clone());
                            })
                            .ok_or_else(|| {
                                PyError::Runtime("internal: expected dict".to_string())
                            })?;
                    }
                    return Ok(Value::none());
                }
                // Drive the iterable one element at a time and insert each
                // pair into the dict eagerly.  This matches CPython: items
                // yielded before a mid-iteration exception are already in the
                // dict.  Using collect_iterable (materialise-then-process)
                // would silently drop those items when the generator raises.
                let iter = crate::builtin_modules::builtins::make_iterator(self, &arg)?;
                // Each element must be a length-2 sequence; extract the key and
                // value.  Mirror the logic in pyrust_builtins::dict's push_pair,
                // but use value_to_pykey so user-defined __hash__/__eq__ fire
                // correctly for PyInstance keys.
                let mut idx: usize = 0;
                loop {
                    let elem = match self.call_next(&iter, None) {
                        Ok(v) => v,
                        Err(ref e) if e.class_name_is("StopIteration") => break,
                        Err(e) => return Err(e),
                    };
                    let (k_val, v_val): (Value, Value) = match elem.kind() {
                        ValueKind::List(items) => {
                            let len = items.len();
                            if len != 2 {
                                return Err(pyrust_core::value_err!("dictionary update sequence element #{idx} has length {len}; 2 is required"));
                            }
                            (items[0].clone(), items[1].clone())
                        }
                        ValueKind::Tuple(items) => {
                            let len = items.len();
                            if len != 2 {
                                return Err(pyrust_core::value_err!("dictionary update sequence element #{idx} has length {len}; 2 is required"));
                            }
                            (items[0].clone(), items[1].clone())
                        }
                        ValueKind::Str(s) => {
                            let chars: Vec<char> = s.chars().collect();
                            let len = chars.len();
                            if len != 2 {
                                return Err(pyrust_core::value_err!("dictionary update sequence element #{idx} has length {len}; 2 is required"));
                            }
                            (
                                Value::string(chars[0].to_string()),
                                Value::string(chars[1].to_string()),
                            )
                        }
                        _ => {
                            return Err(pyrust_core::type_err!("cannot convert dictionary update sequence element #{idx} to a sequence"));
                        }
                    };
                    let pk = self.value_to_pykey(&k_val)?;
                    // #1914: dedup `PyKey::Object` keys via user `__eq__` (the
                    // dict-arg fast path above and `dict_extend_value_dedup`
                    // share this last-value-wins semantics).
                    self.dict_extend_value_dedup(&receiver, vec![(pk, v_val)])?;
                    idx += 1;
                }
                // Apply keyword arguments after the positional iterable,
                // matching CPython's order.
                for (k, v) in kwargs {
                    receiver
                        .dict_with_mut(|dict| {
                            dict.insert(k.clone(), v.clone());
                        })
                        .ok_or_else(|| {
                            PyError::Runtime("internal: expected dict".to_string())
                        })?;
                }
                Ok(Value::none())
            }
            // `fromkeys` is a classmethod: ignore the dict receiver and call
            // the registry dispatch directly with the user-supplied args.
            "fromkeys" => {
                let dispatch = crate::builtin_registry::lookup("dict.fromkeys")
                    .ok_or_else(|| {
                        PyError::Runtime(
                            "internal: dict.fromkeys not in registry".to_string(),
                        )
                    })?;
                let expanded: Vec<ExpandedCallArg> = args
                    .into_iter()
                    .map(|v| ExpandedCallArg { name: None, value: v })
                    .collect();
                dispatch(self, &expanded)
            }
            // keys/values/items must build a LIVE guarded view — dict::call
            // without the backing Rc materialises a list snapshot (wrong type,
            // unguarded).  The #2436 review found this THIRD copy of the view
            // decision via getattr-bound calls; route through the shared
            // constructor like the slow-path and inline-cache sites.
            "keys" | "values" | "items" if args.is_empty() && kwargs.is_empty() => {
                Self::dict_view_for_backing(&receiver, method, false)
            }
            _ => pyrust_builtins::dict::call(method, &receiver, args, kwargs),
        }
    }

    /// Dispatch any set method.  Methods that read or write keys
    /// (`add`/`discard`/`remove`/`__contains__`) route through
    /// `set_lookup`/`set_insert` so user-defined `__hash__`/`__eq__`
    /// fire (issue #368).  Everything else delegates to the
    /// interpreter-free `pyrust_builtins::set::call`.
    pub(crate) fn call_set_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "add" | "__contains__" | "discard" | "remove" => {
                let mut iter = args.into_iter();
                let key_val = iter.next().ok_or_else(|| {
                    PyError::Runtime(format!("set.{method}() requires at least 1 argument"))
                })?;
                let pk = self.value_to_pykey(&key_val)?;
                match method {
                    "add" => {
                        if self.set_lookup(&receiver, &pk)?.is_some() {
                            return Ok(Value::none());
                        }
                        receiver.set_add(pk)?;
                        Ok(Value::none())
                    }
                    "__contains__" => {
                        Ok(Value::bool_(self.set_lookup(&receiver, &pk)?.is_some()))
                    }
                    "discard" => {
                        if let Some(idx) = self.set_lookup(&receiver, &pk)? {
                            receiver
                                .set_with_mut(|set| {
                                    set.shift_remove_index(idx);
                                })
                                .ok_or_else(|| {
                                    PyError::Runtime("internal: expected set".to_string())
                                })?;
                        }
                        Ok(Value::none())
                    }
                    "remove" => match self.set_lookup(&receiver, &pk)? {
                        Some(idx) => {
                            receiver
                                .set_with_mut(|set| {
                                    set.shift_remove_index(idx);
                                })
                                .ok_or_else(|| {
                                    PyError::Runtime("internal: expected set".to_string())
                                })?;
                            Ok(Value::none())
                        }
                        None => Err(PyError::key_error(key_val.clone())),
                    },
                    _ => unreachable!(),
                }
            }
            // set.update uses value_to_pykey so that hashable slices and
            // PyInstance elements (which need __hash__ dispatch) work correctly.
            // The pyrust-builtins path calls Value::to_key() which returns None
            // for slices (SliceOps doesn't implement hash), causing a misleading
            // "unhashable type: 'slice'" error for all slices.
            "update" => {
                for arg in args {
                    // Snapshot if the argument is the receiver itself to avoid
                    // aliased-borrow issues during iteration (matches CPython
                    // semantics: s.update(s) is a no-op).
                    if arg.value_id() == receiver.value_id() && arg.value_id().is_some() {
                        let snapshot: Vec<PyKey> = receiver
                            .set_with(|s| s.iter().cloned().collect())
                            .unwrap_or_default();
                        for pk in snapshot {
                            if self.set_lookup(&receiver, &pk)?.is_none() {
                                receiver.set_add(pk)?;
                            }
                        }
                        continue;
                    }
                    // If the arg is already a set, copy its PyKeys directly.
                    if arg.as_set().is_some() {
                        let keys: Vec<PyKey> =
                            arg.set_with(|s| s.iter().cloned().collect()).unwrap_or_default();
                        for pk in keys {
                            if self.set_lookup(&receiver, &pk)?.is_none() {
                                receiver.set_add(pk)?;
                            }
                        }
                        continue;
                    }
                    // General iterable: iterate and hash each element via
                    // value_to_pykey so slices and PyInstances are handled.
                    let items = self.collect_iterable(&arg)?;
                    for item in items {
                        let pk = self.value_to_pykey(&item)?;
                        if self.set_lookup(&receiver, &pk)?.is_none() {
                            receiver.set_add(pk)?;
                        }
                    }
                }
                Ok(Value::none())
            }
            // Set-algebra method forms (issue #1907).  When the receiver or any
            // operand holds user-instance keys, fold through the `__eq__`-aware
            // `set_binary_op` / `set_subset_cmp`; otherwise fall through to the
            // fast interpreter-free builtin path below.
            "union" | "intersection" | "difference" | "symmetric_difference"
            | "issubset" | "issuperset" | "isdisjoint"
                if self.set_algebra_needs_eq(&receiver, &args)? =>
            {
                self.set_algebra_method_eq(method, receiver, args)
            }
            _ => pyrust_builtins::set::call(method, &receiver, args),
        }
    }

    /// `isdisjoint` for the set-like dict views `dict_keys` / `dict_items`
    /// (issue #1891).  Accepts any iterable and returns `True` when no element
    /// of the argument is a member of the view.  Iterating the *argument* (and
    /// probing the view's `__contains__`) — rather than building a set from the
    /// view — matches CPython's `dictviews_isdisjoint`: a `dict_items` view with
    /// unhashable values still works because its own values are never hashed.
    pub(crate) fn dict_view_isdisjoint(
        &mut self,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        let view_name = value_type_name_str(&receiver);
        if args.len() != 1 {
            let n = args.len();
            return Err(pyrust_core::type_err!(
                "{view_name}.isdisjoint() takes exactly one argument ({n} given)"
            ));
        }
        let other = self.collect_iterable(&args[0])?;
        for item in other {
            if self.eval_in(receiver.clone(), item)?.truthy_raw() {
                return Ok(Value::bool_(false));
            }
        }
        Ok(Value::bool_(true))
    }

    /// `frozenset` method dispatch — mirror of [`Self::call_set_method`] for the
    /// frozen variant (issue #1907).  Intercepts the set-algebra method forms
    /// when user `__eq__` is required, folds through the shared
    /// `set_binary_op`/`set_subset_cmp` mechanism, and coerces the algebra
    /// results back to `frozenset` (CPython: `frozenset.union(...)` returns a
    /// `frozenset`).  All other methods (and the all-primitive fast path) go
    /// straight to the interpreter-free builtin implementation.
    pub(crate) fn call_frozenset_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "union" | "intersection" | "difference" | "symmetric_difference"
                if self.set_algebra_needs_eq(&receiver, &args)? =>
            {
                let result = self.set_algebra_method_eq(method, receiver, args)?;
                // `set_algebra_method_eq` returns a `set`; re-freeze for the
                // frozenset receiver (the algebra result type follows CPython).
                match result.set_with(|s| pyrust_builtins::frozenset::frozenset(s.clone())) {
                    Some(fz) => Ok(fz),
                    None => Ok(result),
                }
            }
            "issubset" | "issuperset" | "isdisjoint"
                if self.set_algebra_needs_eq(&receiver, &args)? =>
            {
                self.set_algebra_method_eq(method, receiver, args)
            }
            _ => pyrust_builtins::frozenset::call(method, &receiver, args),
        }
    }

    /// True when a set-algebra method form (`union`/`intersection`/…) must
    /// dispatch user `__eq__`: the receiver or any operand iterable contains a
    /// `PyKey::Object` element (issue #1907).  All-primitive operands return
    /// false and keep the fast interpreter-free builtin path.
    fn set_algebra_needs_eq(&mut self, receiver: &Value, args: &[Value]) -> Result<bool> {
        let recv_has_obj = receiver
            .set_with(set_has_object_key)
            .or_else(|| {
                pyrust_builtins::frozenset::as_items(receiver)
                    .map(|rc| set_has_object_key(&rc))
            })
            .unwrap_or(false);
        if recv_has_obj {
            return Ok(true);
        }
        // Cheap operand scan: detect a user instance (or already-`Object` set
        // key) without building any `PyKey` — keeps the all-primitive method
        // forms (`s.union(other)` etc.) on a borrow-only fast path.
        for arg in args {
            if value_iterable_has_object(arg) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Materialise an arbitrary iterable operand into a deduplicated
    /// `PySet`, dispatching user `__hash__`/`__eq__` so that two
    /// `__eq__`-equal instances collapse to a single entry (issue #1907).
    fn materialize_set_operand(&mut self, arg: &Value) -> Result<PySet> {
        let items = self.collect_iterable(arg)?;
        let mut out: PySet = PySet::default();
        for item in items {
            let pk = self.value_to_pykey(&item)?;
            self.set_insert(&mut out, pk)?;
        }
        Ok(out)
    }

    /// `__eq__`-aware implementation of the set-algebra method forms.  Folds the
    /// receiver against each operand via `set_binary_op` (union/intersection/
    /// difference/symmetric_difference) or `set_subset_cmp` (issubset/
    /// issuperset).  Operands are materialised into `set` values so the shared
    /// `set_binary_op`/`set_subset_cmp` mechanism is reused (issue #1907).
    fn set_algebra_method_eq(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        match method {
            "union" | "intersection" | "difference" => {
                let op = match method {
                    "union" => SetOp::Or,
                    "intersection" => SetOp::And,
                    "difference" => SetOp::Sub,
                    _ => unreachable!(),
                };
                // Start from a plain `set` copy of the receiver (CPython returns
                // a `set` from these method forms, never a frozenset).
                let recv_items = receiver
                    .set_with(|s| s.clone())
                    .or_else(|| {
                        pyrust_builtins::frozenset::as_items(&receiver).map(|rc| (*rc).clone())
                    })
                    .ok_or_else(|| {
                        PyError::named("TypeError", format!("set.{method} receiver is not a set"))
                    })?;
                let mut acc = Value::set(recv_items);
                for arg in &args {
                    let operand = Value::set(self.materialize_set_operand(arg)?);
                    acc = match set_binary_op(self, &acc, &operand, op, method) {
                        Some(r) => r?,
                        None => unreachable!("acc is always a set"),
                    };
                }
                Ok(acc)
            }
            "symmetric_difference" => {
                let arg = args.first().ok_or_else(|| {
                    PyError::Runtime(
                        "set.symmetric_difference() requires 1 argument".to_string(),
                    )
                })?;
                let recv_items = receiver
                    .set_with(|s| s.clone())
                    .or_else(|| {
                        pyrust_builtins::frozenset::as_items(&receiver).map(|rc| (*rc).clone())
                    })
                    .ok_or_else(|| {
                        PyError::named(
                            "TypeError",
                            "set.symmetric_difference receiver is not a set".to_string(),
                        )
                    })?;
                let lhs = Value::set(recv_items);
                let rhs = Value::set(self.materialize_set_operand(arg)?);
                match set_binary_op(self, &lhs, &rhs, SetOp::Xor, method) {
                    Some(r) => r,
                    None => unreachable!("lhs is always a set"),
                }
            }
            "issubset" | "issuperset" => {
                let arg = args.first().ok_or_else(|| {
                    PyError::Runtime(format!("set.{method}() requires 1 argument"))
                })?;
                let recv_items = receiver
                    .set_with(|s| s.clone())
                    .or_else(|| {
                        pyrust_builtins::frozenset::as_items(&receiver).map(|rc| (*rc).clone())
                    })
                    .ok_or_else(|| {
                        PyError::named("TypeError", format!("set.{method} receiver is not a set"))
                    })?;
                let lhs = Value::set(recv_items);
                let rhs = Value::set(self.materialize_set_operand(arg)?);
                let op = if method == "issubset" {
                    BinaryOp::Le
                } else {
                    BinaryOp::Ge
                };
                match set_subset_cmp(self, &lhs, &rhs, op) {
                    Some(r) => r,
                    None => unreachable!("both operands are sets"),
                }
            }
            "isdisjoint" => {
                // `a.isdisjoint(b)` is True when the two sets share no
                // `__eq__`-equal element (issue #1907).  Probe each receiver
                // element against the materialised operand via `set_lookup_in`,
                // which dispatches user `__hash__`-then-`__eq__`.
                let arg = args.first().ok_or_else(|| {
                    PyError::Runtime("set.isdisjoint() requires 1 argument".to_string())
                })?;
                let recv_items = receiver
                    .set_with(|s| s.clone())
                    .or_else(|| {
                        pyrust_builtins::frozenset::as_items(&receiver).map(|rc| (*rc).clone())
                    })
                    .ok_or_else(|| {
                        PyError::named("TypeError", "set.isdisjoint receiver is not a set".to_string())
                    })?;
                let other = self.materialize_set_operand(arg)?;
                for k in recv_items.iter() {
                    if self.set_lookup_in(&other, k)?.is_some() {
                        return Ok(Value::bool_(false));
                    }
                }
                Ok(Value::bool_(true))
            }
            _ => unreachable!("set_algebra_method_eq called with non-algebra method"),
        }
    }

    /// Dispatch any str method.  `join` is handled here to support generators
    /// and any custom iterable via `collect_iterable`; `format_map` is handled
    /// here because it routes through `format_str_template_map`; `format` is
    /// intercepted in the bound-method dispatch path in `calls.rs` (which has
    /// access to kwargs) before reaching this function.  Everything else delegates to
    /// the interpreter-free `pyrust_builtins::string::call`.
    pub(crate) fn call_str_method(
        &mut self,
        method: &str,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        // CPython's str methods accept any str subclass wherever a str argument
        // is expected (#1927).  The receiver-only `pyrust_builtins::string`
        // extractors match an exact `ValueKind::Str`, so coerce str-subclass
        // instances to their backing here before delegating.  Coercion is a
        // cheap no-op for exact-str / non-instance args (the common case) and
        // for non-str-backed instances, so wrong-type args still raise the
        // existing TypeError.  startswith/endswith also accept a *tuple* of str
        // prefixes/suffixes — coerce each element of a tuple arg too.
        let args = coerce_str_subclass_method_args(method, args);
        if method == "format_map" {
            if args.len() != 1 {
                return Err(pyrust_core::type_err!("str.format_map() takes exactly one argument ({} given)",
                        args.len()));
            }
            // Borrow template as &str from the receiver to avoid a heap allocation.
            // receiver is held by value for the lifetime of this block.
            let template: &str = match receiver.kind() {
                ValueKind::Str(s) => s,
                _ => {
                    return Err(pyrust_core::descriptor_requires!("format_map", "str"))
                }
            };
            let mapping = args.into_iter().next().unwrap();
            return self.format_str_template_map(template, mapping);
        }
        if method == "join" {
            if args.len() != 1 {
                return Err(pyrust_core::type_err!("str.join() takes exactly one argument ({} given)",
                        args.len()));
            }
            let iterable = args.into_iter().next().unwrap();
            // Fast paths: types already handled directly by the builtins join fn.
            // Check the tag first (drops the borrow) before deciding whether to
            // call collect_iterable — the borrow from kind() must not overlap
            // with the &mut self borrow that collect_iterable needs.
            let needs_collect = !matches!(
                iterable.kind(),
                ValueKind::List(_)
                    | ValueKind::Tuple(_)
                    | ValueKind::Str(_)
                    | ValueKind::Dict(_)
            );
            let iterable = if needs_collect {
                let items = self.collect_iterable(&iterable).map_err(|e| {
                    // Only rewrite iterator-acquisition TypeErrors as CPython's
                    // "can only join an iterable". TypeErrors raised by user
                    // code inside __iter__/__next__ or a generator body must
                    // propagate unchanged (#576 Copilot review).
                    if is_join_not_iterable_error(&e) {
                        pyrust_core::type_err!("can only join an iterable")
                    } else {
                        e
                    }
                })?;
                Value::list(items.into_iter().map(coerce_str_subclass_arg).collect())
            } else {
                // Fast-path containers (List/Tuple) may hold str-subclass items;
                // CPython joins them by their str value (#1927).  Materialise a
                // coerced copy only when an item actually needs coercing so the
                // common all-exact-str list pays nothing but a scan.
                coerce_str_subclass_join_iterable(iterable)
            };
            return pyrust_builtins::string::call("join", &receiver, vec![iterable]);
        }
        if method == "translate" {
            // Dict fast path: delegate to pyrust-builtins which handles the
            // common `str.maketrans`-produced dict without needing the interpreter.
            if args.len() != 1 {
                return Err(pyrust_core::type_err!("str.translate() takes exactly one argument ({} given)",
                        args.len()));
            }
            let is_dict = matches!(args[0].kind(), ValueKind::Dict(_));
            if is_dict {
                // pyrust-builtins matches mapping values on `ValueKind`
                // directly, so a builtin-subclass replacement value
                // (`class MyInt(int)` / `class MyStr(str)`) would be rejected
                // as a TypeError even though CPython accepts the inherited
                // int/str/None backing. pyrust-builtins has no interpreter
                // access to unwrap `__builtin_data__`, so do it here: when any
                // value needs unwrapping, hand pyrust-builtins a value-coerced
                // copy of the dict. The common `str.maketrans`-produced dict
                // (plain int / str values) needs no copy and pays only a scan.
                let table = args.into_iter().next().unwrap();
                let needs_coerce = |v: &Value| {
                    builtin_data_backing(v)
                        .is_some_and(|b| !matches!(b.kind(), ValueKind::Dict(_)))
                };
                let coerced = table.dict_with(|d| {
                    if d.values().any(&needs_coerce) {
                        let mut out = pyrust_core::PyDict::default();
                        for (k, v) in d.iter() {
                            let coerced_v = builtin_data_backing(v)
                                .filter(|b| !matches!(b.kind(), ValueKind::Dict(_)))
                                .unwrap_or_else(|| v.clone());
                            out.insert(k.clone(), coerced_v);
                        }
                        Some(Value::dict(out))
                    } else {
                        None
                    }
                });
                let table = coerced.flatten().unwrap_or(table);
                return pyrust_builtins::string::call("translate", &receiver, vec![table]);
            }
            // General mapping protocol: call table[ordinal] per codepoint.
            // KeyError / IndexError / LookupError → keep character;
            // None → delete; int → replace with chr(n); str → replace.
            // Materialise chars and reserve capacity under a narrow borrow so
            // that the &str from receiver.kind() drops before eval_index needs
            // a &mut self borrow (they are separate but keep scopes explicit).
            let (chars, out_capacity) = match receiver.kind() {
                ValueKind::Str(s) => (s.chars().collect::<Vec<char>>(), s.len()),
                _ => {
                    return Err(pyrust_core::descriptor_requires!("translate", "str"))
                }
            };
            let table = args.into_iter().next().unwrap();
            let mut out = String::with_capacity(out_capacity);
            for c in chars {
                let cp = Value::int(c as i64);
                match self.eval_index(&table, cp) {
                    Ok(v) => {
                        // Resolve int/str subclass instances to their backing
                        // primitive before the value match. This covers:
                        //   int subclass  → Int/Bool/BigInt backing
                        //   str subclass  → Str backing
                        // A PyInstance without a relevant backing falls through
                        // to the TypeError arm below.
                        let v = builtin_data_backing(&v).unwrap_or(v);
                        match v.kind() {
                            ValueKind::None => { /* delete */ }
                            ValueKind::Int(n) => {
                                if !(0..=0x10FFFF).contains(&n) {
                                    return Err(pyrust_core::value_err!("character mapping must be in range(0x110000)"
                                            .to_string()));
                                }
                                let replacement = char::from_u32(n as u32).ok_or_else(|| {
                                    pyrust_core::value_err!("character mapping must be in range(0x110000)"
                                            .to_string())
                                })?;
                                out.push(replacement);
                            }
                            ValueKind::Bool(b) => {
                                let replacement = char::from_u32(b as u32)
                                    .expect("0 and 1 are valid codepoints");
                                out.push(replacement);
                            }
                            ValueKind::BigInt(n) => {
                                // Use ToPrimitive::to_u32 then char::from_u32 to
                                // validate the range [0, 0x10FFFF] in one step.
                                // A negative or > u32::MAX BigInt yields None from
                                // to_u32(); char::from_u32 rejects surrogates and
                                // values > 0x10FFFF. Both map to the same ValueError.
                                use crate::value::PyToPrimitive;
                                let replacement =
                                    n.to_u32().and_then(char::from_u32).ok_or_else(|| {
                                        pyrust_core::value_err!("character mapping must be in range(0x110000)"
                                                .to_string())
                                    })?;
                                out.push(replacement);
                            }
                            ValueKind::Str(repl) => {
                                out.push_str(repl);
                            }
                            _ => {
                                return Err(pyrust_core::type_err!("character mapping must return integer, None or str"
                                        .to_string()));
                            }
                        }
                    }
                    Err(e)
                        if e.class_name_is("KeyError")
                            || e.class_name_is("IndexError")
                            || e.class_name_is("LookupError") =>
                    {
                        out.push(c);
                    }
                    Err(e) => return Err(e),
                }
            }
            return Ok(Value::string(out));
        }
        pyrust_builtins::string::call(method, &receiver, args)
    }

    /// Dispatch `bytes.join()` with support for generators and arbitrary iterables.
    /// All other bytes methods are handled directly by `pyrust_builtins::bytes::call`.
    pub(crate) fn call_bytes_join(
        &mut self,
        receiver: Value,
        args: Vec<Value>,
    ) -> Result<Value> {
        if args.len() != 1 {
            return Err(pyrust_core::type_err!("bytes.join() takes exactly one argument ({} given)",
                    args.len()));
        }
        let iterable = args.into_iter().next().unwrap();
        let needs_collect = !matches!(
            iterable.kind(),
            ValueKind::List(_) | ValueKind::Tuple(_)
        );
        let iterable = if needs_collect {
            let items = self.collect_iterable(&iterable).map_err(|e| {
                if is_join_not_iterable_error(&e) {
                    pyrust_core::type_err!("can only join an iterable")
                } else {
                    e
                }
            })?;
            Value::list(items.into_iter().map(coerce_bytes_subclass_arg).collect())
        } else {
            // List/Tuple fast path may hold bytes-subclass / bytearray items;
            // CPython joins them by their bytes value (#1928).
            coerce_bytes_subclass_join_iterable(iterable)
        };
        pyrust_builtins::bytes::call(
            "join",
            &receiver,
            &[iterable],
            &PyDict::default(),
        )
    }

    pub(crate) fn eval_binary(&mut self, left: Value, op: BinaryOp, right: Value) -> Result<Value> {
        match op {
            BinaryOp::Add => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__add__", "__radd__") {
                    return r;
                }
                self.add(left, right)
            }
            BinaryOp::Sub => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__sub__", "__rsub__") {
                    return r;
                }
                if let Some(r) = set_binary_op(self, &left, &right, SetOp::Sub, "-") {
                    return r;
                }
                self.sub(left, right)
            }
            BinaryOp::Mul => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mul__", "__rmul__") {
                    return r;
                }
                // Before raising TypeError, try __index__ on the count operand
                // when one side is a built-in sequence.  CPython calls
                // PyNumber_AsSsize_t (which invokes __index__) before failing.
                let is_seq = |v: &Value| {
                    matches!(
                        v.kind(),
                        ValueKind::Str(_)
                            | ValueKind::List(_)
                            | ValueKind::Tuple(_)
                            | ValueKind::Bytes(_)
                    )
                };
                let is_py_instance =
                    |v: &Value| matches!(v.kind(), ValueKind::PyInstance(_));
                if is_seq(&left) && is_py_instance(&right) {
                    let right = self.try_index_for_seq_repeat(right)?;
                    return self.mul(left, right);
                }
                if is_seq(&right) && is_py_instance(&left) {
                    let left = self.try_index_for_seq_repeat(left)?;
                    return self.mul(left, right);
                }
                self.mul(left, right)
            }
            BinaryOp::MatMul => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__matmul__", "__rmatmul__") {
                    return r;
                }
                self.matmul(left, right)
            }
            BinaryOp::Div => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__truediv__", "__rtruediv__") {
                    return r;
                }
                // Issue #1204: `div` extracts the scalar backing internally and
                // keeps the original operands for the subclass-named TypeError.
                self.div(left, right)
            }
            BinaryOp::FloorDiv => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__floordiv__", "__rfloordiv__") {
                    return r;
                }
                // Issue #1204: `floor_div` extracts the scalar backing
                // internally and keeps the originals for the TypeError arm.
                self.floor_div(left, right)
            }
            BinaryOp::Mod => {
                // str % args: printf-style formatting (#1393).
                // Must come BEFORE try_dunder_binary so that rhs.__rmod__ is
                // never consulted when lhs is str — CPython's str.__mod__ is
                // never NotImplemented, so the reflected slot must not run
                // (#1472).
                // Also covers str subclasses (PyInstance with Str backing):
                // CPython's tp_as_sequence->sq_remainder for str subclasses
                // still runs str.__mod__, never returning NotImplemented.
                let str_backing = if matches!(left.kind(), ValueKind::Str(_)) {
                    Some(left.clone())
                } else {
                    builtin_data_backing(&left)
                        .filter(|backing| matches!(backing.kind(), ValueKind::Str(_)))
                };
                if let Some(fmt_val) = str_backing {
                    return self.str_printf_format(fmt_val, right);
                }
                // bytes % args / bytearray % args: PEP 461 printf-style
                // formatting (#1883).  Like str, bytes.__mod__ is never
                // NotImplemented, so this must precede try_dunder_binary so
                // rhs.__rmod__ is not consulted.  bytearray % args returns a
                // bytearray (result type follows the left operand); bytes and
                // bytes subclasses return plain bytes.
                let bytes_backing: Option<Vec<u8>> = match left.kind() {
                    ValueKind::Bytes(rc) => Some(rc.to_vec()),
                    _ => builtin_data_backing(&left).and_then(|backing| {
                        match backing.kind() {
                            ValueKind::Bytes(rc) => Some(rc.to_vec()),
                            _ => None,
                        }
                    }),
                };
                if let Some(data) = bytes_backing {
                    return self.bytes_printf_format(&data, right, false);
                }
                if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&left) {
                    return self.bytes_printf_format(&data, right, true);
                }
                if let Some(r) = self.try_dunder_binary(&left, &right, "__mod__", "__rmod__") {
                    return r;
                }
                // Issue #1204: `modulo` extracts the scalar backing internally
                // and keeps the originals for the TypeError arm.
                self.modulo(left, right)
            }
            BinaryOp::Eq => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__eq__", "__eq__") {
                    return r;
                }
                // Containers fall through here: `Value::eq` would call
                // `Rc::ptr_eq` on `PyInstance` elements, missing user
                // `__eq__`.  Route through `values_user_eq` so list /
                // tuple / dict / set element comparison dispatches
                // `__eq__` recursively (issue #436).
                Ok(Value::bool_(self.values_user_eq(&left, &right)?))
            }
            BinaryOp::Ne => {
                // CPython's `slot_tp_richcompare` derives `__ne__` as the logical
                // negation of `__eq__` whenever a class does not define its own
                // `__ne__` (issue #2645).  pyrust resolves the inherited
                // `object.__ne__` instead, which only compares identity and so
                // returns the wrong answer for `a != a` when `a.__eq__` returns
                // `False` (and similar cases).  Only dispatch `__ne__` directly
                // when at least one operand carries a *user-defined* `__ne__`;
                // otherwise fall through to `not (a == b)`, which already runs
                // the full `__eq__` / reflected-`__eq__` / NotImplemented chain.
                if pyinstance_has_user_ne(&left) || pyinstance_has_user_ne(&right) {
                    // At least one operand carries a user-defined `__ne__`, so
                    // mirror CPython's `do_richcompare(Py_NE)` directly rather
                    // than `not __eq__`.  The ordering is: (subtype-priority
                    // reflected,) forward operand's `__ne__`, reflected operand's
                    // `__ne__`, then the identity default `a is not b`.  Each
                    // `__ne__` step (`pyinstance_ne_step`) dispatches a user
                    // `__ne__` outright and treats the inherited default as
                    // `not __eq__` single-sided.  When *both* steps yield
                    // NotImplemented, CPython falls back to identity — NOT to
                    // `not __eq__` (issue #2648): a user `__ne__` returning
                    // NotImplemented must not re-dispatch `__eq__`.
                    let right_first = matches!(
                        (left.kind(), right.kind()),
                        (ValueKind::PyInstance(li), ValueKind::PyInstance(ri))
                            if !Rc::ptr_eq(&li.borrow().class, &ri.borrow().class)
                                && class_is_subclass_of(
                                    &ri.borrow().class,
                                    &li.borrow().class,
                                )
                    );
                    let (first, second) = if right_first {
                        ((&right, &left), (&left, &right))
                    } else {
                        ((&left, &right), (&right, &left))
                    };
                    if let Some(r) = self.pyinstance_ne_step(first.0, first.1) {
                        return r;
                    }
                    if let Some(r) = self.pyinstance_ne_step(second.0, second.1) {
                        return r;
                    }
                    return Ok(Value::bool_(!values_are_identical(&left, &right)));
                }
                // No user `__ne__`: mirror CPython's `slot_tp_richcompare`, which
                // derives `__ne__` as `not __eq__`.  Try the user `__eq__` /
                // reflected-`__eq__` chain first and negate its truthiness, so
                // `b != b` honours a custom `__eq__` returning `False` (the
                // `values_user_eq` identity short-circuit below would wrongly
                // report equal for `a is b`).  `values_user_eq` remains the
                // fallback for container element-wise dispatch (issue #436).
                if let Some(r) = self.try_dunder_binary(&left, &right, "__eq__", "__eq__") {
                    return Ok(Value::bool_(!r?.truthy_raw()));
                }
                Ok(Value::bool_(!self.values_user_eq(&left, &right)?))
            }
            BinaryOp::Lt => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__lt__", "__gt__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(self, &left, &right, BinaryOp::Lt) {
                    return r;
                }
                self.compare(left, right, "<", |o| o.is_lt())
            }
            BinaryOp::Le => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__le__", "__ge__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(self, &left, &right, BinaryOp::Le) {
                    return r;
                }
                self.compare(left, right, "<=", |o| o.is_le())
            }
            BinaryOp::Gt => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__gt__", "__lt__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(self, &left, &right, BinaryOp::Gt) {
                    return r;
                }
                self.compare(left, right, ">", |o| o.is_gt())
            }
            BinaryOp::Ge => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__ge__", "__le__") {
                    return r;
                }
                if let Some(r) = set_subset_cmp(self, &left, &right, BinaryOp::Ge) {
                    return r;
                }
                self.compare(left, right, ">=", |o| o.is_ge())
            }
            BinaryOp::Pow => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__pow__", "__rpow__") {
                    return r;
                }
                // Issue #1204: extract scalar primitive backing so that
                // `MyInt(42) ** 2` works identically to `42 ** 2`.  Keep the
                // original `left`/`right` for the TypeError arm so CPython's
                // subclass-named message (`'C' and 'str'`, #2544) is preserved.
                let cl = coerce_numeric(&left);
                let cr = coerce_numeric(&right);
                // When either operand is complex, use complex
                // exponentiation: z^w = exp(w * ln(z)).  `both_as_complex`
                // returns Ok(Some) only when at least one operand is
                // already a Complex value; pure int/float/bigint pairs
                // route through the canonical NumericOps slot below.
                if let Some(((zr, zi), (wr, wi))) = both_as_complex(&cl, &cr)? {
                    return complex_pow(zr, zi, wr, wi);
                }
                // Canonical numeric `**` via the NumericOps slot table
                // (#458): int**int (BigInt promotion on overflow, #421/#484),
                // the BigInt-exponent OverflowError arms, and the float
                // power path (negative-real → complex, 0.0 ** negative
                // ZeroDivisionError).
                if let Some(result) = dispatch_numeric_binop(BinaryOp::Pow, &cl, &cr) {
                    return result;
                }
                Err(unsupported_operand("** or pow()", &left, &right))
            }
            BinaryOp::BitAnd => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__and__", "__rand__") {
                    return r;
                }
                if let Some(r) = set_binary_op(self, &left, &right, SetOp::And, "&") {
                    return r;
                }
                // CPython keeps the `bool` type for `bool & bool` (only `&`,
                // `|`, `^`; arithmetic like `True + True` yields `int`).  Catch
                // this before `coerce_numeric` collapses Bool → Int below.  A
                // single int operand makes the result `int`, so mixed
                // bool/int falls through to the int path.
                if let (ValueKind::Bool(a), ValueKind::Bool(b)) = (left.kind(), right.kind()) {
                    return Ok(Value::bool_(a & b));
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = value_type_name_str(&left);
                let rt = value_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `&` via the NumericOps slot table (#458):
                // int×int, BigInt cross-type arms (#485), and Bool coercion
                // all in one site.  Float / non-numeric operands return None
                // → operand-type TypeError below.
                if let Some(result) = dispatch_numeric_binop(BinaryOp::BitAnd, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!("unsupported operand type(s) for &: '{lt}' and '{rt}'"))
            }
            BinaryOp::BitOr => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__or__", "__ror__") {
                    return r;
                }
                // PEP 584: dict | dict → new merged dict (right wins on key collision).
                // Covers plain `dict` and PyInstance dict subclasses; PyInstance subclasses
                // with a custom `__or__` were already handled by the dunder path above.
                if let Some(lhs_entries) = dict_entries_from_value(&left) {
                    // A mappingproxy's `|` is `dict.__or__`, so a failing merge
                    // reports a mappingproxy operand as `dict` (CPython 3.12).
                    let left_type = bitor_operand_type_name(&left);
                    let right_type = bitor_operand_type_name(&right);
                    let Some(rhs_entries) = dict_entries_from_value(&right) else {
                        return Err(pyrust_core::type_err!("unsupported operand type(s) for |: '{left_type}' and '{right_type}'"));
                    };
                    // #1914: dedup via user `__eq__` for `PyKey::Object` keys.
                    // `dict_extend_dedup` keeps the raw fast path for the common
                    // all-primitive case; later values win on duplicate keys.
                    let mut merged: PyDict = PyDict::default();
                    self.dict_extend_dedup(&mut merged, lhs_entries)?;
                    self.dict_extend_dedup(&mut merged, rhs_entries)?;
                    return Ok(Value::dict(merged));
                }
                if let Some(r) = set_binary_op(self, &left, &right, SetOp::Or, "|") {
                    return r;
                }
                // PEP 604: `type | type` (and `None | type`, `type | None`,
                // `UnionType | type`, etc.) creates a `types.UnionType`.
                // `None` is coerced to `NoneType` as the union component.
                // At least one operand must be a strict type (PyClass / BuiltinFunction /
                // UnionType): `None | None` has neither side as a type and must raise
                // TypeError, matching CPython 3.12 (`type.__or__` is what makes it work).
                if is_union_operand(&left)
                    && is_union_operand(&right)
                    && (is_strict_type_union_operand(&left) || is_strict_type_union_operand(&right))
                {
                    let lhs = coerce_none_to_nonetype(left);
                    let rhs = coerce_none_to_nonetype(right);
                    return Ok(pyrust_builtins::union_type::make_union_type(lhs, rhs));
                }
                // `None | None`: both operands looked like union components but neither
                // was a type, so CPython raises TypeError with the operand-type message.
                if is_union_operand(&left) && is_union_operand(&right) {
                    let lt = value_type_name_str(&left);
                    let rt = value_type_name_str(&right);
                    return Err(pyrust_core::type_err!("unsupported operand type(s) for |: '{lt}' and '{rt}'"));
                }
                // CPython keeps the `bool` type for `bool | bool`.  Catch this
                // before `coerce_numeric` collapses Bool → Int below; mixed
                // bool/int falls through to the int path.
                if let (ValueKind::Bool(a), ValueKind::Bool(b)) = (left.kind(), right.kind()) {
                    return Ok(Value::bool_(a | b));
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                // A mappingproxy's `|` / `__ror__` is `dict.__or__`, so a failing
                // merge names it `dict` on either side (CPython 3.12).
                let lt = bitor_operand_type_name(&left);
                let rt = bitor_operand_type_name(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `|` via the NumericOps slot table (#458).
                if let Some(result) = dispatch_numeric_binop(BinaryOp::BitOr, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!("unsupported operand type(s) for |: '{lt}' and '{rt}'"))
            }
            BinaryOp::BitXor => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__xor__", "__rxor__") {
                    return r;
                }
                if let Some(r) = set_binary_op(self, &left, &right, SetOp::Xor, "^") {
                    return r;
                }
                // CPython keeps the `bool` type for `bool ^ bool`.  Catch this
                // before `coerce_numeric` collapses Bool → Int below; mixed
                // bool/int falls through to the int path.
                if let (ValueKind::Bool(a), ValueKind::Bool(b)) = (left.kind(), right.kind()) {
                    return Ok(Value::bool_(a ^ b));
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = value_type_name_str(&left);
                let rt = value_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `^` via the NumericOps slot table (#458).
                if let Some(result) = dispatch_numeric_binop(BinaryOp::BitXor, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!("unsupported operand type(s) for ^: '{lt}' and '{rt}'"))
            }
            BinaryOp::LShift => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__lshift__", "__rlshift__") {
                    return r;
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = value_type_name_str(&left);
                let rt = value_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `<<` via the NumericOps slot table (#458):
                // BigInt-exact shift, Int→BigInt promotion, the
                // OverflowError / "0 << huge" saturation, and the
                // ValueError("negative shift count").  A Float / non-int
                // operand returns None → operand-type TypeError below.
                if let Some(result) = dispatch_numeric_binop(BinaryOp::LShift, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!("unsupported operand type(s) for <<: '{lt}' and '{rt}'"))
            }
            BinaryOp::RShift => {
                if let Some(r) = self.try_dunder_binary(&left, &right, "__rshift__", "__rrshift__") {
                    return r;
                }
                // Issue #1204: extract backing for scalar primitive subclasses.
                let lt = value_type_name_str(&left);
                let rt = value_type_name_str(&right);
                let left = coerce_numeric(&left);
                let right = coerce_numeric(&right);
                // Canonical numeric `>>` via the NumericOps slot table (#458):
                // BigInt-exact shift, sign-collapse on huge counts, and the
                // ValueError("negative shift count").
                if let Some(result) = dispatch_numeric_binop(BinaryOp::RShift, &left, &right) {
                    return result;
                }
                Err(pyrust_core::type_err!("unsupported operand type(s) for >>: '{lt}' and '{rt}'"))
            }
            BinaryOp::In => self.eval_in(right, left),
            BinaryOp::NotIn => Ok(Value::bool_(!self.eval_in(right, left)?.truthy_raw())),
            BinaryOp::Is    => Ok(Value::bool_(values_are_identical(&left, &right))),
            BinaryOp::IsNot => Ok(Value::bool_(!values_are_identical(&left, &right))),
            BinaryOp::And | BinaryOp::Or => unreachable!("short-circuit handled earlier"),
        }
    }

    /// `eval_binary` for an augmented assignment (`a op= b`) that fell through
    /// the in-place dunder path (`try_inplace_op` returned `None`).
    ///
    /// CPython formats the operand-type `TypeError` with the *augmented* symbol
    /// (`+=`, `-=`, `**=`, …) rather than the plain binary symbol (`+`, `-`,
    /// `** or pow()`) — see issue #2561.  This rewrites only the
    /// `unsupported operand type(s) for {sym}:` message produced by the plain
    /// `eval_binary` path, leaving the sequence-specific messages
    /// (`can only concatenate …`, `'int' object is not iterable`) untouched, to
    /// match CPython exactly.
    pub(crate) fn eval_binary_aug(
        &mut self,
        left: Value,
        op: BinaryOp,
        right: Value,
    ) -> Result<Value> {
        self.eval_binary(left, op, right).map_err(|e| rewrite_aug_operand_error(op, e))
    }

    fn add(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            return Ok(Value::complex(a.0 + b.0, a.1 + b.1));
        }
        // Representation-substitutability boundary (#2386): a bytearray-subclass
        // operand acts as its inherited bytearray for concatenation.  Unwrap
        // ONLY for the bytearray-snapshot probe below — `as_bytearray_snapshot`
        // does not see through a `PyInstance`, so without this `BA(b'a') +
        // BA(b'b')` would fall through to a TypeError.  A user `__add__`/
        // `__radd__` was already dispatched at `BinaryOp::Add`, so no override
        // gate is needed.  The original `left`/`right` are kept for the error
        // arms below so CPython's subclass-named messages (`can only
        // concatenate list (not "StrSub") to list`) are preserved.
        let ba_left = effective_builtin_receiver(&left, &[])
            .filter(|b| pyrust_builtins::bytearray::as_bytearray_snapshot(b).is_some());
        let ba_right = effective_builtin_receiver(&right, &[])
            .filter(|b| pyrust_builtins::bytearray::as_bytearray_snapshot(b).is_some());
        if ba_left.is_some() || ba_right.is_some() {
            let l = ba_left.unwrap_or_else(|| left.clone());
            let r = ba_right.unwrap_or_else(|| right.clone());
            return self.add(l, r);
        }
        // bytearray concatenation: handle before coerce_numeric since bytearray
        // is a BuiltinObject and would fall through the numeric match arms.
        // CPython 3.12: bytearray + bytearray → bytearray, bytearray + bytes →
        // bytearray, bytes + bytearray → bytes.
        let lhs_ba = pyrust_builtins::bytearray::as_bytearray_snapshot(&left);
        let rhs_ba = pyrust_builtins::bytearray::as_bytearray_snapshot(&right);
        if lhs_ba.is_some() || rhs_ba.is_some() {
            match (lhs_ba, rhs_ba) {
                (Some(a), Some(b)) => {
                    // bytearray + bytearray → bytearray
                    let mut out = a;
                    out.extend_from_slice(&b);
                    return Ok(pyrust_builtins::bytearray::bytearray(out));
                }
                (Some(a), None) => {
                    // bytearray + bytes → bytearray
                    if let ValueKind::Bytes(rc) = right.kind() {
                        let mut out = a;
                        out.extend_from_slice(rc);
                        return Ok(pyrust_builtins::bytearray::bytearray(out));
                    }
                    return Err(pyrust_core::type_err!("can't concat {} to bytearray",
                            pyrust_core::builtin_type_name(&right)));
                }
                (None, Some(b)) => {
                    // bytes + bytearray → bytes (CPython 3.12 parity)
                    if let ValueKind::Bytes(rc) = left.kind() {
                        let mut out = rc.as_ref().clone();
                        out.extend_from_slice(&b);
                        return Ok(Value::bytes(out));
                    }
                    // Non-bytes LHS with bytearray RHS: mirror CPython's
                    // per-type error messages.
                    let lt = value_type_name_str(&left);
                    let err_msg = match left.kind() {
                        ValueKind::Str(_) | ValueKind::List(_) | ValueKind::Tuple(_) => {
                            format!("can only concatenate {lt} (not \"bytearray\") to {lt}")
                        }
                        _ => format!(
                            "unsupported operand type(s) for +: '{lt}' and 'bytearray'"
                        ),
                    };
                    return Err(pyrust_core::type_err!(err_msg));
                }
                (None, None) => unreachable!(),
            }
        }
        // Issue #1939: list/tuple subclasses inherit `__add__`, so `L([1]) +
        // [2]` (and `[1] + L([2])`) concatenate via the backing list and yield
        // a plain `list`.  Extract container backing (a user `__add__`/
        // `__radd__` was already dispatched at `BinaryOp::Add` before reaching
        // here, so no override check is needed); scalar backing continues
        // through `coerce_numeric`.
        let l = coerce_operand_backing(&left);
        let r = coerce_operand_backing(&right);
        // Canonical numeric arithmetic via the NumericOps slot table
        // (issue #458): handles every numeric type pair in one site.
        // Non-numeric operands return None and fall through to the
        // container / concatenation arms below.
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Add, &l, &r) {
            return result;
        }
        match (l.kind(), r.kind()) {
                (ValueKind::Str(a), ValueKind::Str(b)) => Ok(Value::string(format!("{a}{b}"))),
                (ValueKind::List(a), ValueKind::List(b)) => {
                    let mut out = a.to_vec();
                    out.extend_from_slice(&b[..]);
                    Ok(Value::list(out))
                }
                (ValueKind::Tuple(a), ValueKind::Tuple(b)) => {
                    let mut out = a.to_vec();
                    out.extend_from_slice(b);
                    Ok(Value::tuple(out))
                }
                (ValueKind::Bytes(a), ValueKind::Bytes(b)) => {
                    let mut out = a.as_ref().clone();
                    out.extend_from_slice(b);
                    Ok(Value::bytes(out))
                }
                (ValueKind::Bytes(_), _) => Err(pyrust_core::type_err!("can't concat {} to bytes",
                        pyrust_core::builtin_type_name(&right))),
                // CPython sequences (str / list / tuple) report a dedicated
                // "can only concatenate X (not "Y") to X" message when the RHS is
                // not the same sequence type, rather than the generic
                // "unsupported operand type(s)" used for numeric operands.
                (ValueKind::Str(_), _)
                | (ValueKind::List(_), _)
                | (ValueKind::Tuple(_), _) => {
                    // LHS name comes from the coerced sequence (`str` / `list`
                    // / `tuple`, even for subclasses — CPython names the base
                    // sequence type whose concat slot ran); RHS name comes from
                    // the original operand so subclass names are preserved
                    // (e.g. `not "MyInt"`).
                    let lt = value_type_name_str(&l);
                    let rt = value_type_name_str(&right);
                    Err(pyrust_core::type_err!(
                        "can only concatenate {lt} (not \"{rt}\") to {lt}"
                    ))
                }
                _ => Err(unsupported_operand("+", &left, &right)),
        }
    }

    fn sub(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            return Ok(Value::complex(a.0 - b.0, a.1 - b.1));
        }
        let (l, r) = (coerce_numeric(&left), coerce_numeric(&right));
        // Canonical numeric arithmetic via the NumericOps slot table (#458).
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Sub, &l, &r) {
            return result;
        }
        Err(unsupported_operand("-", &left, &right))
    }

    /// Resolve a sequence repetition count through `__index__` when the value
    /// is a PyInstance.  Returns the original value for int/bool/bigint.
    /// Raises `TypeError` if `__index__` returns non-int, or if the instance
    /// has no `__index__` at all, matching CPython 3.12 sequence repetition.
    fn try_index_for_seq_repeat(&mut self, val: Value) -> Result<Value> {
        // CPython's repeat-count message names the *original* object's type
        // (both for the non-index TypeError and the BigInt OverflowError), so
        // capture it before `value_to_index` may resolve through `__index__`.
        let type_name_for_err = value_type_name_str(&val).to_string();
        let resolved = self.value_to_index(&val, |_| {
            pyrust_core::type_err!("can't multiply sequence by non-int of type '{type_name_for_err}'")
        })?;
        // `value_to_index` guarantees Int/Bool/BigInt.  A BigInt count is too
        // large to fit a Py_ssize_t; CPython's PyNumber_AsSsize_t raises
        // OverflowError using the *original* object's type name, not "int".
        if matches!(resolved.kind(), ValueKind::BigInt(_)) {
            return Err(pyrust_core::overflow_err!("cannot fit '{type_name_for_err}' into an index-sized integer"));
        }
        Ok(resolved)
    }

    fn mul(&self, left: Value, right: Value) -> Result<Value> {
        if let Some((a, b)) = both_as_complex(&left, &right)? {
            // (ar+ai*j) * (br+bi*j) = (ar*br - ai*bi) + (ar*bi + ai*br)j
            return Ok(Value::complex(a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0));
        }
        // bytearray * int / int * bytearray — handled before coerce_numeric
        // because bytearray is a BuiltinObject and won't match any explicit arm.
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&left) {
            let n = match right.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(_) => {
                    return Err(pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer"));
                }
                _ => {
                    let type_name = value_type_name_str(&right);
                    return Err(pyrust_core::type_err!("can't multiply sequence by non-int of type '{type_name}'"));
                }
            };
            return seq_repeat_bytearray(&data, n);
        }
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&right) {
            let n = match left.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(_) => {
                    return Err(pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer"));
                }
                _ => {
                    let type_name = value_type_name_str(&left);
                    return Err(pyrust_core::type_err!("can't multiply sequence by non-int of type '{type_name}'"));
                }
            };
            return seq_repeat_bytearray(&data, n);
        }
        // Issue #1939: list/tuple subclasses inherit `__mul__`, so `T((1,)) *
        // 2` repeats via the backing tuple and yields a plain `tuple`.  Extract
        // container backing (a user `__mul__`/`__rmul__` was already dispatched
        // at `BinaryOp::Mul`); scalar backing continues through `coerce_numeric`.
        let l = coerce_operand_backing(&left);
        let r = coerce_operand_backing(&right);
        // Canonical numeric arithmetic via the NumericOps slot table
        // (#458).  Sequence repetition (Str/List/Tuple/Bytes × Int) and
        // the TypeError diagnostics stay below: at least one operand is a
        // sequence there, so `dispatch_numeric_binop` returns None.
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Mul, &l, &r) {
            return result;
        }
        match (l.kind(), r.kind()) {
            (ValueKind::Str(text), ValueKind::Int(n)) => {
                seq_repeat_str(text, n)
            }
            (ValueKind::Int(n), ValueKind::Str(text)) => {
                seq_repeat_str(text, n)
            }
            (ValueKind::Str(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::Str(_)) => Err(pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer")),
            (ValueKind::List(items), ValueKind::Int(n)) => seq_repeat_list(&items, n),
            (ValueKind::Int(n), ValueKind::List(items)) => seq_repeat_list(&items, n),
            (ValueKind::List(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::List(_)) => Err(pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer")),
            (ValueKind::Bytes(data), ValueKind::Int(n)) => seq_repeat_bytes(data, n),
            (ValueKind::Int(n), ValueKind::Bytes(data)) => seq_repeat_bytes(data, n),
            (ValueKind::Bytes(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::Bytes(_)) => Err(pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer")),
            // Tuple * Int / Int * Tuple — checked repeat, MemoryError on
            // overflow (matches CPython 3.12 `tuplerepeat` behaviour).
            (ValueKind::Tuple(items), ValueKind::Int(n)) => {
                seq_repeat_tuple(items, n)
            }
            (ValueKind::Int(n), ValueKind::Tuple(items)) => {
                seq_repeat_tuple(items, n)
            }
            // Tuple * BigInt / BigInt * Tuple — any BigInt is too large to
            // fit in a platform index; CPython raises OverflowError for both
            // positive and negative BigInt values.
            (ValueKind::Tuple(_), ValueKind::BigInt(_))
            | (ValueKind::BigInt(_), ValueKind::Tuple(_)) => Err(pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer")),
            _ => {
                let is_sequence = |v: &Value| {
                    matches!(
                        v.kind(),
                        ValueKind::Str(_)
                            | ValueKind::List(_)
                            | ValueKind::Tuple(_)
                            | ValueKind::Bytes(_)
                    )
                };
                let is_int_like = |v: &Value| {
                    matches!(v.kind(), ValueKind::Int(_) | ValueKind::BigInt(_))
                };
                if is_sequence(&l) && !is_int_like(&r) {
                    let type_name = value_type_name_str(&r);
                    return Err(pyrust_core::type_err!("can't multiply sequence by non-int of type '{type_name}'"));
                }
                if is_sequence(&r) && !is_int_like(&l) {
                    let type_name = value_type_name_str(&l);
                    return Err(pyrust_core::type_err!("can't multiply sequence by non-int of type '{type_name}'"));
                }
                Err(unsupported_operand("*", &left, &right))
            }
        }
    }

    /// Dispatch a single binary method (e.g. `__iadd__`) on a
    /// PyInstance receiver.  Returns `Some(result)` when the method
    /// exists and was called (possibly returning `NotImplemented`),
    /// `None` when the method isn't defined on the class.  Like
    /// `try_dunder_binary`, this routes both user-defined and
    /// `pyrust_module!`-generated class methods through
    /// `invoke_class_method` so Counter's `__iadd__` (a BuiltinFunction
    /// in the class's attr map) participates in `+=` dispatch.
    fn try_call_binary_method(
        &mut self,
        receiver: &Value,
        method: &str,
        other: Value,
    ) -> Result<Option<Value>> {
        let inst = match receiver.kind() {
            ValueKind::PyInstance(i) => Rc::clone(i),
            _ => return Ok(None),
        };
        let class = Rc::clone(&inst.borrow().class);
        let Some(method_value) = lookup_class_attr(&class, method) else {
            return Ok(None);
        };
        if !is_callable_method(&method_value) {
            return Ok(None);
        }
        // Issue #2122: the in-place set / dict / sequence dunder sentinels
        // (`set.__ior__`, `dict.__ior__`, `list.__iadd__`, …) registered on the
        // primitive base classes (#1909-style exposure) are *not* overrides —
        // a subclass that merely inherits them must reach the identity- and
        // type-preserving in-place fallbacks below (issue #1006), not dispatch
        // the base sentinel (which operates on the unwrapped backing value and
        // would drop the subclass type).  A user-defined `__i*__` (a
        // `UserFunction`, e.g. `MySet2.__ior__`) is a genuine override and is
        // dispatched normally.
        if let ValueKind::BuiltinFunction(name) = method_value.kind()
            && matches!(
                name,
                "set.__ior__"
                    | "set.__iand__"
                    | "set.__isub__"
                    | "set.__ixor__"
                    | "dict.__ior__"
                    | "list.__iadd__"
                    | "list.__imul__"
                    | "bytearray.__iadd__"
                    | "bytearray.__imul__"
            ) {
                return Ok(None);
            }
        let self_val = Value::py_instance(Rc::clone(&inst));
        let arg = ExpandedCallArg {
            name: None,
            value: other,
        };
        let result = invoke_class_method(self, method_value, self_val, &[arg])?;
        Ok(Some(result))
    }

    pub(crate) fn try_inplace_op(
        &mut self,
        left: &Value,
        op: BinaryOp,
        right: &Value,
        is_augmented_assign: bool,
    ) -> Result<Option<Value>> {
        // Fast paths for built-in mutable containers: mutate in-place and
        // return the *same* Value (same Rc pointer) so that aliases see the
        // update.  This implements the Python guarantee that `a += b` on a
        // list or set does not rebind aliases.
        //
        // Quick scalar-exit: primitive scalars (Int, Float, Bool, Str, Bytes,
        // BigInt, Complex, None, Ellipsis, Range) cannot have in-place mutation
        // semantics, so return None immediately without dispatching a dunder.
        // This keeps BinOpConst cost near-zero for the common int/float case.
        if matches!(
            left.kind(),
            ValueKind::Int(_)
                | ValueKind::Float(_)
                | ValueKind::Bool(_)
                | ValueKind::Str(_)
                | ValueKind::Bytes(_)
                | ValueKind::BigInt(_)
                | ValueKind::Complex(_, _)
                | ValueKind::None
                | ValueKind::Ellipsis
                | ValueKind::Tuple(_)
                | ValueKind::Range { .. }
                | ValueKind::NotImplemented
        ) {
            return Ok(None);
        }
        // In-place mutation / `__iadd__`-style semantics only apply to a genuine
        // augmented assignment (`a += b`).  A plain binary `+`/`*`/… that the
        // optimizer fused into a const/imm opcode arrives here with
        // `is_augmented_assign == false` (dst != lhs); it must NOT mutate the LHS
        // or extend it.  Bail out so the caller falls through to eval_binary, which
        // applies the correct non-mutating `__add__` semantics (e.g.
        // `list + non-list` raises TypeError instead of extending).  See issue #1874.
        if !is_augmented_assign {
            return Ok(None);
        }
        // mappingproxy is read-only: `mp |= x` is rejected (CPython 3.12), even
        // though `mp | x` produces a merged dict (PEP 584).
        if op == BinaryOp::BitOr && is_mapping_proxy(left) {
            return Err(pyrust_core::type_err!(
                "'|=' is not supported by mappingproxy; use '|' instead"
            ));
        }
        let is_list = matches!(left.kind(), ValueKind::List(_));
        let is_set = matches!(left.kind(), ValueKind::Set(_));
        if is_list {
            match op {
                BinaryOp::Add => {
                    // list += iterable  =>  list.extend(iterable)
                    let items = self.collect_iterable(right)?;
                    left.list_extend(items)?;
                    return Ok(Some(left.clone()));
                }
                BinaryOp::Mul => {
                    // list *= n  =>  repeat in-place
                    let n = match right.kind() {
                        ValueKind::Int(n) => n,
                        ValueKind::Bool(b) => b as i64,
                        _ => return Ok(None), // fall through to TypeError
                    };
                    left.list_with_mut(|items| {
                        if n <= 0 {
                            items.clear();
                        } else {
                            let orig = items.clone();
                            for _ in 1..n {
                                items.extend_from_slice(&orig);
                            }
                        }
                    });
                    return Ok(Some(left.clone()));
                }
                _ => {}
            }
        } else if is_set {
            match op {
                BinaryOp::BitOr | BinaryOp::BitAnd | BinaryOp::Sub | BinaryOp::BitXor => {
                    // set |= / &= / -= / ^= require RHS to be a set or frozenset.
                    // If RHS is neither, raise the CPython-format TypeError directly
                    // (the op symbol must include `=` for in-place operators).
                    let rhs_items = match set_items_from_value(right) {
                        Some((items, _)) => items,
                        None => {
                            let op_sym = match op {
                                BinaryOp::BitOr => "|=",
                                BinaryOp::BitAnd => "&=",
                                BinaryOp::Sub => "-=",
                                BinaryOp::BitXor => "^=",
                                _ => unreachable!(),
                            };
                            let lt = value_type_name_str(left);
                            let rt = value_type_name_str(right);
                            return Err(pyrust_core::type_err!("unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"));
                        }
                    };
                    // Fast path: when no user-instance key is involved, raw
                    // `IndexSet` identity comparison is exact (issue #2244).
                    // Most sets are primitive, so keep this allocation-cheap
                    // path with no `__eq__` dispatch, and — critically — without
                    // adding a second full LHS scan that would regress the
                    // primitive `s |= t` hot loop (issue #2244 perf-neutrality).
                    //
                    // Object-detection rules per op, mirroring the eq-aware
                    // membership the existing dict/set machinery already
                    // implements (a *primitive* key never dispatches `__eq__`
                    // against object keys — `set_lookup_in` only scans object
                    // buckets for object/None/nested-tuple probe keys):
                    //
                    //  - `|=` inserts RHS keys into the LHS. Raw insertion of a
                    //    primitive RHS key is exact regardless of the LHS, so we
                    //    only need to dispatch `__eq__` when the *RHS* holds an
                    //    object key. The LHS is never scanned, and the per-key
                    //    object check is folded into the single insert pass (no
                    //    separate full RHS pre-scan) — this is what keeps the
                    //    `s |= t` hot loop scan-neutral vs the pre-#2244 path.
                    //  - `&=` / `-=` / `^=` test LHS keys against the RHS (and,
                    //    for `^=`, vice versa), so an object key on *either* side
                    //    means raw `contains` would compare by identity and miss
                    //    an `__eq__`-equal element. Those ops already iterate the
                    //    LHS during mutation; the object-check is folded into the
                    //    same single `set_with_mut` borrow (so the backing
                    //    `RefCell` is acquired once) and bails *before* mutating
                    //    to avoid a partial in-place update.
                    let needs_eq = left
                        .set_with_mut(|lhs| {
                            match op {
                                BinaryOp::BitOr => {
                                    // Fold the object-key check into the single
                                    // insert pass (no separate full RHS pre-scan):
                                    // bail on the first object key.  Primitive
                                    // keys inserted before the bail are exact and
                                    // are re-inserted idempotently by the slow
                                    // path, so a partial in-place insert is safe.
                                    for k in &rhs_items {
                                        if key_contains_object(k) {
                                            return true;
                                        }
                                        lhs.insert(k.clone());
                                    }
                                }
                                BinaryOp::BitAnd => {
                                    if set_has_object_key(lhs) || set_has_object_key(&rhs_items) {
                                        return true;
                                    }
                                    lhs.retain(|k| rhs_items.contains(k));
                                }
                                BinaryOp::Sub => {
                                    if set_has_object_key(lhs) || set_has_object_key(&rhs_items) {
                                        return true;
                                    }
                                    for k in &rhs_items {
                                        lhs.shift_remove(k);
                                    }
                                }
                                BinaryOp::BitXor => {
                                    if set_has_object_key(lhs) || set_has_object_key(&rhs_items) {
                                        return true;
                                    }
                                    let mut to_add: Vec<PyKey> = Vec::new();
                                    for k in &rhs_items {
                                        if !lhs.contains(k) {
                                            to_add.push(k.clone());
                                        }
                                    }
                                    lhs.retain(|k| !rhs_items.contains(k));
                                    for k in to_add {
                                        lhs.insert(k);
                                    }
                                }
                                _ => unreachable!(),
                            }
                            false
                        })
                        .unwrap_or(false);
                    if !needs_eq {
                        return Ok(Some(left.clone()));
                    }
                    // Slow path (issue #2244): a user instance is present on
                    // either side, so membership and dedup must dispatch user
                    // `__hash__`/`__eq__`.  Running user code can re-enter and
                    // mutate the receiver, so the backing borrow must not be
                    // held across it: snapshot the LHS, compute the result with
                    // the eq-aware helpers (`set_lookup_in`/`set_insert`), then
                    // replace the receiver's contents in place so aliases see
                    // the update.
                    let lhs = left.set_with(|s| s.clone()).ok_or_else(|| {
                        PyError::Runtime("internal: expected set".to_string())
                    })?;
                    let mut out: PySet = PySet::default();
                    match op {
                        BinaryOp::BitOr => {
                            for k in lhs.iter().chain(rhs_items.iter()) {
                                self.set_insert(&mut out, k.clone())?;
                            }
                        }
                        BinaryOp::BitAnd => {
                            for k in lhs.iter() {
                                if self.set_lookup_in(&rhs_items, k)?.is_some() {
                                    self.set_insert(&mut out, k.clone())?;
                                }
                            }
                        }
                        BinaryOp::Sub => {
                            for k in lhs.iter() {
                                if self.set_lookup_in(&rhs_items, k)?.is_none() {
                                    self.set_insert(&mut out, k.clone())?;
                                }
                            }
                        }
                        BinaryOp::BitXor => {
                            for k in lhs.iter() {
                                if self.set_lookup_in(&rhs_items, k)?.is_none() {
                                    self.set_insert(&mut out, k.clone())?;
                                }
                            }
                            for k in rhs_items.iter() {
                                if self.set_lookup_in(&lhs, k)?.is_none() {
                                    self.set_insert(&mut out, k.clone())?;
                                }
                            }
                        }
                        _ => unreachable!(),
                    }
                    left.set_with_mut(|s| *s = out).ok_or_else(|| {
                        PyError::Runtime("internal: expected set".to_string())
                    })?;
                    return Ok(Some(left.clone()));
                }
                _ => {}
            }
        } else if let Some(data_rc) = pyrust_builtins::bytearray::as_bytearray_rc(left) {
            // bytearray += / bytearray *= — mutate backing Vec in place so
            // that aliases (other variables referencing the same bytearray)
            // also see the change.
            match op {
                BinaryOp::Add => {
                    // The RHS may itself be a bytes/bytearray subclass instance,
                    // so unwrap its backing before extracting the byte slice.
                    let rhs_val = coerce_operand_backing(right);
                    let rhs = if let Some(rhs_data) =
                        pyrust_builtins::bytearray::as_bytearray_snapshot(&rhs_val)
                    {
                        rhs_data
                    } else if let ValueKind::Bytes(rc) = rhs_val.kind() {
                        rc.as_slice().to_vec()
                    } else {
                        let type_name = value_type_name_str(right);
                        return Err(pyrust_core::type_err!("can't concat {type_name} to bytearray"));
                    };
                    data_rc.borrow_mut().extend_from_slice(&rhs);
                    return Ok(Some(left.clone()));
                }
                BinaryOp::Mul => {
                    let n = match right.kind() {
                        ValueKind::Int(n) => n,
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::BigInt(_) => {
                            return Err(pyrust_core::overflow_err!("cannot fit 'int' into an index-sized integer"));
                        }
                        _ => {
                            let type_name = value_type_name_str(right);
                            return Err(pyrust_core::type_err!("can't multiply sequence by non-int of type '{type_name}'"));
                        }
                    };
                    let mut data = data_rc.borrow_mut();
                    if n <= 0 {
                        data.clear();
                    } else {
                        let orig = data.clone();
                        for _ in 1..n {
                            data.extend_from_slice(&orig);
                        }
                    }
                    return Ok(Some(left.clone()));
                }
                _ => {}
            }
        } else if matches!(left.kind(), ValueKind::Dict(_)) && op == BinaryOp::BitOr {
            // PEP 584: dict |= other → in-place update.
            // Plain dict: skip dunder path, go directly to update().
            // For binary | (not augmented assign), only dict-compatible RHS is
            // valid; fall through to eval_binary for the TypeError with correct
            // operand names.  For |= the full dict.update() semantics apply
            // (accepts dicts and iterables of pairs).
            if is_augmented_assign || dict_entries_from_value(right).is_some() {
                // #1914: `|=` must dedup `PyKey::Object` keys via user `__eq__`.
                // `dict_entries_from_value` handles plain dicts and dict
                // subclasses; iterables-of-pairs fall through to update() below.
                if let Some(entries) = dict_entries_from_value(right) {
                    self.dict_extend_value_dedup(left, entries)?;
                    return Ok(Some(left.clone()));
                }
                let empty_kw = PyDict::default();
                pyrust_builtins::dict::call("update", left, vec![right.clone()], &empty_kw)?;
                return Ok(Some(left.clone()));
            }
        }

        let dunder = match op {
            BinaryOp::Add => "__iadd__",
            BinaryOp::Sub => "__isub__",
            BinaryOp::Mul => "__imul__",
            BinaryOp::MatMul => "__imatmul__",
            BinaryOp::Div => "__itruediv__",
            BinaryOp::FloorDiv => "__ifloordiv__",
            BinaryOp::Mod => "__imod__",
            BinaryOp::Pow => "__ipow__",
            BinaryOp::BitAnd => "__iand__",
            BinaryOp::BitOr => "__ior__",
            BinaryOp::BitXor => "__ixor__",
            BinaryOp::LShift => "__ilshift__",
            BinaryOp::RShift => "__irshift__",
            _ => return Ok(None),
        };
        let result = self.try_call_binary_method(left, dunder, right.clone())?;
        if let Some(ref v) = result
            && !is_not_implemented(v) {
                return Ok(result);
            }
        // PEP 584 fallback: PyInstance dict subclass |= other when no `__ior__`
        // was found.  Call update() on the backing dict (so dict_with_mut works)
        // and return `left` to preserve object identity.
        // For binary | (not augmented assign), only dict-compatible RHS is valid;
        // fall through to eval_binary which uses the subclass type name correctly
        // (e.g. 'D' rather than 'dict') in the unsupported-operand TypeError.
        //
        // `result.is_none()` gates this to the no-override case only: if a
        // user-defined `__ior__` *exists* and returned `NotImplemented`, CPython
        // falls back to plain binary `|` (yielding a plain `dict`, dropping the
        // subclass type), so we must let it fall through to `eval_binary` rather
        // than mutate the backing dict in place and return the subclass here (#2639).
        if result.is_none()
            && op == BinaryOp::BitOr
            && let Some(backing) = builtin_data_backing(left)
                    && matches!(backing.kind(), ValueKind::Dict(_))
                        && (is_augmented_assign || dict_entries_from_value(right).is_some()) {
                            // #1914: dedup `PyKey::Object` keys via user `__eq__`.
                            if let Some(entries) = dict_entries_from_value(right) {
                                self.dict_extend_value_dedup(&backing, entries)?;
                                return Ok(Some(left.clone()));
                            }
                            let empty_kw = PyDict::default();
                            pyrust_builtins::dict::call("update", &backing, vec![right.clone()], &empty_kw)?;
                            return Ok(Some(left.clone()));
                        }
        // Issue #1006 + #1007: PyInstance set subclass |= / &= / -= / ^= — when
        // no user-defined __ior__ / __iand__ / __isub__ / __ixor__ was found,
        // fall back to mutating the backing set in-place and returning `left`
        // so the subclass type is preserved (matching CPython's set.__ior__ etc.
        // which mutate self and return self).
        //
        // Also covers frozenset (plain BuiltinObject) and set subclass TypeError:
        // when LHS is set-like but RHS is not, raise the CPython-format TypeError
        // with the `|=:` / `&=:` / etc. symbol directly (returning None would
        // fall through to eval_binary which uses the non-`=` symbol).
        if matches!(
            op,
            BinaryOp::BitOr | BinaryOp::BitAnd | BinaryOp::Sub | BinaryOp::BitXor
        ) {
            // `result.is_none()` gates the subclass-preserving in-place arm to the
            // no-override case only: if a user-defined `__ior__` / `__iand__` /
            // `__isub__` / `__ixor__` *exists* and returned `NotImplemented`,
            // CPython falls back to plain binary `|` / `&` / `-` / `^` (yielding a
            // plain `set`, dropping the subclass type), so we let it fall through
            // to `eval_binary` rather than mutate the backing set in place (#2639).
            // The frozenset `else` branch below always runs (its `result` is always
            // `None` since `try_call_binary_method` no-ops on non-PyInstance left).
            if left.as_py_instance_rc().is_some() {
                if result.is_none()
                    && let Some(backing) = builtin_data_backing(left)
                    && matches!(backing.kind(), ValueKind::Set(_)) {
                        let op_sym = match op {
                            BinaryOp::BitOr => "|=",
                            BinaryOp::BitAnd => "&=",
                            BinaryOp::Sub => "-=",
                            BinaryOp::BitXor => "^=",
                            _ => unreachable!(),
                        };
                        let rhs_items = match set_items_from_value(right) {
                            Some((items, _)) => items,
                            None => {
                                let lt = value_type_name_str(left);
                                let rt = value_type_name_str(right);
                                return Err(pyrust_core::type_err!("unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"));
                            }
                        };
                        backing.set_with_mut(|lhs| match op {
                            BinaryOp::BitOr => {
                                for k in &rhs_items {
                                    lhs.insert(k.clone());
                                }
                            }
                            BinaryOp::BitAnd => {
                                lhs.retain(|k| rhs_items.contains(k));
                            }
                            BinaryOp::Sub => {
                                for k in &rhs_items {
                                    lhs.shift_remove(k);
                                }
                            }
                            BinaryOp::BitXor => {
                                let mut to_add: Vec<PyKey> = Vec::new();
                                for k in &rhs_items {
                                    if !lhs.contains(k) {
                                        to_add.push(k.clone());
                                    }
                                }
                                lhs.retain(|k| !rhs_items.contains(k));
                                for k in to_add {
                                    lhs.insert(k);
                                }
                            }
                            _ => unreachable!(),
                        });
                        return Ok(Some(left.clone()));
                    }
            } else {
                // Plain frozenset (BuiltinObject) — not caught by the is_set
                // branch above (which only matches ValueKind::Set).
                if set_items_from_value(left).is_some() && set_items_from_value(right).is_none()
                {
                    let op_sym = match op {
                        BinaryOp::BitOr => "|=",
                        BinaryOp::BitAnd => "&=",
                        BinaryOp::Sub => "-=",
                        BinaryOp::BitXor => "^=",
                        _ => unreachable!(),
                    };
                    let lt = value_type_name_str(left);
                    let rt = value_type_name_str(right);
                    return Err(pyrust_core::type_err!("unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'"));
                }
            }
        }
        // Issue #2386: PyInstance bytearray subclass `+=` / `*=` — when no
        // user-defined `__iadd__` / `__imul__` was found (the inherited
        // `bytearray.__i*__` sentinels are skipped in `try_call_binary_method`),
        // mutate the backing bytearray in place and return `left` so the subclass
        // type and object identity are preserved (matching CPython's
        // `bytearray.__iadd__` / `__imul__`, which mutate self and return self).
        //
        // `result.is_none()` gates this to the no-override case only: if a
        // user-defined `__iadd__` / `__imul__` *exists* and returned
        // `NotImplemented`, CPython falls back to plain binary `+` / `*` (yielding
        // a plain `bytearray`, dropping the subclass type), so we must let it fall
        // through to `eval_binary_aug` rather than mutate self in place here.
        if result.is_none()
            && matches!(op, BinaryOp::Add | BinaryOp::Mul)
            && let Some(backing) = builtin_data_backing(left)
            && let Some(data_rc) = pyrust_builtins::bytearray::as_bytearray_rc(&backing)
        {
            match op {
                BinaryOp::Add => {
                    // The RHS may itself be a bytes/bytearray subclass instance,
                    // so unwrap its backing before extracting the byte slice.
                    let rhs_val = coerce_operand_backing(right);
                    let rhs = if let Some(rhs_data) =
                        pyrust_builtins::bytearray::as_bytearray_snapshot(&rhs_val)
                    {
                        rhs_data
                    } else if let ValueKind::Bytes(rc) = rhs_val.kind() {
                        rc.as_slice().to_vec()
                    } else {
                        // CPython names the LHS by its actual (subclass) type.
                        let lhs_name = value_type_name_str(left);
                        let type_name = value_type_name_str(right);
                        return Err(pyrust_core::type_err!(
                            "can't concat {type_name} to {lhs_name}"
                        ));
                    };
                    data_rc.borrow_mut().extend_from_slice(&rhs);
                    return Ok(Some(left.clone()));
                }
                BinaryOp::Mul => {
                    let n = match right.kind() {
                        ValueKind::Int(n) => n,
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::BigInt(_) => {
                            return Err(pyrust_core::overflow_err!(
                                "cannot fit 'int' into an index-sized integer"
                            ));
                        }
                        _ => {
                            let type_name = value_type_name_str(right);
                            return Err(pyrust_core::type_err!(
                                "can't multiply sequence by non-int of type '{type_name}'"
                            ));
                        }
                    };
                    let mut data = data_rc.borrow_mut();
                    if n <= 0 {
                        data.clear();
                    } else {
                        let orig = data.clone();
                        for _ in 1..n {
                            data.extend_from_slice(&orig);
                        }
                    }
                    return Ok(Some(left.clone()));
                }
                _ => unreachable!(),
            }
        }
        Ok(None)
    }

    fn matmul(&mut self, left: Value, right: Value) -> Result<Value> {
        // `@` has no built-in numeric implementation, and the full dunder
        // dispatch — forward `__matmul__`, reflected `__rmatmul__`, the
        // same-type skip and subtype-priority rule — already ran in
        // `eval_binary` via `try_dunder_binary` before this fallback is
        // reached.  Re-dispatching the reflected slot here bypassed that rule
        // and wrongly called `__rmatmul__` on a same-type operand after the
        // forward returned `NotImplemented` (#2092).  Just raise the TypeError.
        Err(unsupported_operand("@", &left, &right))
    }



    fn div(&self, left: Value, right: Value) -> Result<Value> {
        // Issue #1204/#2544: coerce scalar primitive subclass backings for the
        // numeric path, but keep the originals for the subclass-named TypeError.
        let cl = coerce_numeric(&left);
        let cr = coerce_numeric(&right);
        if let Some((a, b)) = both_as_complex(&cl, &cr)? {
            // (ar+ai*j) / (br+bi*j) = ((ar*br + ai*bi) + (ai*br - ar*bi)j) / (br^2 + bi^2)
            let denom = b.0 * b.0 + b.1 * b.1;
            if denom == 0.0 {
                return Err(pyrust_core::zerodiv_err!("complex division by zero"));
            }
            return Ok(Value::complex(
                (a.0 * b.0 + a.1 * b.1) / denom,
                (a.1 * b.0 - a.0 * b.1) / denom,
            ));
        }
        // Canonical numeric true division via the NumericOps slot table
        // (#458).  Non-numeric operands return None → TypeError.
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Div, &cl, &cr) {
            return result;
        }
        Err(unsupported_operand("/", &left, &right))
    }

    fn floor_div(&self, left: Value, right: Value) -> Result<Value> {
        // Issue #1204/#2544: coerce scalar primitive subclass backings for the
        // numeric path, but keep the originals for the subclass-named TypeError.
        let cl = coerce_numeric(&left);
        let cr = coerce_numeric(&right);
        // Canonical numeric floor division via the NumericOps slot table
        // (#458): one site handles int/int (with BigInt promotion on
        // i64::MIN overflow, #485), BigInt cross-type arms, and float
        // floor division, plus the ZeroDivisionError wording.
        if let Some(result) = dispatch_numeric_binop(BinaryOp::FloorDiv, &cl, &cr) {
            return result;
        }
        Err(unsupported_operand("//", &left, &right))
    }

    fn modulo(&self, left: Value, right: Value) -> Result<Value> {
        // Issue #1204/#2544: coerce scalar primitive subclass backings for the
        // numeric path, but keep the originals for the subclass-named TypeError.
        let cl = coerce_numeric(&left);
        let cr = coerce_numeric(&right);
        // Canonical numeric modulo via the NumericOps slot table (#458).
        if let Some(result) = dispatch_numeric_binop(BinaryOp::Mod, &cl, &cr) {
            return result;
        }
        Err(unsupported_operand("%", &left, &right))
    }

    fn compare(
        &self,
        left: Value,
        right: Value,
        op_name: &str,
        cmp: impl Fn(std::cmp::Ordering) -> bool,
    ) -> Result<Value> {
        // Issue #1204: extract scalar primitive backing for subclasses of
        // int/float/str/bytes so that `MyInt(5) < 10` etc. works.
        // Issue #1939: also extract container backing (list/tuple/dict/set
        // subclasses) so `L([1]) < L([2])` compares the backing lists.  A user
        // comparison override was already dispatched at the `BinaryOp::Lt..Ge`
        // sites via `try_dunder_binary` before reaching `compare`, so an empty
        // override list is safe here.
        let left = coerce_operand_backing(&left);
        let right = coerce_operand_backing(&right);
        if matches!(left.kind(), ValueKind::Float(f) if f.is_nan())
            || matches!(right.kind(), ValueKind::Float(f) if f.is_nan())
        {
            return Ok(Value::bool_(false));
        }
        Ok(Value::bool_(cmp(compare_values_with_op(&left, &right, op_name)?)))
    }

    /// Resolve one slice bound through the `__index__` protocol if needed.
    ///
    /// Used for the built-in sequence path (List/Tuple/Str/Bytes) where the
    /// caller needs a concrete integer from each bound.  `PyInstance` and
    /// `BuiltinObject` targets receive the raw (unresolved) bound values so
    /// that user `__getitem__` implementations see the original objects — the
    /// same as CPython: `a[Index(2):]` calls `list.__getitem__` which then
    /// applies `__index__`; `my_obj[Index(2):]` delivers `slice(Index(2),
    /// None, None)` unchanged to `my_obj.__getitem__`.
    ///
    /// `None` (a missing bound, e.g. `a[:]`) and Python `None` are passed
    /// through as-is.  `Int`, `Bool`, and `BigInt` are returned unchanged.
    /// `PyInstance` values that define `__index__` are called and the integer
    /// result is returned.  Anything else is left to `slice_index_from_value`
    /// to reject with a proper TypeError.
    fn resolve_slice_bound_val(&mut self, val: Option<Value>) -> Result<Option<Value>> {
        let v = match val {
            None => return Ok(None),
            Some(v) => v,
        };
        // Fast path: already an integer type or Python None — no protocol call needed.
        if v.is_none()
            || matches!(
                v.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return Ok(Some(v));
        }
        // Slow path: try __index__.
        let resolved = self.resolve_index_arg(v)?;
        Ok(Some(resolved))
    }

    /// Resolve a value assigned to a bytearray element (`ba[i] = v`) through
    /// the `__index__` protocol (#1908).  A `PyInstance` defining `__index__`
    /// is called and its integer result returned (`__index__ returned non-int`
    /// on a bad return).  Every other value — including plain ints, floats, and
    /// `__int__`-only objects — is returned unchanged so the receiver-side
    /// `value_to_byte` produces the correct range / type error verbatim.
    fn resolve_byte_value(&mut self, v: Value) -> Result<Value> {
        if !matches!(v.kind(), ValueKind::PyInstance(_)) {
            return Ok(v);
        }
        // Route the `__index__` dispatch through the shared protocol (#2022).
        // The `NotIndex` sentinel maps "instance isn't integer-like" back to the
        // original value, so the receiver-side `value_to_byte` raises the
        // canonical range / type error.
        match self.value_to_index(&v, |_| PyError::named("__pyrust_NotIndex__", String::new())) {
            Ok(resolved) => Ok(resolved),
            Err(PyError::Named(name, _)) if name == "__pyrust_NotIndex__" => Ok(v),
            Err(e) => Err(e),
        }
    }

    fn eval_slice(&mut self, target: &Value, lo: Option<Value>, hi: Option<Value>, st: Option<Value>) -> Result<Value> {
        // PyInstance: dispatch __getitem__ with a slice object built from the
        // raw (unresolved) bounds.  CPython passes the bound objects as-is so
        // that the user's __getitem__ sees them; resolution via __index__ is
        // the caller's responsibility (e.g. when the user delegates back to a
        // built-in sequence).
        if let ValueKind::PyInstance(inst) = target.kind() {
            let inst_rc = Rc::clone(inst);
            // Issue #994: if the instance has a backing primitive value
            // (tuple/frozenset/dict/list/set subclass), delegate slice to it.
            // eval_index does the same for integer subscripts; without this,
            // `MyTuple([1,2,3])[1:3]` reaches the __getitem__ branch and
            // raises TypeError because tuple subclasses don't register a
            // user-level __getitem__.
            // Issue #1134: check user __getitem__ before backing fast path,
            // matching the same ordering fix in eval_index.  The builtin
            // sentinels for the base types are not overrides.
            let class = Rc::clone(&inst_rc.borrow().class);
            let user_getitem = lookup_class_attr(&class, "__getitem__").filter(|v| {
                !matches!(
                    v.kind(),
                    ValueKind::BuiltinFunction(
                        "dict.__getitem__"
                            | "list.__getitem__"
                            | "tuple.__getitem__"
                            | "bytes.__getitem__"
                    )
                )
            });
            if let Some(method_val) = user_getitem {
                let slice_val = pyrust_builtins::slice::make_slice(lo, hi, st);
                return invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[ExpandedCallArg {
                        name: None,
                        value: slice_val,
                    }],
                );
            }
            if let Some(backing) = builtin_data_backing(target) {
                return self.eval_slice(&backing, lo, hi, st);
            }
            return Err(pyrust_core::type_err!("'{}' object is not subscriptable", class.borrow().name));
        }

        // BuiltinObject: delegate to ops.get_item with a slice value (issue #847).
        // This mirrors what eval_index does when a runtime slice object is used
        // as a subscript, and lets BuiltinObject types opt into slice subscripting
        // via BuiltinTypeOps::get_item.  bytearray's receiver-only get_item can't
        // reach user dunders, so resolve any __index__ bounds here first (#1908);
        // other BuiltinObject types resolve internally so are passed raw.
        if let ValueKind::BuiltinObject { ops, .. } = target.kind()
            && ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME
        {
            let lo = self.resolve_slice_bound_val(lo)?;
            let hi = self.resolve_slice_bound_val(hi)?;
            let st = self.resolve_slice_bound_val(st)?;
            let slice_val = pyrust_builtins::slice::make_slice(lo, hi, st);
            let ValueKind::BuiltinObject { ops, state } = target.kind() else {
                unreachable!("target kind checked above");
            };
            return ops.get_item(state, &slice_val);
        }
        if let ValueKind::BuiltinObject { ops, state } = target.kind() {
            let slice_val = pyrust_builtins::slice::make_slice(lo, hi, st);
            return ops.get_item(state, &slice_val);
        }

        // Range slicing: compute the result range arithmetically, matching
        // CPython's range.__getitem__ for slice arguments.  Handled before
        // the general built-in sequence path so we never materialise elements.
        //
        // CPython's algorithm (Objects/rangeobject.c):
        //   (sl_start, sl_stop, sl_step) = slice.indices(len(r))
        //   new_start = r.start + sl_start * r.step
        //   new_stop  = r.start + sl_stop  * r.step  ← note: uses r.start, not new_start
        //   new_step  = r.step  * sl_step
        if let ValueKind::Range { start: r_start, stop: r_stop, step: r_step } = target.kind() {
            let lo = self.resolve_slice_bound_val(lo)?;
            let hi = self.resolve_slice_bound_val(hi)?;
            let st = self.resolve_slice_bound_val(st)?;
            let r_len = range_len(r_start, r_stop, r_step);
            let (sl_start, sl_stop, sl_step) =
                Self::resolve_slice_bounds(r_len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
            let new_start = r_start + sl_start * r_step;
            let new_stop  = r_start + sl_stop  * r_step;
            let new_step  = r_step  * sl_step;
            return Ok(Value::range(new_start, new_stop, new_step));
        }
        // Arbitrary-precision range slicing (#2118).  CPython resolves the slice
        // indices against the range length as a Python int (not Py_ssize_t), so
        // `range(10**20)[:5]` slices fine even though the length overflows i64.
        // All of len / slice-index resolution / the new start·stop·step run in
        // BigInt arithmetic.
        if let ValueKind::BigRange { start: r_start, stop: r_stop, step: r_step } = target.kind() {
            let r_start = r_start.clone();
            let r_step = r_step.clone();
            let r_len = pyrust_core::bigrange_len(&r_start, r_stop, &r_step);
            let lo = self.resolve_slice_bound_val(lo)?;
            let hi = self.resolve_slice_bound_val(hi)?;
            let st = self.resolve_slice_bound_val(st)?;
            let (sl_start, sl_stop, sl_step) =
                Self::resolve_slice_bounds_big(&r_len, lo.as_ref(), hi.as_ref(), st.as_ref())?;
            let new_start = &r_start + &sl_start * &r_step;
            let new_stop = &r_start + &sl_stop * &r_step;
            let new_step = &r_step * &sl_step;
            return Ok(Value::range_big(new_start, new_stop, new_step));
        }

        // Built-in sequences: resolve bounds through __index__ before applying
        // the integer arithmetic in resolve_slice_bounds (issue #849).
        let lo = self.resolve_slice_bound_val(lo)?;
        let hi = self.resolve_slice_bound_val(hi)?;
        let st = self.resolve_slice_bound_val(st)?;

        // For str, the slice bounds are char-based; computing `len` as the char
        // count is O(n), so the contiguous fast path below resolves char->byte
        // offsets in a single forward scan instead of materialising Vec<char>.
        //
        // ASCII fast path (#2032): an all-ASCII string has char index == byte
        // index, so `len` is `s.len()` and every slice/index is direct byte
        // arithmetic — no char scan at all.  ASCII-ness is cached on the string
        // header (#2124), so the check is O(1) and we reuse the flag for both the
        // length and the contiguous/stepped slice arms below.
        let str_is_ascii = target.is_str() && target.str_is_ascii();
        let len = match target.kind() {
            ValueKind::List(items) => items.len() as i64,
            ValueKind::Tuple(items) => items.len() as i64,
            ValueKind::Str(s) if str_is_ascii => s.len() as i64,
            ValueKind::Str(s) => s.chars().count() as i64,
            ValueKind::Bytes(rc) => rc.len() as i64,
            _ => {
                return Err(pyrust_core::type_err!("'{}' object is not subscriptable",
                        pyrust_core::builtin_type_name(target)));
            }
        };
        let (start, end, step) = Self::resolve_slice_bounds(len, lo.as_ref(), hi.as_ref(), st.as_ref())?;

        // Full-slice identity short-circuit (#2277): CPython's `tuple` / `bytes`
        // `__getitem__` return the original object when the resolved slice
        // covers the whole sequence with unit step (`start == 0 && end == len &&
        // step == 1`), so `t[:] is t`, `t[0:len(t)] is t`, `t[::1] is t`,
        // `t[0:100] is t` (stop clamps to len) all hold.  The Rc-shared tuple
        // backing (#2268) and Rc-shared bytes make the clone identity-preserving
        // (`value_id` reads the same obj_id / Rc pointer), so this is cheap and
        // correct.  `list` is excluded — `l[:]` always copies in CPython.
        //
        // `str` is intentionally NOT short-circuited: pyrust strings have no
        // stable object identity even under plain aliasing (`x = s; x is s` is
        // already False), so a full str slice cannot match CPython's `s[:] is s`
        // == True regardless of what this returns.  That is a broader str
        // identity gap tracked separately, not a slice bug.
        if step == 1
            && start == 0
            && end == len
            && matches!(target.kind(), ValueKind::Tuple(_) | ValueKind::Bytes(_))
        {
            return Ok(target.clone());
        }

        // Contiguous fast path: `step == 1` produces the contiguous run
        // `[start, end)` (memcpy for bytes, range clone for list/tuple, zero-copy
        // shared-buffer slice for ASCII str) — see #2066 / #2111 / #2116 / #2136.
        // The body lives in `fast_path.rs::fast_slice_contiguous`.
        if step == 1 {
            return fast_slice_contiguous(target, start, end, str_is_ascii);
        }

        let indices = Self::slice_target_indices(len, start, end, step);

        match target.kind() {
            ValueKind::List(items) => Ok(Value::list(indices.into_iter().map(|ix| items[ix].clone()).collect::<Vec<Value>>())),
            ValueKind::Tuple(items) => Ok(Value::tuple(indices.into_iter().map(|ix| items[ix].clone()).collect::<Vec<Value>>())),
            ValueKind::Str(s) if str_is_ascii => {
                // ASCII fast path (#2032): char index == byte index, so index the
                // bytes directly — no Vec<char> materialisation.
                let bytes = s.as_bytes();
                let out: String = indices.into_iter().map(|ix| bytes[ix] as char).collect();
                Ok(Value::string(out))
            }
            ValueKind::Str(s) => {
                let chars: Vec<char> = s.chars().collect();
                let mut out = String::new();
                for ix in indices {
                    out.push(chars[ix]);
                }
                Ok(Value::string(out))
            }
            ValueKind::Bytes(rc) => {
                Ok(Value::bytes(indices.into_iter().map(|ix| rc[ix]).collect()))
            }
            _ => unreachable!(),
        }
    }

    /// `item in items` for a list/tuple element slice, dispatching user
    /// `__eq__` only when an operand can fire it.
    ///
    /// Single-pass fast scan (mirrors `call_seq_remove`): when `item` itself
    /// cannot fire user `__eq__`, walk the slice once.  While each element is a
    /// scalar (`cannot_user_eq` — a tag-only check, no `ValueKind` build, no
    /// pointer deref) compare with the primitive `Value::eq`.  On the first
    /// non-scalar element (which might match `item` through its own `__eq__`),
    /// or when `item` can dispatch, snapshot the slice (so a re-entrant user
    /// `__eq__` cannot invalidate the backing store through an alias) and walk
    /// with `values_user_eq`, whose identity short-circuit keeps the mixed
    /// primitive+instance case allocation-light.
    ///
    /// Replaces the previous two-pass shape (a full `needs_dispatch` pre-scan
    /// over every element followed by the membership scan), which was O(n) even
    /// when the match was the first element (#2341).
    fn seq_membership(&mut self, items: &[Value], item: &Value) -> Result<Value> {
        if !Self::value_search_dispatches(item) {
            for elem in items {
                if !elem.cannot_user_eq() {
                    // Non-scalar element: a dispatching element could match
                    // `item` through its own `__eq__`.  Snapshot from the front
                    // and restart on the dispatch path (preserving semantics).
                    let snapshot: Vec<Value> = items.to_vec();
                    for elem in &snapshot {
                        // Identity short-circuit (CPython `PyObject_RichCompareBool`)
                        // before `__eq__` — needed for NaN-bearing complex, which is
                        // non-scalar and so reaches this dispatch branch instead of
                        // the scalar fast path below (#2535).
                        if elem.is_identical_nan(item) || self.values_user_eq(elem, item)? {
                            return Ok(Value::bool_(true));
                        }
                    }
                    return Ok(Value::bool_(false));
                }
                // Identity short-circuit (CPython `PyObject_RichCompareBool`):
                // a NaN searching for itself matches even though `==` is False.
                if elem == item || elem.is_identical_nan(item) {
                    return Ok(Value::bool_(true));
                }
            }
            return Ok(Value::bool_(false));
        }
        // `item` can fire user `__eq__`: snapshot (re-entrancy safety) and walk
        // with full dispatch.
        let snapshot: Vec<Value> = items.to_vec();
        for elem in &snapshot {
            if self.values_user_eq(elem, item)? {
                return Ok(Value::bool_(true));
            }
        }
        Ok(Value::bool_(false))
    }

    pub(crate) fn eval_in(&mut self, container: Value, item: Value) -> Result<Value> {
        // Handle Dict/Set separately so the temporary `&IndexMap`/`&IndexSet`
        // from `container.kind()` doesn't outlive the call into
        // `dict_lookup`/`set_lookup` (which may run user `__eq__`).
        if container.as_dict().is_some() {
            let found = if let Some(s) = item.as_str() {
                self.dict_str_lookup(&container, s)?.is_some()
            } else {
                let key = self.value_to_pykey(&item)?;
                self.dict_lookup(&container, &key)?.is_some()
            };
            return Ok(Value::bool_(found));
        }
        if container.as_set().is_some() {
            let key = self.value_to_pykey(&item)?;
            return Ok(Value::bool_(
                self.set_lookup(&container, &key)?.is_some(),
            ));
        }
        // Frozenset membership — must intercept before the generic BuiltinObject
        // arm because `FrozenSetOps::contains` calls `item.to_key()` which has
        // no interpreter access and cannot dispatch user `__hash__`.  Mirror the
        // Set path above: get the key via `value_to_pykey` (which runs user
        // `__hash__`) then search the underlying `IndexSet` via `set_lookup_in`
        // (which dispatches user `__eq__` for `PyKey::Object` entries).
        if let Some(rc) = pyrust_builtins::frozenset::as_items(&container) {
            let key = self.value_to_pykey(&item)?;
            return Ok(Value::bool_(self.set_lookup_in(&rc, &key)?.is_some()));
        }
        // List and Tuple: `seq_membership` does a single-pass primitive scan
        // and only snapshots + dispatches user `__eq__` when an operand can
        // fire it (see its doc comment for the full contract).
        if let Some(items) = container.as_list() {
            return self.seq_membership(items, &item);
        }
        if let Some(items) = container.as_tuple() {
            return self.seq_membership(items, &item);
        }
        match container.kind() {
            ValueKind::List(_) | ValueKind::Tuple(_) => unreachable!("handled above"),
            ValueKind::Set(_) => unreachable!("handled above"),
            ValueKind::BuiltinObject { ops, state } => {
                // bytearray accepts any bytes-like object (bytes subclass,
                // bytearray) as the left operand of `in` (#1928).  Coerce the
                // item for bytearray; other BuiltinObjects (frozenset) keep the
                // original value so their hashing / equality is unaffected.
                if ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME {
                    let item = coerce_bytes_subclass_arg(item);
                    ops.contains(state, &item).map(Value::bool_)
                } else {
                    ops.contains(state, &item).map(Value::bool_)
                }
            }
            ValueKind::Bytes(rc) => {
                // CPython accepts any bytes-like object (bytes subclass,
                // bytearray) as the left operand of `in` (#1928).  Coerce the
                // item to its `Bytes` backing before the match; non-bytes-like
                // values are returned untouched and hit the error arm.
                let item = coerce_bytes_subclass_arg(item);
                match item.kind() {
                    ValueKind::Int(n) if (0..=255).contains(&n) => Ok(Value::bool_(rc.contains(&(n as u8)))),
                    // bool is a subclass of int in Python; True==1 and False==0 are
                    // valid byte values, so treat them as their integer equivalents.
                    ValueKind::Bool(b) => Ok(Value::bool_(rc.contains(&(if b { 1u8 } else { 0u8 })))),
                    ValueKind::Int(_) | ValueKind::BigInt(_) => Err(pyrust_core::value_err!("byte must be in range(0, 256)")),
                    ValueKind::Bytes(sub) => Ok(Value::bool_(
                        sub.is_empty() || rc.windows(sub.len()).any(|w| w == sub.as_ref().as_slice())
                    )),
                    _ => Err(pyrust_core::type_err!("a bytes-like object is required, not '{}'",
                            value_type_name_str(&item))),
                }
            }
            ValueKind::Str(s) => {
                // CPython accepts any str subclass as the left operand of `in`
                // (#1927).  Coerce the item to its `Str` backing first.
                let item = coerce_str_subclass_arg(item);
                match item.kind() {
                    ValueKind::Str(sub) => Ok(Value::bool_(s.contains(sub))),
                    _ => Err(pyrust_core::type_err!("'in <string>' requires string as left operand, not {}",
                            value_type_name_str(&item))),
                }
            }
            ValueKind::Dict(_) => unreachable!("handled above"),
            ValueKind::Range { start, stop, step } => {
                let range_contains_i64 = |v: i64| -> bool {
                    if step > 0 {
                        v >= start && v < stop && (v - start) % step == 0
                    } else if step < 0 {
                        v <= start && v > stop && (v - start) % step == 0
                    } else {
                        false
                    }
                };
                match item.kind() {
                    ValueKind::Int(v) => Ok(Value::bool_(range_contains_i64(v))),
                    // bool is a subclass of int; True==1, False==0.
                    ValueKind::Bool(b) => Ok(Value::bool_(range_contains_i64(b as i64))),
                    // BigInt: if it fits in i64 apply the check; if it overflows
                    // it cannot be in any range whose bounds are i64.
                    ValueKind::BigInt(n) => {
                        Ok(Value::bool_(n.to_i64().is_some_and(range_contains_i64)))
                    }
                    // Float: if the value is an integer-valued finite float,
                    // convert to i64 and do the fast O(1) range check.
                    // Non-integer or non-finite floats cannot equal any integer.
                    // This matches CPython 3.12's range.__contains__ behaviour.
                    //
                    // Bounds are checked before casting to avoid Rust's saturating
                    // f64-to-i64 cast.  float(2**63) and float(2**63-1) are the same
                    // f64 value (both round to 9.223372036854776e18), so the round-trip
                    // check `(f as i64) as f64 == f` does NOT detect saturation at the
                    // positive boundary.  Use strict half-open bounds instead:
                    // i64 range is [-2**63, 2**63), both endpoints are exact f64 values.
                    ValueKind::Float(f) => {
                        // 9223372036854775808.0 == 2**63 as f64 (exactly representable)
                        const I64_MIN_F: f64 = i64::MIN as f64;
                        const I64_MAX_PLUS1_F: f64 = 9_223_372_036_854_775_808.0_f64;
                        let in_range = f.is_finite()
                            && f.fract() == 0.0
                            && (I64_MIN_F..I64_MAX_PLUS1_F).contains(&f)
                            && range_contains_i64(f as i64);
                        Ok(Value::bool_(in_range))
                    }
                    // Complex: if imaginary part is zero and real part is an
                    // integer-valued finite float, same fast O(1) check.
                    ValueKind::Complex(re, im) => {
                        const I64_MIN_F: f64 = i64::MIN as f64;
                        const I64_MAX_PLUS1_F: f64 = 9_223_372_036_854_775_808.0_f64;
                        let in_range = im == 0.0
                            && re.is_finite()
                            && re.fract() == 0.0
                            && (I64_MIN_F..I64_MAX_PLUS1_F).contains(&re)
                            && range_contains_i64(re as i64);
                        Ok(Value::bool_(in_range))
                    }
                    _ => Ok(Value::bool_(false)),
                }
            }
            ValueKind::BigRange { start, stop, step } => {
                // Arbitrary-precision range membership (#2118).  Mirrors the i64
                // O(1) check but in BigInt arithmetic: `v` is a member iff it lies
                // within [start, stop) (for positive step, or (stop, start] for
                // negative) and `(v - start)` is divisible by `step`.
                let bigrange_contains = |v: &PyBigInt| -> bool {
                    use pyrust_core::PyBigIntSign;
                    let sgn = step.sign();
                    let in_bounds = if sgn == PyBigIntSign::Plus {
                        v >= start && v < stop
                    } else {
                        v <= start && v > stop
                    };
                    in_bounds && ((v - start) % step).sign() == PyBigIntSign::NoSign
                };
                // Resolve `item` to an integer value when possible (int/bool/bigint,
                // or an integer-valued finite float/complex).  Anything else is a
                // non-member.
                use num_traits::FromPrimitive;
                let int_valued_float = |f: f64| -> Option<PyBigInt> {
                    if f.is_finite() && f.fract() == 0.0 {
                        PyBigInt::from_f64(f)
                    } else {
                        None
                    }
                };
                // A float-literal pattern (`Complex(re, 0.0)`) would trigger the
                // deprecated `illegal_floating_point_literal_pattern` lint, so
                // keep the equality guard despite `redundant_guards`.
                #[allow(clippy::redundant_guards)]
                let as_int: Option<PyBigInt> = match item.kind() {
                    ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
                        value_to_bigint(&item)
                    }
                    ValueKind::Float(f) => int_valued_float(f),
                    ValueKind::Complex(re, im) if im == 0.0 => int_valued_float(re),
                    _ => None,
                };
                Ok(Value::bool_(as_int.is_some_and(|v| bigrange_contains(&v))))
            }
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__contains__") {
                    let result = invoke_class_method(
                        self,
                        method_val,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[ExpandedCallArg {
                            name: None,
                            value: item.clone(),
                        }],
                    )?;
                    return Ok(Value::bool_(result.truthy_raw()));
                }
                // list/dict/set subclass with no user-defined __contains__:
                // delegate to the backing primitive, matching CPython's
                // inherited tp_sq_contains / sq_contains slot behaviour.
                if let Some(backing) = builtin_data_backing(&container) {
                    return self.eval_in(backing, item);
                }
                // No __contains__ or __builtin_data__: fall back to __iter__ if available.
                if let Some(iter_method) = lookup_class_attr(&class, "__iter__") {
                    let iter_obj = invoke_class_method(
                        self,
                        iter_method,
                        Value::py_instance(Rc::clone(&inst_rc)),
                        &[],
                    )?;
                    loop {
                        match self.call_next(&iter_obj, None) {
                            Ok(elem) => {
                                if self.values_user_eq(&elem, &item)? {
                                    return Ok(Value::bool_(true));
                                }
                            }
                            Err(ref e) if e.class_name_is("StopIteration") => {
                                return Ok(Value::bool_(false));
                            }
                            // class_name_is walks the hierarchy for Raised variants;
                            // subclasses of StopIteration are caught by the arm above.
                            // Any other Raised exception propagates.
                            Err(PyError::Raised(exc)) => return Err(PyError::Raised(exc)),
                            Err(e) => return Err(e),
                        }
                    }
                }
                // Legacy sequence-iter protocol (#394): if the class
                // defines `__getitem__` but no `__iter__`/`__contains__`,
                // walk indices 0, 1, … until IndexError/StopIteration.
                // **Short-circuits** on first match (#416 Copilot
                // review): the lazy iterator stops calling
                // `__getitem__` past the matching index, so a later
                // index raising `RuntimeError` doesn't surface.
                if lookup_class_attr(&class, "__getitem__").is_some() {
                    let iter_val = self.make_getitem_iter(Rc::clone(&inst_rc))?;
                    loop {
                        match self.call_next(&iter_val, None) {
                            Ok(elem) => {
                                if self.values_user_eq(&elem, &item)? {
                                    return Ok(Value::bool_(true));
                                }
                            }
                            Err(ref e) if e.class_name_is("StopIteration") => {
                                return Ok(Value::bool_(false));
                            }
                            // class_name_is walks the hierarchy; any remaining
                            // Raised is not StopIteration or a subclass.
                            Err(PyError::Raised(exc)) => return Err(PyError::Raised(exc)),
                            Err(e) => return Err(e),
                        }
                    }
                }
                Err(pyrust_core::type_err!("argument of type '{}' is not iterable", class.borrow().name))
            }
            // Generators and every native iterator object (map/filter/zip/
            // enumerate/reversed/iter(...)/itertools iterators — all carried by
            // the Generator value tag): CPython's last-resort sq_contains walks
            // the iterator lazily with `__eq__`, short-circuiting on first
            // match (and consuming it up to the hit).  Coroutines and async
            // generators share the tag but are not iterable — they fall through
            // to the TypeError below with their own type name.
            ValueKind::Generator(_)
                if !crate::builtin_modules::builtins::is_coroutine_value(&container)
                    && !crate::builtin_modules::builtins::is_async_generator_value(&container) =>
            {
                loop {
                    match self.call_next(&container, None) {
                        Ok(elem) => {
                            if self.values_user_eq(&elem, &item)? {
                                return Ok(Value::bool_(true));
                            }
                        }
                        Err(ref e) if e.class_name_is("StopIteration") => {
                            return Ok(Value::bool_(false));
                        }
                        // class_name_is walks the hierarchy for Raised variants;
                        // subclasses of StopIteration are caught by the arm above.
                        // Any other Raised exception propagates.
                        Err(PyError::Raised(exc)) => return Err(PyError::Raised(exc)),
                        Err(e) => return Err(e),
                    }
                }
            }
            // A class whose metaclass defines `__contains__` (e.g. an `Enum`
            // subclass under `EnumMeta`): `member in Color` dispatches the
            // metaclass slot with the class as the receiver (#2611).
            ValueKind::PyClass(cls)
                if metaclass_dunder(cls, "__contains__").is_some() =>
            {
                let method_val = metaclass_dunder(cls, "__contains__").unwrap();
                let result = invoke_class_method(
                    self,
                    method_val,
                    Value::py_class(Rc::clone(cls)),
                    &[ExpandedCallArg {
                        name: None,
                        value: item.clone(),
                    }],
                )?;
                Ok(Value::bool_(result.truthy_raw()))
            }
            // Scalar non-iterables (int/float/bool/bigint/complex/None …) reach
            // here.  CPython raises `TypeError: argument of type '<type>' is not
            // iterable` with the operand's type name — matching the PyInstance
            // arm above (issue #2030); a bare `RuntimeError` escaped before.
            _ => Err(pyrust_core::type_err!(
                "argument of type '{}' is not iterable",
                value_type_name_str(&container)
            )),
        }
    }

    /// Coerce a `PyInstance` argument to its int backing (for int subclasses) or
    /// call `__index__` (for objects that define it), ready for integer printf
    /// format codes (`%d`, `%i`, `%u`, `%o`, `%x`, `%X`).
    ///
    /// Non-`PyInstance` values are returned unchanged; `str_printf_to_int` will
    /// handle them (or raise `TypeError`) as before.  This mirrors CPython's
    /// `PyNumber_Index` pre-coercion that happens before `formatlong`.
    fn coerce_printf_int_arg(&mut self, val: Value) -> Result<Value> {
        // Use a tag enum so the borrow from val.kind() ends before we move val.
        enum Tag {
            Instance(Rc<RefCell<PyInstance>>),
            Other,
        }
        let tag = match val.kind() {
            ValueKind::PyInstance(inst) => Tag::Instance(Rc::clone(inst)),
            _ => Tag::Other,
        };
        let inst_rc = match tag {
            Tag::Other => return Ok(val),
            Tag::Instance(rc) => rc,
        };
        // Int subclass: extract the backing primitive (Int or BigInt).
        if let Some(backing) = builtin_data_backing(&val)
            && matches!(backing.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
                return Ok(backing);
            }
        // Non-int-subclass: look for __index__.
        let class = Rc::clone(&inst_rc.borrow().class);
        let Some(method_val) = lookup_class_attr(&class, "__index__") else {
            // No backing and no __index__: return original; str_printf_to_int
            // will produce the correct TypeError.
            return Ok(val);
        };
        let result = invoke_class_method(
            self,
            method_val,
            Value::py_instance(Rc::clone(&inst_rc)),
            &[],
        )?;
        // CPython: if __index__ returns non-int, the printf format code falls
        // back to its standard error ("a real number is required, not Foo").
        // Return val unchanged so str_printf_to_int produces the right message.
        let is_int = matches!(
            result.kind(),
            ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
        );
        if is_int { Ok(result) } else { Ok(val) }
    }

    /// Coerce a `PyInstance` argument to `f64` for float printf format codes
    /// (`%e`, `%E`, `%f`, `%F`, `%g`, `%G`).
    ///
    /// Tries `__float__` first (float subclasses carry a float backing value),
    /// then `__index__` (int-like objects acceptable as float arguments).
    /// Non-`PyInstance` values are returned unchanged.
    fn coerce_printf_float_arg(&mut self, val: Value) -> Result<Value> {
        // Use a tag enum so the borrow from val.kind() ends before we move val.
        enum Tag {
            Instance(Rc<RefCell<PyInstance>>),
            Other,
        }
        let tag = match val.kind() {
            ValueKind::PyInstance(inst) => Tag::Instance(Rc::clone(inst)),
            _ => Tag::Other,
        };
        let inst_rc = match tag {
            Tag::Other => return Ok(val),
            Tag::Instance(rc) => rc,
        };
        // Float or int subclass: extract the backing primitive directly.
        if let Some(backing) = builtin_data_backing(&val)
            && matches!(
                backing.kind(),
                ValueKind::Float(_) | ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            ) {
                return Ok(backing);
            }
        let class = Rc::clone(&inst_rc.borrow().class);
        // Try __float__ first.
        if let Some(method_val) = lookup_class_attr(&class, "__float__") {
            let result = invoke_class_method(
                self,
                method_val,
                Value::py_instance(Rc::clone(&inst_rc)),
                &[],
            )?;
            enum FloatTag { Ok, Err { class_name: String, type_name: String } }
            let ftag = match result.kind() {
                ValueKind::Float(_) => FloatTag::Ok,
                _ => FloatTag::Err {
                    class_name: inst_rc.borrow().class.borrow().name.clone(),
                    type_name: value_type_name_str(&result).to_string(),
                },
            };
            return match ftag {
                FloatTag::Ok => Ok(result),
                FloatTag::Err { class_name, type_name } => Err(pyrust_core::type_err!("{class_name}.__float__ returned non-float (type {type_name})")),
            };
        }
        // Try __index__ as fallback (CPython accepts integer-like objects for %f).
        if let Some(method_val) = lookup_class_attr(&class, "__index__") {
            let result = invoke_class_method(
                self,
                method_val,
                Value::py_instance(Rc::clone(&inst_rc)),
                &[],
            )?;
            enum IdxTag { Ok, Err(String) }
            let itag = match result.kind() {
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => IdxTag::Ok,
                _ => IdxTag::Err(value_type_name_str(&result).to_string()),
            };
            return match itag {
                IdxTag::Ok => Ok(result),
                IdxTag::Err(type_name) => Err(pyrust_core::type_err!("__index__ returned non-int (type {type_name})")),
            };
        }
        // No __float__, no __index__: return original; str_printf_to_float
        // will produce the correct TypeError.
        Ok(val)
    }
    /// Format one `%`-conversion argument — the `match conv` body lifted out
    /// of `str_printf_format`.  `conv_index` is the 0-based source index of
    /// `conv`, used only for the unsupported-conversion error message.
    #[allow(clippy::too_many_arguments)]
    fn str_printf_convert(
        &mut self,
        conv: char,
        arg: Value,
        precision: Option<usize>,
        flag_plus: bool,
        flag_space: bool,
        flag_hash: bool,
        conv_index: usize,
        bytes_mode: bool,
    ) -> Result<String> {
        Ok(match conv {
                's' => apply_str_precision(self.render_value_as_str(&arg)?, precision),
                'r' => apply_str_precision(render_instance_repr(self, &arg)?, precision),
                // %a — ascii repr (like the ascii() builtin); mirrors %r but escapes
                // non-ASCII. The bytes path already implements this; the str path was
                // missing the arm (#2073).
                'a' => apply_str_precision(ascii_repr_interp(self, &arg)?, precision),
                'd' | 'i' | 'u' => {
                    let coerced_int = self.coerce_printf_int_arg(arg)?;
                    match str_printf_to_int(&coerced_int, conv, bytes_mode)? {
                        PrintfInt::Small(n) => {
                            if n < 0 {
                                format!("{}", n)
                            } else if flag_plus {
                                format!("+{}", n)
                            } else if flag_space {
                                format!(" {}", n)
                            } else {
                                format!("{}", n)
                            }
                        }
                        PrintfInt::Big(b) => {
                            // gh-95778: %d/%i/%u render base 10 — subject to
                            // int_max_str_digits (%x/%o are exempt).
                            if pyrust_core::bigint_str_digits_exceed_limit(&b) {
                                return Err(pyrust_core::int_max_str_digits_format_error());
                            }
                            // to_str_radix(10) includes the '-' sign for negatives.
                            let mut s = b.to_str_radix(10);
                            if !s.starts_with('-') && flag_plus {
                                s.insert(0, '+');
                            } else if !s.starts_with('-') && flag_space {
                                s.insert(0, ' ');
                            }
                            s
                        }
                    }
                }
                'o' => {
                    let coerced_int = self.coerce_printf_int_arg(arg)?;
                    match str_printf_to_int(&coerced_int, conv, bytes_mode)? {
                        PrintfInt::Small(n) => {
                            if n < 0 {
                                // CPython uses sign-magnitude (not two's complement) for negative octal.
                                let u = (n as u64).wrapping_neg();
                                if flag_hash {
                                    format!("-0o{:o}", u)
                                } else {
                                    format!("-{:o}", u)
                                }
                            } else if flag_hash {
                                // CPython applies 0o prefix for all values (including 0) when # is set.
                                if flag_plus {
                                    format!("+0o{:o}", n)
                                } else if flag_space {
                                    format!(" 0o{:o}", n)
                                } else {
                                    format!("0o{:o}", n)
                                }
                            } else if flag_plus {
                                format!("+{:o}", n)
                            } else if flag_space {
                                format!(" {:o}", n)
                            } else {
                                format!("{:o}", n)
                            }
                        }
                        PrintfInt::Big(b) => format_printf_bigint_radix(
                            &b, 8, "0o", false, flag_hash, flag_plus, flag_space,
                        ),
                    }
                }
                'x' => {
                    let coerced_int = self.coerce_printf_int_arg(arg)?;
                    match str_printf_to_int(&coerced_int, conv, bytes_mode)? {
                        PrintfInt::Small(n) => {
                            if n < 0 {
                                let u = (n as u64).wrapping_neg();
                                if flag_hash {
                                    format!("-0x{:x}", u)
                                } else {
                                    format!("-{:x}", u)
                                }
                            } else if flag_hash {
                                // CPython applies 0x prefix for all values (including 0) when # is set.
                                if flag_plus {
                                    format!("+0x{:x}", n)
                                } else if flag_space {
                                    format!(" 0x{:x}", n)
                                } else {
                                    format!("0x{:x}", n)
                                }
                            } else if flag_plus {
                                format!("+{:x}", n)
                            } else if flag_space {
                                format!(" {:x}", n)
                            } else {
                                format!("{:x}", n)
                            }
                        }
                        PrintfInt::Big(b) => format_printf_bigint_radix(
                            &b, 16, "0x", false, flag_hash, flag_plus, flag_space,
                        ),
                    }
                }
                'X' => {
                    let coerced_int = self.coerce_printf_int_arg(arg)?;
                    match str_printf_to_int(&coerced_int, conv, bytes_mode)? {
                        PrintfInt::Small(n) => {
                            if n < 0 {
                                let u = (n as u64).wrapping_neg();
                                if flag_hash {
                                    format!("-0X{:X}", u)
                                } else {
                                    format!("-{:X}", u)
                                }
                            } else if flag_hash {
                                // CPython applies 0X prefix for all values (including 0) when # is set.
                                if flag_plus {
                                    format!("+0X{:X}", n)
                                } else if flag_space {
                                    format!(" 0X{:X}", n)
                                } else {
                                    format!("0X{:X}", n)
                                }
                            } else if flag_plus {
                                format!("+{:X}", n)
                            } else if flag_space {
                                format!(" {:X}", n)
                            } else {
                                format!("{:X}", n)
                            }
                        }
                        PrintfInt::Big(b) => format_printf_bigint_radix(
                            &b, 16, "0X", true, flag_hash, flag_plus, flag_space,
                        ),
                    }
                }
                'e' | 'E' => {
                    let coerced_float = self.coerce_printf_float_arg(arg)?;
                    let f = str_printf_to_float(&coerced_float, conv, bytes_mode)?;
                    let prec = precision.unwrap_or(6);
                    let mut s = format_scientific(f, prec, conv == 'E');
                    // Alt-form (#) with precision 0 keeps the decimal point even
                    // though no fractional digits are emitted: "3.e+00" (#2029).
                    // Non-finite values (inf/nan) never get a point.
                    if flag_hash && prec == 0 && f.is_finite()
                        && let Some(e_pos) = s.find(['e', 'E']) {
                            s.insert(e_pos, '.');
                        }
                    if f.is_sign_positive() && flag_plus {
                        s.insert(0, '+');
                    } else if f.is_sign_positive() && flag_space {
                        s.insert(0, ' ');
                    }
                    s
                }
                'f' | 'F' => {
                    let coerced_float = self.coerce_printf_float_arg(arg)?;
                    let f = str_printf_to_float(&coerced_float, conv, bytes_mode)?;
                    let upper = conv == 'F';
                    // Special-case NaN and Inf before calling format!(), which
                    // produces Rust-style 'NaN'/'inf' rather than CPython-style
                    // 'nan'/'inf'/'NAN'/'INF'.
                    let mut s = if f.is_nan() {
                        if upper { "NAN".to_string() } else { "nan".to_string() }
                    } else if f.is_infinite() {
                        if f > 0.0 {
                            if upper { "INF".to_string() } else { "inf".to_string() }
                        } else if upper {
                            "-INF".to_string()
                        } else {
                            "-inf".to_string()
                        }
                    } else {
                        let prec = precision.unwrap_or(6);
                        let mut body = format!("{:.prec$}", f, prec = prec);
                        // Alt-form (#) with precision 0 keeps a trailing decimal
                        // point: "3." rather than "3" (#2029).
                        if flag_hash && prec == 0 {
                            body.push('.');
                        }
                        body
                    };
                    if f.is_sign_positive() && flag_plus {
                        s.insert(0, '+');
                    } else if f.is_sign_positive() && flag_space {
                        s.insert(0, ' ');
                    }
                    s
                }
                'g' | 'G' => {
                    let coerced_float = self.coerce_printf_float_arg(arg)?;
                    let f = str_printf_to_float(&coerced_float, conv, bytes_mode)?;
                    let prec = precision.unwrap_or(6).max(1);
                    let mut s = format_general_float(f, prec, conv == 'G', flag_hash);
                    if f.is_sign_positive() && flag_plus {
                        s.insert(0, '+');
                    } else if f.is_sign_positive() && flag_space {
                        s.insert(0, ' ');
                    }
                    s
                }
                'c' => {
                    // Coerce int subclasses and __index__ objects the same way
                    // as %d/%x etc.  If __index__ returns non-int, we fall back
                    // to the original value so the match below emits the correct
                    // "%c requires int or char" TypeError.
                    let coerced_char = self.coerce_printf_int_arg(arg)?;
                    match coerced_char.kind() {
                        ValueKind::Str(s) => {
                            let mut cs = s.chars();
                            let c = cs.next().ok_or_else(|| {
                                pyrust_core::type_err!("%c requires int or char")
                            })?;
                            if cs.next().is_some() {
                                return Err(pyrust_core::type_err!("%c requires a single character"));
                            }
                            c.to_string()
                        }
                        ValueKind::Int(n) => char::from_u32(n as u32)
                            .ok_or_else(|| {
                                pyrust_core::overflow_err!("%c arg not in range(0x110000)")
                            })?
                            .to_string(),
                        ValueKind::Bool(b) => char::from_u32(b as u32)
                            .ok_or_else(|| {
                                pyrust_core::overflow_err!("%c arg not in range(0x110000)")
                            })?
                            .to_string(),
                        ValueKind::BigInt(b) => {
                            // A BigInt may be in range [0, 0x10ffff] or not.
                            use crate::value::PyToPrimitive;
                            let n = b.to_u32();
                            let c = n
                                .and_then(char::from_u32)
                                .ok_or_else(|| {
                                    pyrust_core::overflow_err!("%c arg not in range(0x110000)")
                                })?;
                            c.to_string()
                        }
                        _ => {
                            return Err(pyrust_core::type_err!("%c requires int or char"));
                        }
                    }
                }
                _ => {
                    return Err(pyrust_core::value_err!("unsupported format character '{}' (0x{:02x}) at index {}",
                            conv,
                            conv as u32,
                            conv_index));
                }
        })
    }


    /// `str % args` — CPython-compatible printf-style string formatting (#1393).
    ///
    /// Handles positional (`%s`, `%d`, …) and named (`%(key)s`) format codes.
    /// The right-hand side may be a single value (implicitly a one-element
    /// positional tuple), a tuple (positional), or a dict (named lookup).
    fn str_printf_format(&mut self, fmt_val: Value, args: Value) -> Result<Value> {
        // Borrow the format string directly from the Value to avoid a heap allocation.
        // fmt_val is held by value for the duration of this function, so the &str is valid.
        let fmt: &str = match fmt_val.kind() {
            ValueKind::Str(s) => s,
            _ => unreachable!("str_printf_format called with non-str left"),
        };

        // CPython mapping mode is triggered by the format string, not by the RHS type.
        // A dict RHS is only used as a mapping when the format string contains %(key) codes;
        // if the format has only positional codes, the dict is treated as a single positional arg.
        let has_named_key = {
            let b = fmt.as_bytes();
            let mut found = false;
            let mut j = 0;
            while j + 1 < b.len() {
                if b[j] == b'%' && b[j + 1] == b'(' {
                    found = true;
                    break;
                }
                j += 1;
            }
            found
        };
        // CPython enters mapping mode when the format has a `%(key)` code and the
        // rhs is a mapping (issue #2089): a `dict`, a `dict` subclass, or any
        // non-tuple/non-str object exposing `__getitem__`.  A plain `dict` keeps
        // the fast `d.get` lookup; a subclass / custom mapping routes the lookup
        // through `__getitem__` so `__missing__` and a custom `KeyError` are
        // honoured.
        let is_mapping = has_named_key && is_percent_format_mapping(&args);
        // Wrap a non-tuple, non-mapping rhs in a virtual single-element tuple.
        // Use &[Value] to avoid cloning the tuple's items upfront; borrow from
        // args directly for the single-value case to avoid an extra clone.
        let positional: Option<&[Value]> = if is_mapping {
            None
        } else {
            match args.kind() {
                ValueKind::Tuple(items) => Some(items),
                _ => Some(std::slice::from_ref(&args)),
            }
        };
        let mut pos_idx: usize = 0;

        let mut out = String::with_capacity(fmt.len());
        let bytes = fmt.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if bytes[i] != b'%' {
                let ch = fmt[i..].chars().next().unwrap();
                out.push(ch);
                i += ch.len_utf8();
                continue;
            }
            i += 1; // consume '%'
            if i >= len {
                return Err(pyrust_core::value_err!("incomplete format"));
            }

            // Named key: %(key)s — borrow a slice of fmt directly to avoid allocating.
            let named_key: Option<&str> = if bytes[i] == b'(' {
                i += 1;
                let start = i;
                while i < len && bytes[i] != b')' {
                    i += 1;
                }
                if i >= len {
                    return Err(pyrust_core::value_err!("incomplete format key"));
                }
                let key = &fmt[start..i];
                i += 1; // consume ')'
                Some(key)
            } else {
                None
            };

            // Flags: -, +, space, #, 0
            let mut flag_minus = false;
            let mut flag_plus = false;
            let mut flag_space = false;
            let mut flag_zero = false;
            let mut flag_hash = false;
            while i < len {
                match bytes[i] {
                    b'-' => flag_minus = true,
                    b'+' => flag_plus = true,
                    b' ' => flag_space = true,
                    b'0' => flag_zero = true,
                    b'#' => flag_hash = true,
                    _ => break,
                }
                i += 1;
            }

            // Width: integer or '*'
            let width: Option<usize> = if i < len && bytes[i] == b'*' {
                i += 1;
                let w = str_printf_take_positional(&positional, &mut pos_idx)?;
                match w.kind() {
                    ValueKind::Int(n) if n >= 0 => Some(n as usize),
                    ValueKind::Int(n) => {
                        flag_minus = true;
                        Some((-n) as usize)
                    }
                    _ => {
                        return Err(pyrust_core::type_err!("* wants int"));
                    }
                }
            } else if i < len && bytes[i].is_ascii_digit() {
                let start = i;
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                Some(fmt[start..i].parse::<usize>().unwrap())
            } else {
                None
            };

            // Precision: .integer or .*
            let precision: Option<usize> = if i < len && bytes[i] == b'.' {
                i += 1;
                if i < len && bytes[i] == b'*' {
                    i += 1;
                    let p = str_printf_take_positional(&positional, &mut pos_idx)?;
                    match p.kind() {
                        ValueKind::Int(n) if n >= 0 => Some(n as usize),
                        ValueKind::Int(_) => Some(0),
                        _ => {
                            return Err(pyrust_core::type_err!("* wants int"));
                        }
                    }
                } else {
                    let start = i;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == start {
                        Some(0)
                    } else {
                        Some(fmt[start..i].parse::<usize>().unwrap())
                    }
                }
            } else {
                None
            };

            // Length modifier: h, l, L — ignored (CPython ignores them too).
            if i < len && matches!(bytes[i], b'h' | b'l' | b'L') {
                i += 1;
            }

            if i >= len {
                return Err(pyrust_core::value_err!("incomplete format"));
            }
            let conv = bytes[i] as char;
            i += 1;

            // %% — literal percent, no argument consumed.
            if conv == '%' {
                out.push('%');
                continue;
            }

            // Get the argument value.
            let arg: Value = if let Some(key) = named_key {
                if is_mapping {
                    match args.kind() {
                        ValueKind::Dict(d) => {
                            let k = PyKey::Str(intern_string(key));
                            match d.get(&k) {
                                Some(v) => v.clone(),
                                None => {
                                    return Err(PyError::key_error(Value::string(key)));
                                }
                            }
                        }
                        // dict subclass / custom mapping: subscript via
                        // `__getitem__` so `__missing__` and a custom `KeyError`
                        // are honoured (issue #2089).
                        _ => self.eval_index(&args, Value::string(key))?,
                    }
                } else {
                    return Err(pyrust_core::type_err!("format requires a mapping"));
                }
            } else {
                str_printf_take_positional(&positional, &mut pos_idx)?
            };

            // Format the argument according to the conversion code.
            let formatted = self.str_printf_convert(
                conv, arg, precision, flag_plus, flag_space, flag_hash, i - 1, false,
            )?;

            // Apply width and alignment.
            let padded = apply_printf_width(formatted, width, flag_minus, flag_zero, conv);
            out.push_str(&padded);
        }

        // Unconsumed positional arguments: raise TypeError.
        if let Some(pos) = positional
            && pos_idx < pos.len() {
                return Err(pyrust_core::type_err!("not all arguments converted during string formatting"));
            }

        Ok(Value::string(out))
    }

    /// `bytes % args` / `bytearray % args` — PEP 461 printf-style formatting
    /// (#1883).  Mirrors [`Self::str_printf_format`]'s flag / width / precision
    /// parser but produces a `bytes` (or `bytearray` when `as_bytearray`) and
    /// applies PEP 461 conversion semantics:
    ///
    /// - `%b` / `%s`: a bytes-like argument (bytes, bytearray, bytes subclass)
    ///   or an object implementing `__bytes__`; a `str` argument is a
    ///   `TypeError`.
    /// - `%a`: the `ascii()` repr of the argument, encoded to bytes.
    /// - `%c`: an integer in `range(256)` or a single byte.
    /// - numeric / float codes (`%d %i %u %o %x %X %e %E %f %F %g %G`): reuse
    ///   the shared [`Self::str_printf_convert`] machinery — the output is pure
    ///   ASCII and identical to the `str %` path.
    fn bytes_printf_format(
        &mut self,
        fmt: &[u8],
        args: Value,
        as_bytearray: bool,
    ) -> Result<Value> {
        // Mapping mode is triggered by a %(key) code in the format string,
        // exactly as in str %; the keys are bytes, not str.
        let has_named_key = {
            let mut found = false;
            let mut j = 0;
            while j + 1 < fmt.len() {
                if fmt[j] == b'%' && fmt[j + 1] == b'(' {
                    found = true;
                    break;
                }
                j += 1;
            }
            found
        };
        // Same mapping rule as the str path (issue #2089); the subclass /
        // custom-mapping lookup routes through `__getitem__` so `__missing__`
        // is honoured.
        let is_mapping = has_named_key && is_percent_format_mapping(&args);
        let positional: Option<&[Value]> = if is_mapping {
            None
        } else {
            match args.kind() {
                ValueKind::Tuple(items) => Some(items),
                _ => Some(std::slice::from_ref(&args)),
            }
        };
        let mut pos_idx: usize = 0;

        let mut out: Vec<u8> = Vec::with_capacity(fmt.len());
        let len = fmt.len();
        let mut i = 0;

        while i < len {
            if fmt[i] != b'%' {
                out.push(fmt[i]);
                i += 1;
                continue;
            }
            i += 1; // consume '%'
            if i >= len {
                return Err(pyrust_core::value_err!("incomplete format"));
            }

            // Named key: %(key)b — keys are bytes.
            let named_key: Option<&[u8]> = if fmt[i] == b'(' {
                i += 1;
                let start = i;
                while i < len && fmt[i] != b')' {
                    i += 1;
                }
                if i >= len {
                    return Err(pyrust_core::value_err!("incomplete format key"));
                }
                let key = &fmt[start..i];
                i += 1; // consume ')'
                Some(key)
            } else {
                None
            };

            // Flags: -, +, space, #, 0
            let mut flag_minus = false;
            let mut flag_plus = false;
            let mut flag_space = false;
            let mut flag_zero = false;
            let mut flag_hash = false;
            while i < len {
                match fmt[i] {
                    b'-' => flag_minus = true,
                    b'+' => flag_plus = true,
                    b' ' => flag_space = true,
                    b'0' => flag_zero = true,
                    b'#' => flag_hash = true,
                    _ => break,
                }
                i += 1;
            }

            // Width: integer or '*'
            let width: Option<usize> = if i < len && fmt[i] == b'*' {
                i += 1;
                let w = str_printf_take_positional(&positional, &mut pos_idx)?;
                match w.kind() {
                    ValueKind::Int(n) if n >= 0 => Some(n as usize),
                    ValueKind::Int(n) => {
                        flag_minus = true;
                        Some((-n) as usize)
                    }
                    _ => {
                        return Err(pyrust_core::type_err!("* wants int"));
                    }
                }
            } else if i < len && fmt[i].is_ascii_digit() {
                let start = i;
                while i < len && fmt[i].is_ascii_digit() {
                    i += 1;
                }
                Some(
                    std::str::from_utf8(&fmt[start..i])
                        .unwrap()
                        .parse::<usize>()
                        .unwrap(),
                )
            } else {
                None
            };

            // Precision: .integer or .*
            let precision: Option<usize> = if i < len && fmt[i] == b'.' {
                i += 1;
                if i < len && fmt[i] == b'*' {
                    i += 1;
                    let p = str_printf_take_positional(&positional, &mut pos_idx)?;
                    match p.kind() {
                        ValueKind::Int(n) if n >= 0 => Some(n as usize),
                        ValueKind::Int(_) => Some(0),
                        _ => {
                            return Err(pyrust_core::type_err!("* wants int"));
                        }
                    }
                } else {
                    let start = i;
                    while i < len && fmt[i].is_ascii_digit() {
                        i += 1;
                    }
                    if i == start {
                        Some(0)
                    } else {
                        Some(
                            std::str::from_utf8(&fmt[start..i])
                                .unwrap()
                                .parse::<usize>()
                                .unwrap(),
                        )
                    }
                }
            } else {
                None
            };

            // Length modifier: h, l, L — ignored (CPython ignores them too).
            if i < len && matches!(fmt[i], b'h' | b'l' | b'L') {
                i += 1;
            }

            if i >= len {
                return Err(pyrust_core::value_err!("incomplete format"));
            }
            let conv = fmt[i] as char;
            i += 1;

            // %% — literal percent, no argument consumed.
            if conv == '%' {
                out.push(b'%');
                continue;
            }

            // Get the argument value.
            let arg: Value = if let Some(key) = named_key {
                if is_mapping {
                    match args.kind() {
                        ValueKind::Dict(d) => {
                            let k = PyKey::Bytes(Rc::new(key.to_vec()));
                            match d.get(&k) {
                                Some(v) => v.clone(),
                                None => {
                                    return Err(PyError::key_error(Value::bytes(key.to_vec())));
                                }
                            }
                        }
                        // dict subclass / custom mapping: subscript via
                        // `__getitem__` (bytes key) so `__missing__` and a custom
                        // `KeyError` are honoured (issue #2089).
                        _ => self.eval_index(&args, Value::bytes(key.to_vec()))?,
                    }
                } else {
                    return Err(pyrust_core::type_err!("format requires a mapping"));
                }
            } else {
                str_printf_take_positional(&positional, &mut pos_idx)?
            };

            // Format the argument according to the conversion code.  PEP 461
            // conversions produce bytes directly; numeric / float codes reuse
            // the shared str printf converter (ASCII output).
            match conv {
                'b' | 's' => {
                    let data = self.bytes_printf_to_bytes(arg)?;
                    let truncated = match precision {
                        Some(p) if data.len() > p => &data[..p],
                        _ => &data[..],
                    };
                    bytes_printf_apply_width(&mut out, truncated, width, flag_minus);
                }
                'a' => {
                    let repr = ascii_repr_interp(self, &arg)?;
                    let bytes = repr.into_bytes();
                    let truncated = match precision {
                        Some(p) if bytes.len() > p => &bytes[..p],
                        _ => &bytes[..],
                    };
                    bytes_printf_apply_width(&mut out, truncated, width, flag_minus);
                }
                'c' => {
                    let byte = self.bytes_printf_to_char(arg)?;
                    bytes_printf_apply_width(&mut out, &[byte], width, flag_minus);
                }
                _ => {
                    let formatted = self.str_printf_convert(
                        conv, arg, precision, flag_plus, flag_space, flag_hash, i - 1, true,
                    )?;
                    let padded = apply_printf_width(formatted, width, flag_minus, flag_zero, conv);
                    out.extend_from_slice(padded.as_bytes());
                }
            }
        }

        // Unconsumed positional arguments: raise TypeError.
        if let Some(pos) = positional
            && pos_idx < pos.len() {
                return Err(pyrust_core::type_err!(
                    "not all arguments converted during bytes formatting"
                ));
            }

        if as_bytearray {
            Ok(pyrust_builtins::bytearray::bytearray(out))
        } else {
            Ok(Value::bytes(out))
        }
    }

    /// Resolve a `%b` / `%s` argument to its bytes content (PEP 461).
    ///
    /// Accepts bytes, bytearray, bytes subclasses, and objects implementing
    /// `__bytes__`.  A `str` argument (or any other type) raises the
    /// CPython 3.12 `TypeError`, which always names `%b` (the canonical code)
    /// even when the source code used the `%s` alias.
    fn bytes_printf_to_bytes(&mut self, arg: Value) -> Result<Vec<u8>> {
        if let ValueKind::Bytes(rc) = arg.kind() {
            return Ok(rc.to_vec());
        }
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&arg) {
            return Ok(data);
        }
        if let Some(inst_rc) = arg.as_py_instance_rc() {
            // bytes subclass: extract the backing bytes directly.
            if let Some(backing) = builtin_data_backing(&arg)
                && let ValueKind::Bytes(rc) = backing.kind() {
                    return Ok(rc.to_vec());
                }
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method) = lookup_class_attr(&class, "__bytes__") {
                let self_val = Value::py_instance(Rc::clone(inst_rc));
                let result = invoke_class_method(self, method, self_val, &[])?;
                return match result.kind() {
                    ValueKind::Bytes(rc) => Ok(rc.to_vec()),
                    _ => Err(pyrust_core::type_err!(
                        "__bytes__ returned non-bytes (type {})",
                        value_type_name_str(&result)
                    )),
                };
            }
        }
        Err(pyrust_core::type_err!(
            "%b requires a bytes-like object, or an object that implements __bytes__, not '{}'",
            value_type_name_str(&arg)
        ))
    }

    /// Resolve a `%c` argument to a single byte (PEP 461).
    ///
    /// Accepts an integer in `range(256)` (`OverflowError` otherwise) or a
    /// single byte / single-byte bytes-like (`TypeError` for multi-byte or
    /// other types).
    fn bytes_printf_to_char(&mut self, arg: Value) -> Result<u8> {
        // Single-byte bytes-like: b"A" or bytes([65]).
        if let ValueKind::Bytes(rc) = arg.kind() {
            return single_byte_or_err(rc);
        }
        if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&arg) {
            return single_byte_or_err(&data);
        }
        // Integer (or __index__): must be in range(256).
        let coerced = self.coerce_printf_int_arg(arg)?;
        match coerced.kind() {
            ValueKind::Int(n) if (0..=255).contains(&n) => Ok(n as u8),
            ValueKind::Bool(b) => Ok(b as u8),
            ValueKind::Int(_) | ValueKind::BigInt(_) => {
                Err(pyrust_core::overflow_err!("%c arg not in range(256)"))
            }
            _ => Err(pyrust_core::type_err!(
                "%c requires an integer in range(256) or a single byte"
            )),
        }
    }

    /// Coerce a set-algebra operand to its `(PySet, frozen)` items.
    ///
    /// Recognises `set` / `frozenset` / `PyInstance` subclasses thereof (via
    /// the free `set_items_from_value`) **and** the set-like dict views
    /// `dict_keys` / `dict_items` (issue #1891).  `dict_values` is *not*
    /// set-like, so it returns `None` (caller falls through to TypeError).
    ///
    /// - `None`         — operand is not a set-like type.
    /// - `Some(Ok(..))` — coerced items; `frozen` is `false` for views (CPython
    ///   set ops on views return a `set`).
    /// - `Some(Err(..))`— operand is set-like but coercion failed, e.g. a
    ///   `dict_items` view whose value is unhashable (`unhashable type: 'list'`).
    pub(crate) fn coerce_set_operand(
        &mut self,
        v: &Value,
    ) -> Option<Result<(PySet, bool)>> {
        if let Some(items) = set_items_from_value(v) {
            return Some(Ok(items));
        }
        match pyrust_builtins::dict_views::view_kind(v) {
            // dict_keys: keys are already `PyKey`s in the backing IndexMap.
            Some(0) => {
                let rc = pyrust_builtins::dict_views::as_dict_rc(v)?;
                let keys: PySet = rc.borrow().keys().cloned().collect();
                Some(Ok((keys, false)))
            }
            // dict_items: each pair becomes a `(key, value)` tuple `PyKey`; the
            // value must be hashable (matches CPython, which builds a set).
            Some(2) => {
                let rc = pyrust_builtins::dict_views::as_dict_rc(v)?;
                let pairs: Vec<(PyKey, Value)> = rc
                    .borrow()
                    .iter()
                    .map(|(k, val)| (k.clone(), val.clone()))
                    .collect();
                let mut out: PySet = PySet::default();
                for (k, val) in pairs {
                    let val_key = match self.value_to_pykey(&val) {
                        Ok(vk) => vk,
                        Err(e) => return Some(Err(e)),
                    };
                    out.insert(PyKey::Tuple(vec![k, val_key]));
                }
                Some(Ok((out, false)))
            }
            // dict_values (Some(1)) and non-views: not set-like.
            _ => None,
        }
    }

    /// Coerce an operand of a dict-view set operator (`&`/`|`/`-`/`^`) to its
    /// `(PySet, frozen)` items (issue #1891).
    ///
    /// Unlike [`Self::coerce_set_operand`], when `allow_iterable` is set this
    /// accepts *any* iterable — list, tuple, str, generator, dict, … — exactly
    /// as CPython's `dictviews_and`/`_or`/`_sub`/`_xor` do (they build a set
    /// from the iterable).  Returns `None` only for non-iterable operands, so
    /// the caller falls through and the normal `__and__`/etc. path raises the
    /// `unsupported operand type(s)` TypeError.
    pub(crate) fn coerce_setop_operand(
        &mut self,
        v: &Value,
        allow_iterable: bool,
    ) -> Option<Result<(PySet, bool)>> {
        if let Some(res) = self.coerce_set_operand(v) {
            return Some(res);
        }
        if !allow_iterable {
            return None;
        }
        // Not set-like: treat as an arbitrary iterable (CPython builds a set
        // from it).  A non-iterable operand surfaces the iterator protocol's
        // own `'<type>' object is not iterable` TypeError — matching CPython,
        // whose `dictviews_and`/etc. iterate the operand directly.
        let items = match self.collect_iterable(v) {
            Ok(items) => items,
            Err(e) => return Some(Err(e)),
        };
        let mut out: PySet = PySet::default();
        for item in items {
            match self.value_to_pykey(&item) {
                Ok(k) => {
                    out.insert(k);
                }
                Err(e) => return Some(Err(e)),
            }
        }
        Some(Ok((out, false)))
    }
}

/// Append `data` to `out`, applying printf width padding with spaces.
///
/// `%b` / `%s` / `%a` / `%c` never zero-fill (CPython only zero-fills numeric
/// codes), so this only handles space padding plus left/right alignment.
fn bytes_printf_apply_width(
    out: &mut Vec<u8>,
    data: &[u8],
    width: Option<usize>,
    left_align: bool,
) {
    let w = match width {
        None | Some(0) => {
            out.extend_from_slice(data);
            return;
        }
        Some(w) => w,
    };
    if data.len() >= w {
        out.extend_from_slice(data);
        return;
    }
    let pad = w - data.len();
    if left_align {
        out.extend_from_slice(data);
        out.extend(std::iter::repeat_n(b' ', pad));
    } else {
        out.extend(std::iter::repeat_n(b' ', pad));
        out.extend_from_slice(data);
    }
}

/// Extract a single byte from a bytes-like for `%c`, or raise the CPython
/// `TypeError` for an empty or multi-byte argument.
fn single_byte_or_err(data: &[u8]) -> Result<u8> {
    if data.len() == 1 {
        Ok(data[0])
    } else {
        Err(pyrust_core::type_err!(
            "%c requires an integer in range(256) or a single byte"
        ))
    }
}

/// Take the next positional argument for printf-style formatting.
fn str_printf_take_positional(positional: &Option<&[Value]>, idx: &mut usize) -> Result<Value> {
    match positional {
        None => Err(pyrust_core::type_err!("not enough arguments for format string")),
        Some(items) => {
            if *idx >= items.len() {
                Err(pyrust_core::type_err!("not enough arguments for format string"))
            } else {
                let v = items[*idx].clone();
                *idx += 1;
                Ok(v)
            }
        }
    }
}

/// Result of coercing a printf argument to an integer value.
///
/// `Small` covers values that fit in `i64` (the common case: `int`, `bool`,
/// truncated `float`).  `Big` is used only for `BigInt` values that are
/// outside the `i64` range — the caller formats them with BigInt-native
/// methods (`to_str_radix`, etc.) instead of Rust integer formatting.
enum PrintfInt {
    Small(i64),
    Big(PyBigInt),
}

/// Convert a `Value` to a `PrintfInt` for integer printf format codes.
///
/// Unlike the old `i64`-returning version, the `BigInt` arm no longer raises
/// `OverflowError`; it returns `PrintfInt::Big` so that the caller can format
/// arbitrarily large integers using BigInt-native methods.
///
/// For `%d`/`%i`/`%u`, float arguments are truncated toward zero following
/// CPython's `int(float)` semantics: NaN raises `ValueError`, infinity raises
/// `OverflowError`, and finite floats larger than `i64::MAX` are promoted to
/// `PrintfInt::Big` rather than being silently clamped.
fn str_printf_to_int(v: &Value, conv: char, bytes_mode: bool) -> Result<PrintfInt> {
    // CPython's bytes %-formatter normalises the %i alias to %d in its error
    // messages (the str formatter keeps %i); mirror that so the wording is
    // byte-identical to CPython 3.12 for both receivers.
    let disp = if bytes_mode && conv == 'i' { 'd' } else { conv };
    match v.kind() {
        ValueKind::Int(n) => Ok(PrintfInt::Small(n)),
        ValueKind::Bool(b) => Ok(PrintfInt::Small(b as i64)),
        ValueKind::Float(_) if matches!(conv, 'o' | 'x' | 'X') => {
            // CPython 3.12: %o/%x/%X reject float with "an integer is required".
            // %d/%i/%u accept float (truncating toward zero) for historical reasons.
            Err(pyrust_core::type_err!("%{disp} format: an integer is required, not float"))
        }
        ValueKind::Float(f) => {
            // CPython converts via PyLong_FromDouble: NaN → ValueError,
            // infinity → OverflowError, finite → truncate toward zero.
            // Rust's `f as i64` silently saturates at i64::MAX/MIN for
            // out-of-range finite floats, losing significant digits.
            let int_val = float_to_bigint(f)?;
            match int_val.kind() {
                ValueKind::Int(n) => Ok(PrintfInt::Small(n)),
                ValueKind::BigInt(b) => Ok(PrintfInt::Big(b.clone())),
                _ => unreachable!("float_to_bigint returns Int or BigInt"),
            }
        }
        ValueKind::BigInt(b) => Ok(PrintfInt::Big(b.clone())),
        _ => {
            // CPython uses "a real number is required" for %d/%i/%u,
            // and "an integer is required" for %o/%x/%X.
            let msg = if matches!(conv, 'o' | 'x' | 'X') {
                format!(
                    "%{disp} format: an integer is required, not {}",
                    pyrust_core::builtin_type_name(v)
                )
            } else {
                format!(
                    "%{disp} format: a real number is required, not {}",
                    pyrust_core::builtin_type_name(v)
                )
            };
            Err(pyrust_core::type_err!(msg))
        }
    }
}

/// Format a `BigInt` value for `%o`/`%x`/`%X` printf codes.
///
/// `to_str_radix` produces sign-magnitude notation (e.g., `-ff` for `-255`),
/// which matches CPython's behaviour.  This helper inserts the optional base
/// prefix (`0o`/`0x`/`0X`) and sign prefix (`+` / ` `) in the positions that
/// `apply_printf_width` expects for correct zero-fill later.
fn format_printf_bigint_radix(
    b: &PyBigInt,
    radix: u32,
    base_prefix: &str,
    upper: bool,
    flag_hash: bool,
    flag_plus: bool,
    flag_space: bool,
) -> String {
    // num_bigint::BigInt::to_str_radix uses sign-magnitude: negative values
    // get a leading '-'; the remaining digits are the absolute magnitude.
    let raw = b.to_str_radix(radix);
    let is_neg = raw.starts_with('-');
    let digits: std::borrow::Cow<str> = if upper {
        let d = if is_neg { &raw[1..] } else { &raw[..] };
        std::borrow::Cow::Owned(d.to_uppercase())
    } else if is_neg {
        std::borrow::Cow::Borrowed(&raw[1..])
    } else {
        std::borrow::Cow::Borrowed(&raw[..])
    };
    if is_neg {
        if flag_hash {
            format!("-{}{}", base_prefix, digits)
        } else {
            format!("-{}", digits)
        }
    } else if flag_hash {
        if flag_plus {
            format!("+{}{}", base_prefix, digits)
        } else if flag_space {
            format!(" {}{}", base_prefix, digits)
        } else {
            format!("{}{}", base_prefix, digits)
        }
    } else if flag_plus {
        format!("+{}", digits)
    } else if flag_space {
        format!(" {}", digits)
    } else {
        digits.into_owned()
    }
}

/// Convert a `Value` to `f64` for float printf format codes.
fn str_printf_to_float(v: &Value, _conv: char, bytes_mode: bool) -> Result<f64> {
    match v.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(n) => Ok(n as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        ValueKind::BigInt(b) => bigint_to_float_or_overflow(b),
        // CPython's bytes %-formatter reports "float argument required, not X"
        // for the float codes (%e/%E/%f/%F/%g/%G), whereas the str formatter
        // reports "must be real number, not X".
        _ if bytes_mode => Err(pyrust_core::type_err!(
            "float argument required, not {}",
            pyrust_core::builtin_type_name(v)
        )),
        _ => Err(pyrust_core::type_err!(
            "must be real number, not {}",
            pyrust_core::builtin_type_name(v)
        )),
    }
}

/// Truncate a string to `precision` Unicode chars (for `%s` and `%r`).
fn apply_str_precision(s: String, precision: Option<usize>) -> String {
    match precision {
        None => s,
        Some(max_chars) => {
            if s.chars().count() <= max_chars {
                s
            } else {
                s.chars().take(max_chars).collect()
            }
        }
    }
}

/// Apply width padding to a formatted value string.
fn apply_printf_width(
    s: String,
    width: Option<usize>,
    left_align: bool,
    zero_fill: bool,
    conv: char,
) -> String {
    let w = match width {
        None | Some(0) => return s,
        Some(w) => w,
    };
    let char_len = s.chars().count();
    if char_len >= w {
        return s;
    }
    let pad = w - char_len;
    // Zero-fill only for numeric codes, not %s/%r/%c, and not with left-align.
    if zero_fill
        && !left_align
        && matches!(
            conv,
            'd' | 'i' | 'u' | 'o' | 'x' | 'X' | 'f' | 'e' | 'E' | 'g' | 'G'
        )
    {
        // Determine the non-digit prefix: optional sign (+/-/space), then
        // optional base prefix (0x, 0X, 0o).  Zeros are inserted after the
        // full prefix so that "%#010x" % 255 → "0x000000ff" not "0000000xff".
        let prefix_len = {
            let mut cs = s.chars();
            let mut n = 0usize;
            // sign
            if let Some('+' | '-' | ' ') = cs.next() {
                n += 1;
                // base prefix after sign: 0x, 0X, 0o
                let mut peek = s[n..].chars();
                if peek.next() == Some('0')
                    && matches!(peek.next(), Some('x' | 'X' | 'o')) {
                        n += 2;
                    }
            } else if s.starts_with("0x") || s.starts_with("0X") || s.starts_with("0o") {
                n = 2;
            }
            n
        };
        let mut out = String::with_capacity(w);
        out.push_str(&s[..prefix_len]);
        for _ in 0..pad {
            out.push('0');
        }
        out.push_str(&s[prefix_len..]);
        return out;
    }
    if left_align {
        let mut out = s;
        for _ in 0..pad {
            out.push(' ');
        }
        out
    } else {
        let mut out = String::with_capacity(w);
        for _ in 0..pad {
            out.push(' ');
        }
        out.push_str(&s);
        out
    }
}

/// Format a float in scientific notation matching CPython's `%e`/`%E`.
///
/// CPython always uses a sign and at least two exponent digits: e+03, e-03.
/// Rust's default format may omit the sign for positive exponents; this
/// function normalises the output to match CPython.
fn format_scientific(f: f64, prec: usize, upper: bool) -> String {
    if f.is_nan() {
        return if upper { "NAN".to_string() } else { "nan".to_string() };
    }
    if f.is_infinite() {
        return if f > 0.0 {
            if upper { "INF".to_string() } else { "inf".to_string() }
        } else if upper {
            "-INF".to_string()
        } else {
            "-inf".to_string()
        };
    }
    let raw = format!("{:.prec$e}", f, prec = prec);
    let e_char = if upper { 'E' } else { 'e' };
    if let Some(pos) = raw.find('e') {
        let mantissa = &raw[..pos];
        let exp_str = &raw[pos + 1..];
        let digits = if exp_str.starts_with(['-', '+']) {
            &exp_str[1..]
        } else {
            exp_str
        };
        let sign: i32 = if exp_str.starts_with('-') { -1 } else { 1 };
        let exp_n: i32 = digits.parse::<i32>().unwrap_or(0) * sign;
        // {:+03} produces "+03", "-03" — sign always included, at least 2 digits.
        format!("{}{}{:+03}", mantissa, e_char, exp_n)
    } else {
        raw
    }
}

/// `%g` / `%G` printf conversion. Delegates the digit/exponent logic to the
/// shared `format_g` so the `%`-printf path and `str.format` path agree
/// on rounding-then-exponent (#2000) and `#`-alternate trailing zeros (#1950).
/// `alt` is the printf `#` flag.
fn format_general_float(f: f64, prec: usize, upper: bool, alt: bool) -> String {
    if f.is_nan() {
        return if upper { "NAN".to_string() } else { "nan".to_string() };
    }
    if f.is_infinite() {
        return if f > 0.0 {
            if upper { "INF".to_string() } else { "inf".to_string() }
        } else if upper {
            "-INF".to_string()
        } else {
            "-inf".to_string()
        };
    }
    let body = format_g(f.abs(), prec, upper, alt);
    if f.is_sign_negative() {
        format!("-{body}")
    } else {
        body
    }
}

fn is_not_implemented(v: &Value) -> bool {
    matches!(v.kind(), ValueKind::NotImplemented)
}

/// Does a class-attribute value look like a callable method?  Accepts
/// both pure-Python user functions and the `BuiltinFunction` entries
/// that `pyrust_module!`'s `class { … }` block produces — anything
/// else (descriptor, raw int set via `Foo.x = 1`, …) should fall
/// through dunder dispatch without being invoked.  Issue #331 added
/// `BuiltinFunction` to the accepted set so Counter's `__add__`
/// participates in the binary-op path.
fn is_callable_method(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
    )
}

/// Whether a resolved class-attribute slot value is callable, in the same
/// sense as the `callable()` builtin.  A plain function / builtin function is
/// callable (the common case); issue #2054 additionally accepts a callable
/// *instance* (an object whose class defines `__call__`), a bound method, or a
/// class object as a dunder slot, matching CPython's "invoke whatever the slot
/// resolves to" behaviour.  `invoke_class_method` knows how to dispatch each of
/// these.  A slot that is *not* callable (`__add__ = 5`) is rejected by the
/// callers with `TypeError: 'int' object is not callable` (issue #2055).
pub(crate) fn slot_is_callable(v: &Value) -> bool {
    match v.kind() {
        ValueKind::UserFunction(_)
        | ValueKind::BuiltinFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::PyClass(_) => true,
        ValueKind::BuiltinObject { .. } => {
            pyrust_builtins::bound_method::is_bound_method(v)
                || pyrust_builtins::super_bound_builtin::as_super_bound_builtin(v).is_some()
        }
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            lookup_class_attr(&class, "__call__").is_some()
        }
        _ => false,
    }
}

/// Container-only backing extraction for the `==` path (`values_user_eq`):
/// returns the backing list/tuple/dict/set/frozenset of a container subclass
/// instance with no user `__eq__` override, or `None` otherwise.  Scalar
/// backings are deliberately excluded — those are handled by the
/// `coerce_numeric` step further down in `values_user_eq`.
fn coerce_container_backing_for_eq(v: &Value) -> Option<Value> {
    let backing = coerce_subclass_backing(v, &["__eq__"])?;
    let is_container = matches!(
        backing.kind(),
        ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
            | ValueKind::Bytes(_)
    ) || pyrust_builtins::frozenset::as_items(&backing).is_some()
        || pyrust_builtins::bytearray::as_bytearray_snapshot(&backing).is_some();
    is_container.then_some(backing)
}

pub(crate) fn coerce_numeric(v: &Value) -> Value {
    // Extract via kind() in a scope so the borrow is dropped before we
    // clone `v` in the fallthrough — #450 made `kind()`'s borrow
    // explicit, so we can't hold a borrow while returning an owned Value.
    if let ValueKind::Bool(b) = v.kind() {
        return Value::int(b as i64);
    }
    // Issue #1204: PyInstance subclasses of int/float/str/bytes carry their
    // underlying primitive value as `__builtin_data__`.  Extract it here so
    // that arithmetic and concatenation operations on bare subclass instances
    // (e.g. `MyInt(42) + 1`) fall through to the primitive fast paths below.
    // This mirrors CPython's slot delegation for `tp_as_number` / `tp_as_sequence`.
    if let Some(backing) = builtin_data_backing(v) {
        let is_scalar = matches!(
            backing.kind(),
            ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _)
                | ValueKind::Str(_)
                | ValueKind::Bytes(_)
        );
        if is_scalar {
            return backing;
        }
    }
    v.clone()
}

/// Like [`coerce_numeric`] but also unwraps *container* subclass backings
/// (list/tuple/dict/set/frozenset).  Used by the `+`/`*`/`<` operator paths
/// where a user dunder override was already dispatched upstream (so no
/// override check is needed here) and the result type should follow the base
/// type (`L([1]) + [2]` → plain `list`).
///
/// Hot-path: a single `as_py_instance_rc()` tag check.  Concrete operands
/// (the common `[1,2] + [3,4]` / `5 + 6` case) take the `Bool`-then-clone
/// fall-through with no extra instance probe — identical cost to the bare
/// `coerce_numeric` it replaced.
#[inline]
pub(crate) fn coerce_operand_backing(v: &Value) -> Value {
    if let ValueKind::Bool(b) = v.kind() {
        return Value::int(b as i64);
    }
    if let Some(backing) = builtin_data_backing(v) {
        let is_primitive = matches!(
            backing.kind(),
            ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Float(_)
                | ValueKind::Str(_)
                | ValueKind::Bytes(_)
                | ValueKind::List(_)
                | ValueKind::Tuple(_)
                | ValueKind::Dict(_)
                | ValueKind::Set(_)
        ) || pyrust_builtins::frozenset::as_items(&backing).is_some()
            || pyrust_builtins::bytearray::as_bytearray_snapshot(&backing).is_some();
        if is_primitive {
            return backing;
        }
    }
    v.clone()
}

/// Extract the primitive backing of a builtin-subclass `PyInstance` —
/// scalars (int/float/str/bytes) AND containers (list/tuple/dict/set/
/// frozenset) — but only when the subclass does NOT override the relevant
/// dunder(s) with a user method.  Returns `Some(backing)` when the value is
/// such a subclass using inherited builtin behaviour; `None` otherwise.
///
/// This is the container-aware analogue of [`coerce_numeric`] used by the
/// `+`/`*`/`==`/ordering/hashing paths so that, e.g., `L([1]) + [2]` (where
/// `L` subclasses `list`) operates on the backing list and yields a plain
/// `list`, matching CPython's inherited-slot semantics (#1929/#1934/#1936/
/// #1939).
///
/// `override_dunders` lists the user-method names that, if present on the
/// subclass MRO, mean the subclass customises this operation and the backing
/// must NOT be used (the override wins).  `lookup_class_attr` only finds
/// *user*-defined dunders (builtin dunders aren't exposed as class attrs —
/// #1909), so a `Some` result there reliably indicates an override.
///
/// Hot-path note: the only work for a non-`PyInstance` operand is the
/// `as_py_instance_rc()` tag check, which returns `None` immediately — the
/// dunder lookups run only for actual subclass instances.
pub(crate) fn coerce_subclass_backing(v: &Value, override_dunders: &[&str]) -> Option<Value> {
    // Thin alias for the unified representation-substitutability boundary
    // (issue #2386).  `effective_builtin_receiver` covers every builtin backing
    // — scalars, containers, AND `BuiltinObject`-backed `bytearray`/`frozenset`
    // — with the same inherited-vs-overridden dunder gate this op-path needs.
    effective_builtin_receiver(v, override_dunders)
}

pub(crate) fn iter_values(value: &Value) -> Result<Vec<Value>> {
    // list/dict/set subclass: delegate to the backing primitive value.
    // Keep the `inst_rc` binding (not just `builtin_data_backing`) so the
    // not-iterable error below can name the actual subclass, not the base.
    if let Some(inst_rc) = value.as_py_instance_rc()
        && let Some(backing) = instance_builtin_data(inst_rc) {
            // A subclass of a *non-iterable* builtin (e.g. `class C(int): pass`)
            // is itself not iterable.  CPython reports the actual subclass name
            // ("'C' object is not iterable"), not the backing base's name, so
            // re-label the not-iterable error with the carrier's class name
            // rather than letting the int/float/… backing surface "'int' …".
            return iter_values(&backing).map_err(|e| {
                if e.class_name_is("TypeError") {
                    pyrust_core::type_err!(
                        "'{}' object is not iterable",
                        inst_rc.borrow().class.borrow().name
                    )
                } else {
                    e
                }
            });
        }
    match value.kind() {
        ValueKind::List(items) => Ok(items.to_vec()),
        ValueKind::Tuple(items) => Ok(items.to_vec()),
        ValueKind::Set(items) => Ok(items.iter().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::BuiltinObject { .. } => {
            // Frozensets materialise through their inner key set; dict views
            // materialise through their backing IndexMap; everything else
            // iterates via `iter_next`.
            // Bytearray: materialise as integers (same shape as bytes iteration).
            if let Some(elems) = pyrust_builtins::bytearray::iter_elements(value) {
                return Ok(elems);
            }
            if let Some(rc) = pyrust_builtins::frozenset::as_items(value) {
                return Ok(rc.iter().map(|k| key_to_value(k.clone())).collect());
            }
            if let Some(kind) = pyrust_builtins::dict_views::view_kind(value) {
                // `view_kind` and `as_dict_rc` both check the same ops, so
                // they should agree — but use a structured error rather than
                // unwrap to avoid panicking if a future BuiltinObject impl
                // shares the dict-view type name without the matching state.
                // Surface as TypeError so Python-level `except` blocks can
                // catch it (the only way to reach this is a misregistered
                // ops table, which is a type-mismatch error).
                let rc = pyrust_builtins::dict_views::as_dict_rc(value).ok_or_else(|| {
                    pyrust_core::type_err!("dict-view state type mismatch")
                })?;
                let map = rc.borrow();
                return Ok(match kind {
                    0 => map.keys().map(|k| key_to_value(k.clone())).collect(),
                    1 => map.values().cloned().collect(),
                    _ => map
                        .iter()
                        .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                        .collect(),
                });
            }
            if let Some(class_rc) = pyrust_builtins::mapping_proxy::as_class_rc(value) {
                let class = class_rc.borrow();
                return Ok(class
                    .attrs
                    .keys()
                    .map(|k| Value::string(k.clone()))
                    .collect());
            }
            // Dict-backed `mappingproxy` (`d.keys().mapping`, issue #2679):
            // iterating yields the parent dict's keys, like iterating a dict.
            if let Some(rc) = pyrust_builtins::mapping_proxy::as_dict_rc(value) {
                return Ok(rc.borrow().keys().map(|k| key_to_value(k.clone())).collect());
            }
            let mut out = Vec::new();
            let ValueKind::BuiltinObject { ops, state } = value.kind() else {
                unreachable!();
            };
            if !ops.is_iterable() {
                return Err(pyrust_core::type_err!("'{}' object is not iterable", ops.type_name()));
            }
            while let Some(v) = ops.iter_next(state)? {
                out.push(v);
            }
            Ok(out)
        }
        ValueKind::Bytes(rc) => Ok(rc.iter().map(|b| Value::int(*b as i64)).collect()),
        ValueKind::Str(text) => Ok(pyrust_core::cesu8_codepoints(text)
            .map(|cp| Value::string(pyrust_core::cesu8_encode_codepoint(cp)))
            .collect()),
        ValueKind::Dict(items) => Ok(items.keys().map(|k| key_to_value(k.clone())).collect()),
        ValueKind::Range { start, stop, step } => {
            let mut out = Vec::new();
            if step > 0 {
                let mut cur = start;
                while cur < stop {
                    out.push(Value::int(cur));
                    cur += step;
                }
            } else {
                let mut cur = start;
                while cur > stop {
                    out.push(Value::int(cur));
                    cur += step;
                }
            }
            Ok(out)
        }
        ValueKind::BigRange { start, stop, step } => {
            // Materialize an arbitrary-precision range (#2118).  Only reached for
            // out-of-i64 bounds; the element *count* still fits in memory (a range
            // whose length itself overflows would OOM here, exactly as CPython's
            // `list(range(...))` does).
            let mut out = Vec::new();
            let mut cur = start.clone();
            if step.sign() == pyrust_core::PyBigIntSign::Plus {
                while cur < *stop {
                    out.push(value_from_bigint(cur.clone()));
                    cur += step;
                }
            } else {
                while cur > *stop {
                    out.push(value_from_bigint(cur.clone()));
                    cur += step;
                }
            }
            Ok(out)
        }
        ValueKind::Generator(state_rc) => {
            // Drain a NativeIterFrame (created by iter() on builtins) into a Vec.
            let mut borrow = state_rc.borrow_mut();
            if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                let remaining = native.items[native.pos..].to_vec();
                native.pos = native.items.len();
                // Bulk-drain reaches end-of-iteration in one shot; latch the
                // exhausted flag so a later size mutation + `next()` returns
                // StopIteration (not RuntimeError), matching `advance()`'s
                // clean-exhaustion path and CPython (#2448).
                native.exhausted = true;
                Ok(remaining)
            } else {
                Err(pyrust_core::type_err!("object is not iterable"))
            }
        }
        _ => Err(pyrust_core::type_err!("'{}' object is not iterable", value_type_name_str(value))),
    }
}

/// Resolve a built-in name to its `Value` for use as a `LoadGlobal` fallback.
///
/// The 11 primitive type names (`int`, `str`, `list`, …) resolve to the
/// per-thread `PyClass` singletons from `primitive_class_by_name` — see
/// issue #462.  `bool` resolves to a class whose `base` chains to `int`,
/// so `bool.__bases__ == (int,)` matches CPython.  These cannot go through
/// the generic registry path because `isinstance`/`issubclass` require a
/// `ValueKind::PyClass`, not a `ValueKind::BuiltinFunction`.
///
/// `NotImplemented` is a singleton constant, not a callable; it is
/// returned directly without a registry lookup.
///
/// All other names are resolved through `builtin_registry::lookup_name`,
/// which returns the interned `&'static str` stored in the registry entry
/// so `Value::builtin_function` never needs to heap-allocate a new name.
/// Adding a `fn foo(…)` to a `pyrust_module!` body automatically makes
/// `foo()` reachable via bare-name `LoadGlobal` with no edits here.
pub(crate) fn resolve_builtin(name: &str) -> Option<Value> {
    // Primitive types: must remain `Value::py_class` so that
    // `isinstance(x, int)` and `type(x) is int` work correctly (#462).
    if matches!(
        name,
        "bool" | "bytearray" | "bytes" | "complex" | "dict" | "float" | "frozenset"
            | "int"
            | "list"
            | "range"
            | "set"
            | "str"
            | "tuple"
    ) {
        return primitive_class_by_name(name).map(Value::py_class);
    }
    if name == "object" {
        return Some(Value::py_class(object_class_singleton()));
    }
    // `type` is the metaclass — must resolve to a PyClass singleton so that
    // `type is type`, `builtins.type is type`, and `repr(type)` all behave
    // as CPython 3.12 (issue #1312).
    if name == "type" {
        return Some(Value::py_class(type_class_singleton()));
    }
    // Singleton constants that are not callable.
    if name == "NotImplemented" {
        return Some(Value::not_implemented());
    }
    if name == "Ellipsis" {
        return Some(Value::ellipsis());
    }
    // Built-in exception classes — resolved lazily via `EXC_CLASS_CACHE`
    // (built once per thread on first access).  Exception classes are no
    // longer pre-inserted into the module env at startup; scripts that
    // never reference an exception class name pay zero class-build cost.
    if matches!(
        name,
        "ArithmeticError"
            | "AssertionError"
            | "AttributeError"
            | "BaseException"
            | "BaseExceptionGroup"
            | "BlockingIOError"
            | "BrokenPipeError"
            | "BufferError"
            | "BytesWarning"
            | "ChildProcessError"
            | "ConnectionAbortedError"
            | "ConnectionError"
            | "ConnectionRefusedError"
            | "ConnectionResetError"
            | "DeprecationWarning"
            | "EOFError"
            | "EncodingWarning"
            | "Exception"
            | "ExceptionGroup"
            | "FileExistsError"
            | "FileNotFoundError"
            | "FloatingPointError"
            | "FutureWarning"
            | "GeneratorExit"
            | "ImportError"
            | "ImportWarning"
            | "IndentationError"
            | "IndexError"
            | "InterruptedError"
            | "IsADirectoryError"
            | "KeyError"
            | "KeyboardInterrupt"
            | "LookupError"
            | "MemoryError"
            | "ModuleNotFoundError"
            | "NameError"
            | "NotADirectoryError"
            | "NotImplementedError"
            | "EnvironmentError"
            | "IOError"
            | "OSError"
            | "OverflowError"
            | "PendingDeprecationWarning"
            | "PermissionError"
            | "ProcessLookupError"
            | "RecursionError"
            | "ReferenceError"
            | "ResourceWarning"
            | "RuntimeError"
            | "RuntimeWarning"
            | "StopAsyncIteration"
            | "StopIteration"
            | "SyntaxError"
            | "SyntaxWarning"
            | "SystemError"
            | "SystemExit"
            | "TabError"
            | "TimeoutError"
            | "TypeError"
            | "UnboundLocalError"
            | "UnicodeDecodeError"
            | "UnicodeEncodeError"
            | "UnicodeError"
            | "UnicodeTranslateError"
            | "UnicodeWarning"
            | "UserWarning"
            | "ValueError"
            | "Warning"
            | "ZeroDivisionError"
    ) {
        return lookup_exc_class(name)
            .map(pyrust_core::Value::py_class);
    }
    // All registered flat-namespace builtins (`print`, `len`, `abs`, …).
    // lookup_name returns the interned &'static str already stored in the
    // registry entry, so Value::builtin_function needs no extra allocation.
    crate::builtin_registry::lookup_name(name).map(Value::builtin_function)
}

/// Operation tag for set/frozenset binary operators.
#[derive(Clone, Copy)]
enum SetOp {
    Or,  // union
    And, // intersection
    Sub, // difference
    Xor, // symmetric difference
}

/// Extract key-value pairs from a plain `dict` or a `PyInstance` dict
/// subclass backed by a dict.  Returns `None` for any other type.
/// Used by the PEP 584 `dict | dict` merge path in `eval_binary`.
/// CPython's `_PyObject_FunctionStr` for the duplicate-keyword-splat error:
/// `<module>.<qualname>` for a user function (the module prefix is dropped when
/// it is missing or `"builtins"`), else `None` for callees whose qualified name
/// pyrust can't recover here (the error then omits the leading name).
fn callee_function_str(callee: &Value) -> Option<String> {
    let f = match callee.kind() {
        ValueKind::UserFunction(f) => f.clone(),
        ValueKind::BoundMethod { function, .. } | ValueKind::ClassBoundMethod { function, .. } => {
            function.clone()
        }
        // Constructor call `C(**a, **b)`: CPython's `_PyObject_FunctionStr`
        // names the type by `<module>.<qualname>` (the `builtins.` prefix is
        // dropped, e.g. `dict()`).
        ValueKind::PyClass(class) => {
            let c = class.borrow();
            let qual = c.qualname.clone();
            let module = c
                .attrs
                .get("__module__")
                .and_then(|v| v.as_str().map(|s| s.to_string()));
            return match module.as_deref() {
                Some(m) if !m.is_empty() && m != "builtins" => Some(format!("{m}.{qual}")),
                _ => Some(qual),
            };
        }
        // Builtin function/method (`print`, `sorted`, `dict.fromkeys`, …): the
        // stored name is already the (module-less) qualname CPython reports.
        ValueKind::BuiltinFunction(name) => return Some(name.to_string()),
        _ => return None,
    };
    let qual = f.effective_qualname();
    let module = f.module_value();
    match module.as_str() {
        Some(m) if !m.is_empty() && m != "builtins" => Some(format!("{m}.{qual}")),
        _ => Some(qual),
    }
}

/// Render a kwargs key as the keyword name for the duplicate-key error.  Call
/// keyword keys are always `str`; the fallback is defensive.
fn kwkey_name(key: &PyKey) -> String {
    match key {
        PyKey::Str(s) => s.as_str().unwrap_or_default().to_owned(),
        other => format!("{other:?}"),
    }
}

/// Build `TypeError: <name>() got multiple values for keyword argument '<kw>'`,
/// matching CPython's `DICT_MERGE` wording.  When `func_name` is `None` (no
/// recoverable callee name) the function-name prefix is omitted.
fn multiple_values_kw_error(func_name: Option<&str>, kw: &str) -> PyError {
    match func_name {
        Some(name) => pyrust_core::type_err!(
            "{}() got multiple values for keyword argument '{}'",
            name,
            kw
        ),
        None => pyrust_core::type_err!("got multiple values for keyword argument '{}'", kw),
    }
}

/// Operand type name for `|` TypeError messages.  A `mappingproxy` reports as
/// `dict` because its `__or__` / `__ror__` slots are `dict.__or__` /
/// `dict.__ror__` in CPython 3.12, so a failed merge names the operand `dict`.
fn bitor_operand_type_name(v: &Value) -> std::borrow::Cow<'static, str> {
    if is_mapping_proxy(v) {
        std::borrow::Cow::Borrowed("dict")
    } else {
        value_type_name_str(v)
    }
}

/// True if `v` is a `mappingproxy` (either class- or dict-backed).
fn is_mapping_proxy(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::BuiltinObject { ops, .. }
            if ops.type_name() == pyrust_builtins::mapping_proxy::TYPE_NAME
    )
}

fn dict_entries_from_value(v: &Value) -> Option<Vec<(PyKey, Value)>> {
    if let Some(entries) = v.dict_with(|d| {
        d.iter().map(|(k, val)| (k.clone(), val.clone())).collect::<Vec<_>>()
    }) {
        return Some(entries);
    }
    // `mappingproxy` participates in PEP 584 `|` like a dict (CPython 3.12).
    // Extract entries from the underlying source (class attrs or live dict).
    if let Some(cls_rc) = pyrust_builtins::mapping_proxy::as_class_rc(v) {
        return Some(
            cls_rc
                .borrow()
                .attrs
                .iter()
                .map(|(k, val)| (PyKey::str_from(k), val.clone()))
                .collect(),
        );
    }
    if let Some(dict_rc) = pyrust_builtins::mapping_proxy::as_dict_rc(v) {
        return Some(dict_rc.borrow().clone().into_iter().collect());
    }
    if let Some(backing) = builtin_data_backing(v) {
        return dict_entries_from_value(&backing);
    }
    None
}


/// True if `v` is a set-like dict view: `dict_keys` or `dict_items`
/// (issue #1891).  `dict_values` is deliberately *not* set-like.
fn is_setlike_view(v: &Value) -> bool {
    matches!(pyrust_builtins::dict_views::view_kind(v), Some(0) | Some(2))
}

/// Extract a set's items and frozen flag from a value that is a `set`,
/// `frozenset`, or a `PyInstance` subclass backed by either.  Returns
/// `None` when the value is none of those.
fn set_items_from_value(v: &Value) -> Option<(PySet, bool)> {
    if let ValueKind::Set(s) = v.kind() {
        return Some((s.clone(), false));
    }
    if let Some(rc) = pyrust_builtins::frozenset::as_items(v) {
        return Some(((*rc).clone(), true));
    }
    if let Some(backing) = builtin_data_backing(v) {
        return set_items_from_value(&backing);
    }
    None
}

/// Compute a binary set operation when both operands are set/frozenset (or
/// `PyInstance` subclasses thereof).  Returns `Set` if both backing stores are
/// mutable sets, otherwise `FrozenSet` (any frozenset operand promotes the
/// result, matching CPython).
///
/// Returns `None` when the left operand is not a set/frozenset (caller should
/// fall through to the next handler).  Returns `Some(Err(...))` when the left
/// operand is a set/frozenset but the right operand is not — CPython raises
/// `TypeError: unsupported operand type(s) for OP: 'X' and 'Y'` in that case.
/// True if the set holds any `PyKey::Object` element, i.e. a user instance
/// whose membership/equality requires `__hash__`/`__eq__` dispatch rather than
/// raw `IndexSet` identity comparison (issue #1907).  All-primitive sets take
/// the fast raw path.
fn set_has_object_key(s: &PySet) -> bool {
    // Recurse into Tuple/FrozenSet element keys so a user object nested inside
    // a tuple key also forces the eq-aware path (issue #2059); a set of
    // primitive (or primitive-tuple) keys stays on the raw fast path.
    s.iter().any(key_contains_object)
}

/// Cheap, borrow-only check for whether an iterable operand to a set-algebra
/// method form *may* contain a user instance (and thus require `__eq__`
/// dispatch).  Used to keep all-primitive operands on the fast path without
/// materialising `PyKey`s (issue #1907).  Conservatively returns `true` for any
/// element / set key that is a `PyInstance` or already an `Object` key, and for
/// iterables whose contents cannot be cheaply inspected (Generators, custom
/// `BuiltinObject`s) so correctness is never sacrificed for speed.
fn value_iterable_has_object(v: &Value) -> bool {
    match v.kind() {
        ValueKind::Set(s) => set_has_object_key(&s),
        ValueKind::List(items) => {
            items.iter().any(|x| matches!(x.kind(), ValueKind::PyInstance(_)))
        }
        ValueKind::Tuple(items) => {
            items.iter().any(|x| matches!(x.kind(), ValueKind::PyInstance(_)))
        }
        ValueKind::Dict(d) => d.keys().any(key_contains_object),
        // Primitive flat iterables can never hold user instances.
        ValueKind::Str(_) | ValueKind::Bytes(_) | ValueKind::Range { .. } => false,
        _ => {
            if let Some(rc) = pyrust_builtins::frozenset::as_items(v) {
                set_has_object_key(&rc)
            } else {
                // Unknown / opaque iterable: be conservative and dispatch
                // `__eq__` (still correct for primitive elements — just slower).
                true
            }
        }
    }
}

fn set_binary_op(
    interp: &mut Interpreter,
    left: &Value,
    right: &Value,
    op: SetOp,
    op_sym: &str,
) -> Option<Result<Value>> {
    // CPython's dict-view set operators (`&`/`|`/`-`/`^`) accept *any* iterable
    // on the other side, not just a set — `d.keys() & ['a']`, `['a'] & d.keys()`,
    // `d.keys() | 'ab'`, `d.keys() - (g for g in …)` all work and return a plain
    // `set` (issue #1891).  Real `set`/`frozenset` operators keep the strict
    // "set operand required" rule, so only relax it when a view is involved.
    let view_involved = is_setlike_view(left) || is_setlike_view(right);
    if view_involved {
        let lhs_items = match interp.coerce_setop_operand(left, true) {
            Some(Ok(items)) => items,
            Some(Err(e)) => return Some(Err(e)),
            None => return None,
        };
        let rhs_items = match interp.coerce_setop_operand(right, true) {
            Some(Ok(items)) => items,
            Some(Err(e)) => return Some(Err(e)),
            None => return None,
        };
        return set_binary_op_from_items(interp, lhs_items, rhs_items, op, true);
    }
    // Fast path: both operands are plain `set` / `frozenset` (or `PyInstance`
    // subclasses backed by either) whose elements are all primitive (no
    // `PyKey::Object` user instances).  Borrow the backing `IndexSet`s in place
    // and clone only the elements that land in the result, instead of cloning
    // both whole operands up front (issue #1978).
    //
    // Sets with object keys take the eq-aware path below: there, user
    // `__hash__`/`__eq__` runs during the algebra and could re-enter and mutate
    // an operand, so we must not hold a live borrow of the backing `RefCell`
    // across it — the existing clone-then-compute path stays correct there.
    // Dict views and other set-like shapes also fall through.
    if let (Some((lhs_val, l_frozen)), Some((rhs_val, _r_frozen))) =
        (set_direct_value(left), set_direct_value(right))
    {
        let primitive = !with_set_items(&lhs_val, set_has_object_key)
            && !with_set_items(&rhs_val, set_has_object_key);
        if primitive {
            let out = with_set_items(&lhs_val, |a| {
                with_set_items(&rhs_val, |b| set_algebra_fast(a, b, op))
            });
            // Result type follows the LEFT operand (CPython 3.12): `set &
            // frozenset` → `set`, `frozenset & set` → `frozenset` (issue #2042).
            return Some(Ok(if l_frozen {
                pyrust_builtins::frozenset::frozenset(out)
            } else {
                Value::set(out)
            }));
        }
    }
    // LHS must be set-like (set/frozenset/subclass or a set-like dict view,
    // issue #1891); otherwise this isn't a set op and the caller falls through.
    let lhs_items = match interp.coerce_set_operand(left)? {
        Ok(items) => items,
        Err(e) => return Some(Err(e)),
    };
    // LHS is set-like; if RHS is not, emit the CPython-format TypeError.
    let rhs_items = match interp.coerce_set_operand(right) {
        Some(Ok(items)) => items,
        Some(Err(e)) => return Some(Err(e)),
        None => {
            // Only `|` delegates to dict's PEP 584 slots, so a mappingproxy
            // operand reports as `dict` for `|` but keeps its own name for the
            // set-only operators `&` / `-` / `^` (CPython 3.12).
            let (lt, rt) = if op_sym == "|" {
                (bitor_operand_type_name(left), bitor_operand_type_name(right))
            } else {
                (value_type_name_str(left), value_type_name_str(right))
            };
            return Some(Err(pyrust_core::type_err!("unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'")));
        }
    };
    set_binary_op_from_items(interp, lhs_items, rhs_items, op, false)
}

// The set-op no-clone fast-path helpers — `set_direct_value`, `with_set_items`,
// and `set_algebra_fast` — were moved to `runtime/fast_path.rs` (issue #1978).
// `set_binary_op` above still calls them; they live in the same `include!` scope.

/// Shared set-algebra core for [`set_binary_op`].  Computes `lhs OP rhs` over
/// already-coerced `(PySet, frozen)` operands and packages the result.
///
/// `force_set` forces a plain `set` result regardless of the operands' frozen
/// flags — used for dict-view operators, which always return `set` (issue
/// #1891); otherwise the result type follows the LEFT operand (issue #2042).
fn set_binary_op_from_items(
    interp: &mut Interpreter,
    lhs_items: (PySet, bool),
    rhs_items: (PySet, bool),
    op: SetOp,
    force_set: bool,
) -> Option<Result<Value>> {
    let (a, l_frozen) = lhs_items;
    // RHS frozen-ness is irrelevant: the result type follows the LEFT operand
    // (issue #2042).
    let (b, _r_frozen) = rhs_items;
    // Fast path: neither operand contains user-instance keys, so raw
    // `IndexSet` identity comparison is exact (issue #1907).  Most sets are
    // primitive, so keep this allocation-cheap path with no `__eq__` dispatch.
    let needs_eq = set_has_object_key(&a) || set_has_object_key(&b);
    let result: Result<PySet> = if !needs_eq {
        let mut out: PySet = PySet::default();
        match op {
            SetOp::Or => {
                for k in a.iter().chain(b.iter()) {
                    out.insert(k.clone());
                }
            }
            SetOp::And => {
                for k in a.iter() {
                    if b.contains(k) {
                        out.insert(k.clone());
                    }
                }
            }
            SetOp::Sub => {
                for k in a.iter() {
                    if !b.contains(k) {
                        out.insert(k.clone());
                    }
                }
            }
            SetOp::Xor => {
                for k in a.iter() {
                    if !b.contains(k) {
                        out.insert(k.clone());
                    }
                }
                for k in b.iter() {
                    if !a.contains(k) {
                        out.insert(k.clone());
                    }
                }
            }
        }
        Ok(out)
    } else {
        // Slow path: at least one operand holds user instances.  Membership
        // (`contains`) and insertion (`insert`) go through `set_lookup_in` /
        // `set_insert`, which dispatch user `__hash__`-then-`__eq__`.
        (|| -> Result<PySet> {
            let mut out: PySet = PySet::default();
            match op {
                SetOp::Or => {
                    for k in a.iter().chain(b.iter()) {
                        interp.set_insert(&mut out, k.clone())?;
                    }
                }
                SetOp::And => {
                    for k in a.iter() {
                        if interp.set_lookup_in(&b, k)?.is_some() {
                            interp.set_insert(&mut out, k.clone())?;
                        }
                    }
                }
                SetOp::Sub => {
                    for k in a.iter() {
                        if interp.set_lookup_in(&b, k)?.is_none() {
                            interp.set_insert(&mut out, k.clone())?;
                        }
                    }
                }
                SetOp::Xor => {
                    for k in a.iter() {
                        if interp.set_lookup_in(&b, k)?.is_none() {
                            interp.set_insert(&mut out, k.clone())?;
                        }
                    }
                    for k in b.iter() {
                        if interp.set_lookup_in(&a, k)?.is_none() {
                            interp.set_insert(&mut out, k.clone())?;
                        }
                    }
                }
            }
            Ok(out)
        })()
    };
    let out = match result {
        Ok(out) => out,
        Err(e) => return Some(Err(e)),
    };
    // Result type follows the LEFT operand (CPython 3.12, issue #2042): `set &
    // frozenset` → `set`, `frozenset & set` → `frozenset`. The RHS frozen-ness
    // never affects the result type. Dict-view operators always yield `set`
    // (`force_set`).
    Some(Ok(if l_frozen && !force_set {
        pyrust_builtins::frozenset::frozenset(out)
    } else {
        Value::set(out)
    }))
}

/// Set/frozenset subset-relation comparison.
///
/// Returns `Some(Ok(bool))` when both `left` and `right` are set/frozenset
/// (or subclasses thereof), `None` otherwise (caller should fall through to
/// a TypeError).
///
/// Semantics match CPython 3.12:
/// - `a < b`  — proper subset: every element of `a` is in `b` and `a != b`
/// - `a <= b` — subset: every element of `a` is in `b`
/// - `a > b`  — proper superset: every element of `b` is in `a` and `a != b`
/// - `a >= b` — superset: every element of `b` is in `a`
///
/// Mixed `set`/`frozenset` comparisons are supported, as in CPython.
fn set_subset_cmp(
    interp: &mut Interpreter,
    left: &Value,
    right: &Value,
    op: BinaryOp,
) -> Option<Result<Value>> {
    // Both operands must be set-like (set/frozenset/subclass or a set-like dict
    // view, issue #1891); otherwise fall through to the normal comparison path
    // so it raises the `'<=' not supported between …` TypeError.
    let (a, _) = match interp.coerce_set_operand(left)? {
        Ok(items) => items,
        Err(e) => return Some(Err(e)),
    };
    let (b, _) = match interp.coerce_set_operand(right)? {
        Ok(items) => items,
        Err(e) => return Some(Err(e)),
    };
    // Fast path: all-primitive operands — raw `contains` is exact (issue #1907).
    let needs_eq = set_has_object_key(&a) || set_has_object_key(&b);
    let (is_subset, is_superset) = if !needs_eq {
        (
            a.iter().all(|k| b.contains(k)),
            b.iter().all(|k| a.contains(k)),
        )
    } else {
        // Slow path: membership via `set_lookup_in` so user `__eq__` decides.
        let compute = (|| -> Result<(bool, bool)> {
            let mut subset = true;
            for k in a.iter() {
                if interp.set_lookup_in(&b, k)?.is_none() {
                    subset = false;
                    break;
                }
            }
            let mut superset = true;
            for k in b.iter() {
                if interp.set_lookup_in(&a, k)?.is_none() {
                    superset = false;
                    break;
                }
            }
            Ok((subset, superset))
        })();
        match compute {
            Ok(pair) => pair,
            Err(e) => return Some(Err(e)),
        }
    };
    let result = match op {
        BinaryOp::Lt => is_subset && !is_superset,
        BinaryOp::Le => is_subset,
        BinaryOp::Gt => is_superset && !is_subset,
        BinaryOp::Ge => is_superset,
        _ => unreachable!("set_subset_cmp called with non-comparison op"),
    };
    Some(Ok(Value::bool_(result)))
}

/// Coerce a numeric value to a `(real, imag)` pair if possible.
///
/// Returns `Ok(Some(...))` on success, `Ok(None)` when the value is not a
/// numeric type that participates in complex arithmetic, and `Err(...)` when
/// the value is a `BigInt` that is too large to convert to `f64` (matching
/// CPython 3.12's `OverflowError: int too large to convert to float`).
fn as_complex_pair(v: &Value) -> Result<Option<(f64, f64)>> {
    // Issue #2544: a `complex`/`int`/`float` subclass instance carries its
    // primitive in `__builtin_data__`; unwrap it so subclass operands take the
    // same numeric path as the base type (`C(1, 2) + 1` → `(2+2j)`).  A user
    // arithmetic dunder was already dispatched upstream in `eval_binary`, so no
    // override gate is needed here.
    let v = coerce_numeric(v);
    match v.kind() {
        ValueKind::Complex(re, im) => Ok(Some((re, im))),
        ValueKind::Int(n) => Ok(Some((n as f64, 0.0))),
        ValueKind::Float(f) => Ok(Some((f, 0.0))),
        ValueKind::Bool(b) => Ok(Some((if b { 1.0 } else { 0.0 }, 0.0))),
        ValueKind::BigInt(b) => Ok(Some((bigint_to_float_or_overflow(b)?, 0.0))),
        _ => Ok(None),
    }
}

/// True when `v` is a `complex` value or a `complex`-backed subclass instance
/// (issue #2544).  Used to decide whether an arithmetic operand pair should be
/// routed through the complex path; a bare `int`/`float` operand stays on its
/// dedicated numeric fast path.
fn is_complex_operand(v: &Value) -> bool {
    match v.kind() {
        ValueKind::Complex(_, _) => true,
        ValueKind::PyInstance(_) => builtin_data_backing(v)
            .is_some_and(|b| matches!(b.kind(), ValueKind::Complex(_, _))),
        _ => false,
    }
}

/// A pair of complex operands as `((re, im), (re, im))`.
type ComplexOperands = ((f64, f64), (f64, f64));

/// Returns the two operands as complex `(re, im)` pairs only when AT LEAST
/// one of them is already a complex number — that way pure int/float
/// arithmetic continues to use the dedicated fast paths.
///
/// Returns `Ok(None)` when neither operand is complex or when one operand is
/// not a numeric type.  Returns `Err(...)` when a `BigInt` operand overflows
/// `f64` (propagated as `OverflowError`).
fn both_as_complex(left: &Value, right: &Value) -> Result<Option<ComplexOperands>> {
    // Issue #2544: also treat a `complex`-backed subclass instance as complex so
    // its arithmetic flows through here rather than falling to the TypeError arm.
    let l_is_c = is_complex_operand(left);
    let r_is_c = is_complex_operand(right);
    if !l_is_c && !r_is_c {
        return Ok(None);
    }
    let Some(a) = as_complex_pair(left)? else {
        return Ok(None);
    };
    let Some(b) = as_complex_pair(right)? else {
        return Ok(None);
    };
    Ok(Some((a, b)))
}

/// Compute complex exponentiation `(zr + zi*j) ** (wr + wi*j)` with
/// CPython 3.12 parity.
///
/// Mirrors CPython's `_Py_c_pow` from `Objects/complexobject.c`:
///   - For small non-negative integer exponents (`wi == 0`, `wr` is an
///     integer in `0..=100`), use repeated squaring so that results like
///     `(1+1j)**2 == 2j` are exact (no floating-point rounding in the
///     imaginary part).
///   - General case uses `r = |z|` (hypot), `ln_r = ln(r)`, `t = arg(z)`:
///     `len = pow(r, wr) * exp(-wi * t)`,  `at = wr*t + wi*ln_r`,
///     `result = len * (cos(at) + i*sin(at))`.
///     Using `pow(r, wr)` rather than `exp(wr*ln_r)` matches CPython's
///     rounding for cases like `(2+0j)**0.5`.
///
/// Special cases (CPython parity):
///   - `w == 0+0j` → `(1+0j)` for any `z` (including `0j ** 0`).
///   - `z == 0+0j`, `wi != 0` or `wr < 0` → `ZeroDivisionError`.
///   - `z == 0+0j`, `wr > 0`, `wi == 0` → `0j`.
fn complex_pow(zr: f64, zi: f64, wr: f64, wi: f64) -> Result<Value> {
    // z^0 = 1 for any z (including 0j ** 0).
    if wr == 0.0 && wi == 0.0 {
        return Ok(Value::complex(1.0, 0.0));
    }

    let abs_r = zr.hypot(zi); // |z| = sqrt(zr² + zi²)
    if abs_r == 0.0 {
        // 0j ** w where w != 0.
        // CPython raises ZeroDivisionError when the exponent has a nonzero
        // imaginary part or a negative real part.
        if wi != 0.0 || wr < 0.0 {
            return Err(pyrust_core::zerodiv_err!("0.0 to a negative or complex power"));
        }
        // wr > 0, wi == 0: 0j ** positive_real = 0j.
        return Ok(Value::complex(0.0, 0.0));
    }

    // CPython optimisation: use repeated squaring for small integer
    // exponents (wi==0, |wr| <= 100, wr == floor(wr)).
    // This avoids rounding error in the exp/log path so that, e.g.,
    // `(1+1j)**2` returns exactly `2j` rather than `(1.22e-16+2j)`.
    // Negative exponents use the same squaring on |n| and then invert:
    // `z**(-n) = 1 / z**n`.  CPython's `_Py_c_pow` applies the same
    // |wr| <= 100 bound for both positive and negative integers.
    if wi == 0.0 {
        let n = wr as i64;
        if n as f64 == wr && (-100..=100).contains(&n) {
            let (mut re, mut im) = (1.0_f64, 0.0_f64);
            let (mut br, mut bi) = (zr, zi);
            let mut exp = n.unsigned_abs(); // works for n == i64::MIN too (can't happen: |n|<=100)
            while exp > 0 {
                if exp & 1 == 1 {
                    let new_re = re * br - im * bi;
                    let new_im = re * bi + im * br;
                    re = new_re;
                    im = new_im;
                }
                let new_br = br * br - bi * bi;
                let new_bi = 2.0 * br * bi;
                br = new_br;
                bi = new_bi;
                exp >>= 1;
            }
            if n < 0 {
                // Invert: 1/(re + im*j) using the c_quot form from CPython's
                // complexobject.c so that signed-zero behaviour matches.
                // c_quot(1+0j, re+im*j):
                //   result_re = (1*re + 0*im) / (re²+im²)
                //   result_im = (0*re - 1*im) / (re²+im²)
                // Writing im as `0.0 * old_re - 1.0 * im` rather than `-im`
                // preserves positive zero when im == +0.0 (0.0*old_re yields
                // +0.0, then +0.0 - +0.0 == +0.0; direct negation of +0.0
                // yields -0.0, which diverges from CPython).
                let denom = re * re + im * im;
                let old_re = re;
                re = (1.0 * old_re + 0.0 * im) / denom;
                im = (0.0 * old_re - 1.0 * im) / denom;
            }
            return Ok(Value::complex(re, im));
        }
    }

    // General case: matches CPython's `_Py_c_pow` from complexobject.c.
    // Using pow(r, wr) rather than exp(wr * ln_r) is deliberate:
    // `exp(0.5 * ln(2))` and `pow(2.0, 0.5)` differ by 1 ULP; CPython
    // uses the `pow` path, so we must match it for parity.
    let ln_r = abs_r.ln();
    let t = zi.atan2(zr);
    let len = abs_r.powf(wr) * (-wi * t).exp();
    if len.is_infinite() {
        // CPython's _Py_c_pow sets errno = ERANGE when `len` overflows to
        // infinity and the caller raises OverflowError (e.g.
        // `(1+1j) ** 10**20` → `OverflowError: complex exponentiation`).
        return Err(pyrust_core::overflow_err!("complex exponentiation"));
    }
    let at = wr * t + wi * ln_r;
    Ok(Value::complex(len * at.cos(), len * at.sin()))
}

/// True if `v` can serve as an operand in `X | Y` (PEP 604).
/// Valid operands: `PyClass`, `BuiltinFunction` type tokens (like `range`, `generator`),
/// `None` itself (coerced to the `NoneType` PyClass singleton), and existing `UnionType` values.
fn is_union_operand(v: &Value) -> bool {
    match v.kind() {
        ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_) | ValueKind::None => true,
        ValueKind::BuiltinObject { ops, .. } => {
            ops.type_name() == pyrust_builtins::union_type::TYPE_NAME
        }
        _ => false,
    }
}

/// True if `v` is a "type-like" PEP 604 operand — a `PyClass`, a `BuiltinFunction`
/// acting as a type token, or an existing `UnionType`.  `None` is excluded: it
/// can appear in a union *only* when the other operand is a type, so at least
/// one side must satisfy this stricter predicate.  This matches CPython's
/// behaviour where `None | None` raises TypeError but `int | None` succeeds
/// (dispatched through `type.__or__`).
fn is_strict_type_union_operand(v: &Value) -> bool {
    match v.kind() {
        ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_) => true,
        ValueKind::BuiltinObject { ops, .. } => {
            ops.type_name() == pyrust_builtins::union_type::TYPE_NAME
        }
        _ => false,
    }
}

/// Convert `None` to the `NoneType` PyClass singleton, leaving all other
/// values unchanged.  Used when assembling union components so that
/// `int | None` stores `NoneType` as the component (matching CPython).
fn coerce_none_to_nonetype(v: Value) -> Value {
    if v.is_none() {
        Value::py_class(crate::interpreter::primitive_class_by_name("NoneType").expect("NoneType singleton"))
    } else {
        v
    }
}

impl Interpreter {
    /// Evaluate `obj[idx]` and return the result.
    ///
    /// Extracted from the `GetItem` VM dispatch arm so that changes to
    /// subscript-access semantics (__getitem__, slice handling, etc.) only
    /// require touching this method rather than vm.rs.
    pub(crate) fn exec_get_item(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        idx: crate::bytecode::Reg,
    ) -> Result<Value> {
        let fast_int_idx = regs[idx as usize].as_int();
        if let Some(raw_i) = fast_int_idx {
            enum Got {
                Item(Value),
                ListOOR,
                TupleOOR,
                None,
            }
            let got = match regs[obj as usize].as_some().map(|v| v.kind()) {
                Some(ValueKind::List(items)) => {
                    let len = items.len() as i64;
                    let j = if raw_i < 0 { raw_i + len } else { raw_i };
                    if j >= 0 && (j as usize) < items.len() {
                        Got::Item(items[j as usize].clone())
                    } else {
                        Got::ListOOR
                    }
                }
                Some(ValueKind::Tuple(items)) => {
                    let len = items.len() as i64;
                    let j = if raw_i < 0 { raw_i + len } else { raw_i };
                    if j >= 0 && (j as usize) < items.len() {
                        Got::Item(items[j as usize].clone())
                    } else {
                        Got::TupleOOR
                    }
                }
                _ => Got::None,
            };
            match got {
                Got::Item(v) => return Ok(v),
                Got::ListOOR => {
                    return Err(pyrust_core::index_err!("list index out of range"));
                }
                Got::TupleOOR => {
                    return Err(pyrust_core::index_err!("tuple index out of range"));
                }
                Got::None => {}
            }
        }

        let idx_val = vm_read(regs, idx, num_locals)?;
        let obj_is_mapping = matches!(
            regs[obj as usize].as_some().map(|v| v.kind()),
            Some(ValueKind::Dict(_) | ValueKind::BuiltinObject { .. })
        );
        if !obj_is_mapping
            && let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                let obj_val = vm_read(regs, obj, num_locals)?;
                return self.eval_slice(&obj_val, lo, hi, st);
            }
        enum FastResult {
            Value(Value),
            DictLookup(Value),
            Miss,
        }
        let fast = if let Some(ov) = regs[obj as usize].as_some() {
            match ov.kind() {
                ValueKind::List(items) => {
                    if !matches!(
                        idx_val.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    ) {
                        FastResult::Miss
                    } else {
                        let i = normalize_index(&idx_val, items.len(), "list")?;
                        FastResult::Value(items[i].clone())
                    }
                }
                ValueKind::Tuple(items) => {
                    if !matches!(
                        idx_val.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    ) {
                        FastResult::Miss
                    } else {
                        let i = normalize_index(&idx_val, items.len(), "tuple")?;
                        FastResult::Value(items[i].clone())
                    }
                }
                ValueKind::Dict(_) => FastResult::DictLookup(ov.clone()),
                _ => FastResult::Miss,
            }
        } else {
            FastResult::Miss
        };
        match fast {
            FastResult::Value(r) => Ok(r),
            FastResult::DictLookup(dict_val) => {
                let lookup = if let Some(s) = idx_val.as_str() {
                    self.dict_str_lookup(&dict_val, s)?
                } else {
                    let key = self.value_to_pykey(&idx_val)?;
                    self.dict_lookup(&dict_val, &key)?
                };
                lookup
                    .map(|(_, v)| v)
                    .ok_or_else(|| PyError::key_error(idx_val.clone()))
            }
            FastResult::Miss => {
                let obj_val = vm_read(regs, obj, num_locals)?;
                // bytearray honors __index__ on a non-int subscript like bytes
                // (#1908).  Resolve only for a PyInstance index — this check is
                // reached only after the int/list/tuple fast paths miss, so the
                // hot `ba[i]` path never runs it.
                if matches!(idx_val.kind(), ValueKind::PyInstance(_))
                    && matches!(
                        obj_val.kind(),
                        ValueKind::BuiltinObject { ops, .. }
                            if ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME
                    )
                {
                    let resolved = self.call_index_protocol(&idx_val, "bytearray")?;
                    return self.eval_index(&obj_val, resolved);
                }
                self.eval_index(&obj_val, idx_val)
            }
        }
    }

    /// Execute an rvalue slice read `obj[lo:hi:step]` (the `GetSlice` opcode,
    /// CPython BINARY_SLICE analogue).  Reads the three contiguous bound
    /// registers (`base`, `base+1`, `base+2`) and slices `obj` directly via
    /// `eval_slice`, which only materialises a real `slice` object for the
    /// PyInstance `__getitem__` / BuiltinObject paths — built-in sequences skip
    /// the per-access `slice`-object allocation entirely (#1964).
    pub(crate) fn exec_get_slice(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        base: crate::bytecode::Reg,
    ) -> Result<Value> {
        let start = vm_read(regs, base, num_locals)?;
        let stop = vm_read(regs, base + 1, num_locals)?;
        let step = vm_read(regs, base + 2, num_locals)?;
        let lo = if start.is_none() { None } else { Some(start) };
        let hi = if stop.is_none() { None } else { Some(stop) };
        let st = if step.is_none() { None } else { Some(step) };
        // `tuple` is the only source kind whose `Value::clone` is a full O(n)
        // deep copy of the backing `Vec` (list/bytes/str clones are an Rc bump,
        // a shared-buffer slice, or a bit copy).  Cloning the source before
        // slicing therefore turned every tuple slice into an O(source_len) copy
        // *before* the slice even ran — the dominant cost in the #2114 tuple
        // pathology (a `t[100:100]` of a 1000-tuple was 25× slower than the
        // list equivalent).  Slice the tuple through a borrow so the source is
        // never cloned.  Every other kind keeps the exact master path
        // (`vm_read` → owned clone), so the list/bytes/str hot loops are
        // byte-identical to before.
        //
        // `is_tuple()` is a NaN-box tag compare on the raw register — no
        // `RefCell` borrow, no clone — so the non-tuple branch below reaches
        // `vm_read` having done only this one extra compare.
        if regs[obj as usize].is_tuple() {
            let obj_ref = vm_read_ref(regs, obj, num_locals)?;
            // SAFETY/soundness: only take the borrow fast path when every bound
            // is a plain int/None.  `eval_slice` resolves the bounds via
            // `resolve_slice_bound_val`, which runs no Python code for
            // None/Int/Bool/BigInt but dispatches user `__index__` otherwise —
            // and that `__index__` can reassign the source register
            // (`t[Evil():900]` with `Evil.__index__` doing `t = ...`), freeing
            // the tuple's backing `Vec` while we hold a reference into it
            // (use-after-free).  When a bound is an `__index__` object we fall
            // through to the owned-clone path below, which is independent of the
            // register and so survives any reassignment — identical to master.
            let bound_is_plain = |v: &Value| {
                v.is_none()
                    || matches!(
                        v.kind(),
                        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
                    )
            };
            if lo.as_ref().is_none_or(&bound_is_plain)
                && hi.as_ref().is_none_or(&bound_is_plain)
                && st.as_ref().is_none_or(&bound_is_plain)
            {
                return self.eval_slice(obj_ref, lo, hi, st);
            }
        }
        let obj_val = vm_read(regs, obj, num_locals)?;
        // Mapping targets (dict) treat slice notation as a *key lookup*, not a
        // slice: `d[1:2]` builds the slice object and looks it up as a key
        // (KeyError if absent), matching CPython and the prior BuildSlice +
        // GetItem path.  Build a real slice object and dispatch through
        // eval_index so the dict lookup runs.  eval_slice handles every other
        // target (built-in sequences, range, BuiltinObject, PyInstance).
        if matches!(obj_val.kind(), ValueKind::Dict(_)) {
            let slice_val = pyrust_builtins::slice::make_slice(lo, hi, st);
            return self.eval_index(&obj_val, slice_val);
        }
        self.eval_slice(&obj_val, lo, hi, st)
    }

    /// Execute `obj[idx] = val`.
    ///
    /// Extracted from the `SetItem` VM dispatch arm so that changes to
    /// subscript-assignment semantics (__setitem__, slice assignment, etc.)
    /// only require touching this method rather than vm.rs.
    pub(crate) fn exec_set_item(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        idx: crate::bytecode::Reg,
        val: crate::bytecode::Reg,
    ) -> Result<()> {
        if let Some(raw_i) = regs[idx as usize].as_int()
            && let Some(len) = regs[obj as usize].list_len() {
                let j = if raw_i < 0 { raw_i + len as i64 } else { raw_i };
                if j >= 0 && (j as usize) < len {
                    let v = regs[val as usize].clone();
                    regs[obj as usize].list_with_mut(|items| {
                        items[j as usize] = v;
                    });
                } else {
                    return Err(pyrust_core::index_err!("list assignment index out of range"));
                }
                return Ok(());
            }
        let idx_val = vm_read(regs, idx, num_locals)?;
        let val_val = vm_read(regs, val, num_locals)?;
        let is_list_target = regs[obj as usize].list_len().is_some();
        if is_list_target
            && let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                let lo = self.resolve_slice_bound_val(lo)?;
                let hi = self.resolve_slice_bound_val(hi)?;
                let st = self.resolve_slice_bound_val(st)?;
                let new_items: Vec<Value> = match val_val.kind() {
                    ValueKind::List(v) => Some(v.to_vec()),
                    _ => None,
                }
                .unwrap_or_else(Vec::new);
                let new_items = if !new_items.is_empty()
                    || matches!(val_val.kind(), ValueKind::List(_))
                {
                    new_items
                } else {
                    self.collect_iterable(&val_val).map_err(|_| {
                        pyrust_core::type_err!("can only assign an iterable")
                    })?
                };
                let updated = regs[obj as usize].list_with_mut(|items| {
                    Self::slice_setitem(items, lo.as_ref(), hi.as_ref(), st.as_ref(), new_items)
                });
                return match updated {
                    Some(r) => r,
                    None => {
                        let tname = value_type_name_str(&regs[obj as usize]);
                        Err(pyrust_core::type_err!("'{}' object does not support item assignment", tname))
                    }
                };
            }
        let target_kind = regs[obj as usize]
            .as_some()
            .map(|v| match v.kind() {
                ValueKind::List(_) => 1u8,
                ValueKind::Dict(_) => 2u8,
                ValueKind::PyInstance(_) => 3u8,
                ValueKind::BuiltinObject { .. } => 4u8,
                _ => 0u8,
            })
            .unwrap_or(0);
        match target_kind {
            1 => {
                let len = regs[obj as usize].list_len().unwrap_or(0);
                let idx_resolved = self.call_index_protocol(&idx_val, "list")?;
                let i = normalize_index_write(&idx_resolved, len, "list")?;
                regs[obj as usize].list_with_mut(|items| {
                    items[i] = val_val;
                });
            }
            2 => {
                self.set_item_into_dict(regs, obj, idx_val, val_val)?;
            }
            3 => {
                let obj_val = vm_read(regs, obj, num_locals)?;
                if let ValueKind::PyInstance(inst) = obj_val.kind() {
                    let inst_rc = Rc::clone(inst);
                    if let Some(backing) = builtin_data_backing(&obj_val) {
                        enum BkKind {
                            Dict,
                            List,
                            Other,
                        }
                        let bk_kind = match backing.kind() {
                            ValueKind::Dict(_) => BkKind::Dict,
                            ValueKind::List(_) => BkKind::List,
                            _ => BkKind::Other,
                        };
                        // Issue #2010: a dict subclass that *overrides*
                        // `__setitem__` (e.g. `collections.Counter` /
                        // `defaultdict`, whose stores are `__eq__`-aware) must
                        // have its override called rather than the raw
                        // backing-dict insert — but the two paths only diverge
                        // for object keys (`PyKey::Object`), where user
                        // `__eq__` dedup matters.  Primitive keys (int / str /
                        // …) insert identically either way, so the override
                        // probe is gated on a `PyInstance` index: a plain
                        // `class D(dict)` doing `d[i] = v` with int keys keeps
                        // the allocation- and walk-free hot path, while
                        // `counter[obj] = v` routes through the override.
                        if matches!(bk_kind, BkKind::Dict)
                            && matches!(idx_val.kind(), ValueKind::PyInstance(_))
                        {
                            let class = Rc::clone(&inst_rc.borrow().class);
                            let user_setitem =
                                lookup_class_attr(&class, "__setitem__").filter(|v| {
                                    !matches!(
                                        v.kind(),
                                        ValueKind::BuiltinFunction(
                                            "dict.__setitem__" | "list.__setitem__"
                                        )
                                    )
                                });
                            if let Some(method_val) = user_setitem {
                                invoke_class_method(
                                    self,
                                    method_val,
                                    Value::py_instance(inst_rc),
                                    &[
                                        ExpandedCallArg { name: None, value: idx_val },
                                        ExpandedCallArg { name: None, value: val_val },
                                    ],
                                )?;
                                return Ok(());
                            }
                        }
                        match bk_kind {
                            BkKind::Dict => {
                                let key = self.value_to_pykey(&idx_val)?;
                                backing.dict_with_mut(|dict| {
                                    dict.insert(key, val_val);
                                });
                                return Ok(());
                            }
                            BkKind::List => {
                                let len = backing.list_len().unwrap_or(0);
                                let idx_resolved =
                                    self.call_index_protocol(&idx_val, "list")?;
                                let i = normalize_index_write(&idx_resolved, len, "list")?;
                                backing.list_with_mut(|items| {
                                    items[i] = val_val;
                                });
                                return Ok(());
                            }
                            BkKind::Other => {}
                        }
                    }
                    let class = Rc::clone(&inst_rc.borrow().class);
                    if let Some(method_val) = lookup_class_attr(&class, "__setitem__") {
                        invoke_class_method(
                            self,
                            method_val,
                            Value::py_instance(inst_rc),
                            &[
                                ExpandedCallArg {
                                    name: None,
                                    value: idx_val,
                                },
                                ExpandedCallArg {
                                    name: None,
                                    value: val_val,
                                },
                            ],
                        )?;
                        return Ok(());
                    }
                    let class_name = class.borrow().name.clone();
                    return Err(pyrust_core::type_err!("'{}' object does not support item assignment", class_name));
                }
                let tname = value_type_name_str(&regs[obj as usize]);
                return Err(pyrust_core::type_err!("'{}' object does not support item assignment", tname));
            }
            4 => {
                // bytearray item assignment honors the __index__ protocol on
                // both the index and the assigned value (#1908). bytearray's
                // receiver-only set_item can't reach user dunders, so resolve
                // here before delegating. The hot `ba[i] = v` int/bool path is
                // untouched: protocol resolution only runs when the index is a
                // PyInstance / slice object or the value is a PyInstance — a
                // plain-int index with a plain-int/bool value skips it entirely
                // and goes straight to set_item, matching master.
                let needs_resolve = matches!(
                    idx_val.kind(),
                    ValueKind::PyInstance(_) | ValueKind::BuiltinObject { .. }
                ) || matches!(val_val.kind(), ValueKind::PyInstance(_));
                let (idx_val, val_val) = if needs_resolve
                    && matches!(
                        vm_read(regs, obj, num_locals)?.kind(),
                        ValueKind::BuiltinObject { ops, .. }
                            if ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME
                    ) {
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        // Slice assignment: resolve the __index__ bounds and
                        // rebuild the slice; element resolution stays in
                        // set_item (#1908).
                        let lo = self.resolve_slice_bound_val(lo)?;
                        let hi = self.resolve_slice_bound_val(hi)?;
                        let st = self.resolve_slice_bound_val(st)?;
                        (pyrust_builtins::slice::make_slice(lo, hi, st), val_val)
                    } else {
                        let resolved_idx = self.call_index_protocol(&idx_val, "bytearray")?;
                        // Resolve the assigned value's __index__ only when it is
                        // a PyInstance carrying one; otherwise leave it untouched
                        // so set_item's value_to_byte produces the correct error
                        // ("byte must be in range(0, 256)" / "'X' object cannot be
                        // interpreted as an integer").
                        let resolved_val = self.resolve_byte_value(val_val)?;
                        (resolved_idx, resolved_val)
                    }
                } else {
                    (idx_val, val_val)
                };
                let obj_val = vm_read(regs, obj, num_locals)?;
                if let ValueKind::BuiltinObject { ops, state } = obj_val.kind() {
                    ops.set_item(state, &idx_val, val_val)?;
                }
            }
            _ => {
                let tname = value_type_name_str(&regs[obj as usize]);
                return Err(pyrust_core::type_err!("'{}' object does not support item assignment", tname));
            }
        }
        Ok(())
    }

    /// Assign into a dict target for `obj[idx] = val`, including the
    /// module-globals write-through (issue #970): when the dict is
    /// `module_globals_dict`, mirror the write to the script frame's
    /// fastlocal register and bump the LoadGlobal cache version.
    fn set_item_into_dict(
        &mut self,
        regs: &mut RegSlice,
        obj: crate::bytecode::Reg,
        idx_val: Value,
        val_val: Value,
    ) -> Result<()> {
        let key = self.value_to_pykey(&idx_val)?;
        let globals_sync_name: Option<String> = if self.globals_accessed {
            if let PyKey::Str(name_val) = &key {
                let is_globals = regs[obj as usize]
                    .get_dict_rc()
                    .zip(self.module_globals_dict.get_dict_rc())
                    .map(|(a, b)| Rc::ptr_eq(a, b))
                    .unwrap_or(false);
                if is_globals {
                    name_val.as_str().map(|s| s.to_owned())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        let val_for_fastlocal: Option<Value> =
            globals_sync_name.as_ref().map(|_| val_val.clone());
        let needs_dedup = match &key {
            PyKey::Object { .. } => true,
            // Issue #2059: `d[k] = v` where `k` is a tuple/frozenset key nesting
            // a user object must overwrite an `__eq__`-equal-but-distinct
            // existing key instead of appending a duplicate.  `dict_lookup`
            // below performs the eq-aware probe.
            PyKey::Tuple(_) | PyKey::FrozenSet(_) if nested_object_tuple_key(&key) => true,
            PyKey::None => {
                let none_hash = pyrust_core::py_hash_none() as u64;
                regs[obj as usize]
                    .as_dict()
                    .map(|d| {
                        d.keys().any(|k| {
                            matches!(k, PyKey::Object { hash, .. } if *hash == none_hash)
                        })
                    })
                    .unwrap_or(false)
            }
            _ => false,
        };
        if needs_dedup {
            let dict_val = regs[obj as usize]
                .as_some()
                .cloned()
                .unwrap_or(Value::none());
            let existing = self.dict_lookup(&dict_val, &key)?;
            regs[obj as usize].dict_with_mut(|dict| {
                if let Some((idx, _)) = existing {
                    let existing_key = dict.get_index(idx).map(|(k, _)| k.clone());
                    if let Some(k) = existing_key {
                        dict.insert(k, val_val);
                    } else {
                        dict.insert(key, val_val);
                    }
                } else {
                    dict.insert(key, val_val);
                }
            });
        } else {
            regs[obj as usize].dict_with_mut(|dict| {
                dict.insert(key, val_val);
            });
        }
        if let (Some(name), Some(synced_val)) = (globals_sync_name, val_for_fastlocal) {
            bump_global_env_version(self);
            if let Some(script_view) = self
                .vm_frame_views
                .iter()
                .find(|v| v.kind == FrameKind::Script)
                && let Some(&slot) = script_view.local_index.get(&name) {
                    let slot = slot as usize;
                    if slot < script_view.regs_len {
                        // SAFETY: slot < regs_len; regs_ptr is the script frame's
                        // register file.  RegSlice carries no `noalias`, so this
                        // write does not violate aliasing rules (issue #547, PR #646).
                        unsafe {
                            *script_view.regs_ptr.add(slot).as_mut() = synced_val;
                        }
                    }
                }
        }
        Ok(())
    }


    /// Execute `del obj[idx]`.
    ///
    /// Extracted from the `DeleteItem` VM dispatch arm so that changes to
    /// subscript-deletion semantics (__delitem__, slice deletion, etc.) only
    /// require touching this method rather than vm.rs.
    pub(crate) fn exec_delete_item(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        idx: crate::bytecode::Reg,
    ) -> Result<()> {
        let idx_val = vm_read(regs, idx, num_locals)?;
        let is_list_target = regs[obj as usize].list_len().is_some();
        if is_list_target
            && let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                let lo = self.resolve_slice_bound_val(lo)?;
                let hi = self.resolve_slice_bound_val(hi)?;
                let st = self.resolve_slice_bound_val(st)?;
                let updated = regs[obj as usize].list_with_mut(|items| {
                    Self::slice_delitem(items, lo.as_ref(), hi.as_ref(), st.as_ref())
                });
                return match updated {
                    Some(r) => r,
                    None => {
                        let tname = value_type_name_str(&regs[obj as usize]);
                        Err(pyrust_core::type_err!("'{}' object does not support item deletion", tname))
                    }
                };
            }
        let target_kind = regs[obj as usize]
            .as_some()
            .map(|v| match v.kind() {
                ValueKind::List(_) => 1u8,
                ValueKind::Dict(_) => 2u8,
                ValueKind::BuiltinObject { .. } => 3u8,
                _ => 0u8,
            })
            .unwrap_or(0);
        if target_kind == 1 {
            let len = regs[obj as usize].list_len().unwrap_or(0);
            let idx_resolved = self.call_index_protocol(&idx_val, "list")?;
            let i = normalize_index_write(&idx_resolved, len, "list")?;
            regs[obj as usize].list_with_mut(|items| {
                if i + 1 == items.len() {
                    items.pop();
                } else {
                    items.remove(i);
                }
            });
            return Ok(());
        }
        if target_kind == 2 {
            let key = self.value_to_pykey(&idx_val)?;
            let globals_del_name: Option<String> = if self.globals_accessed {
                if let PyKey::Str(name_val) = &key {
                    let is_globals = regs[obj as usize]
                        .get_dict_rc()
                        .zip(self.module_globals_dict.get_dict_rc())
                        .map(|(a, b)| Rc::ptr_eq(a, b))
                        .unwrap_or(false);
                    if is_globals {
                        name_val.as_str().map(|s| s.to_owned())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            // A bare `Object` key, or a tuple/frozenset key nesting a user
            // object (issue #2059), must use the eq-aware `dict_lookup` so that
            // `del d[k]` finds an `__eq__`-equal-but-distinct stored key rather
            // than relying on raw `PyKey` identity.
            if matches!(&key, PyKey::Object { .. }) || nested_object_tuple_key(&key) {
                let dict_val = regs[obj as usize]
                    .as_some()
                    .cloned()
                    .unwrap_or(Value::none());
                let found = self.dict_lookup(&dict_val, &key)?;
                if let Some((idx, _)) = found {
                    regs[obj as usize].dict_with_mut(|dict| {
                        dict.shift_remove_index(idx);
                    });
                } else {
                    return Err(PyError::key_error(idx_val.clone()));
                }
            } else {
                let removed = regs[obj as usize].dict_with_mut(|dict| dict.shift_remove(&key));
                if !matches!(removed, Some(Some(_))) {
                    return Err(PyError::key_error(idx_val.clone()));
                }
            }
            if let Some(name) = globals_del_name {
                bump_global_env_version(self);
                if let Some(script_view) = self
                    .vm_frame_views
                    .iter()
                    .find(|v| v.kind == FrameKind::Script)
                    && let Some(&slot) = script_view.local_index.get(&name) {
                        let slot = slot as usize;
                        if slot < script_view.regs_len {
                            // SAFETY: same as SetItem (issue #547, PR #646).
                            unsafe {
                                *script_view.regs_ptr.add(slot).as_mut() = Value::unset();
                            }
                        }
                    }
            }
            return Ok(());
        }
        if target_kind == 3 {
            let obj_val = vm_read(regs, obj, num_locals)?;
            if let ValueKind::BuiltinObject { ops, state } = obj_val.kind() {
                ops.delete_item(state, &idx_val)?;
            }
            return Ok(());
        }
        let obj_val = vm_read(regs, obj, num_locals)?;
        if let ValueKind::PyInstance(inst) = obj_val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__delitem__") {
                invoke_class_method(
                    self,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[ExpandedCallArg {
                        name: None,
                        value: idx_val,
                    }],
                )?;
                return Ok(());
            }
            let class_name = class.borrow().name.clone();
            return Err(pyrust_core::type_err!("'{class_name}' object does not support item deletion"));
        }
        let tname = value_type_name_str(&regs[obj as usize]);
        let msg = if Self::unpack_slice_key(&idx_val).is_some() {
            format!("'{}' object does not support item deletion", tname)
        } else {
            format!("'{}' object doesn't support item deletion", tname)
        };
        Err(pyrust_core::type_err!(msg))
    }
}
