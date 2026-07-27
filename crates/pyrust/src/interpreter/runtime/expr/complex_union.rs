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
        ValueKind::PyInstance(_) => {
            builtin_data_backing(v).is_some_and(|b| matches!(b.kind(), ValueKind::Complex(_, _)))
        }
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
            return Err(pyrust_core::zerodiv_err!(
                "0.0 to a negative or complex power"
            ));
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
        ValueKind::BuiltinObject { .. } => pyrust_builtins::union_type::is_union_type(v),
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
        ValueKind::BuiltinObject { .. } => pyrust_builtins::union_type::is_union_type(v),
        _ => false,
    }
}

/// Convert `None` to the `NoneType` PyClass singleton, leaving all other
/// values unchanged.  Used when assembling union components so that
/// `int | None` stores `NoneType` as the component (matching CPython).
fn coerce_none_to_nonetype(v: Value) -> Value {
    if v.is_none() {
        Value::py_class(crate::interpreter::canonical_class_by_tag(
            pyrust_core::CanonicalClassTag::NoneType,
        ))
    } else {
        v
    }
}
