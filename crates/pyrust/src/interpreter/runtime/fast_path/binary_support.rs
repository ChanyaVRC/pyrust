/// CPython `unicode_concatenate` fast path for `s += t` (issue #2850).
///
/// When an augmented `+=` stores back into the same register as its left
/// operand and both operands are plain `str`, append `t`'s bytes into `s`'s
/// backing in place (relying on `realloc` for amortised O(1)) instead of
/// building a brand-new concatenated string every iteration — turning the
/// classic `s += "x"` loop from O(n²) into O(n).
///
/// Returns `true` when it handled the op (the result is already stored in
/// `regs[dst]`); `false` when the shape doesn't qualify (op isn't `Add`,
/// `dst != lhs`, or either operand isn't a plain `str`), in which case the
/// caller proceeds with its existing path unchanged.
///
/// Str-subclasses are `PyInstance` values (not `TAG_STR`), so `is_str()`
/// naturally excludes them; bytes are `TAG_OPAQUE`, so `bytes += bytes` never
/// reaches here.
#[inline]
fn try_str_inplace_concat(
    regs: &mut RegSlice,
    dst: crate::bytecode::Reg,
    lhs: crate::bytecode::Reg,
    op: BinaryOp,
    rhs: &Value,
) -> bool {
    if op != BinaryOp::Add || dst != lhs {
        return false;
    }
    if !regs[lhs as usize].is_str() || !rhs.is_str() {
        return false;
    }
    let rhs_str = rhs.as_str().unwrap();
    let mut left = std::mem::replace(&mut regs[lhs as usize], Value::unset());
    if left.str_append_in_place(rhs_str) {
        regs[dst as usize] = left;
    } else {
        let mut s = String::with_capacity(left.as_str().unwrap().len() + rhs_str.len());
        s.push_str(left.as_str().unwrap());
        s.push_str(rhs_str);
        regs[dst as usize] = Value::string(s);
    }
    true
}

impl Interpreter {
    #[inline(always)]
    pub(super) fn exec_binop_in_place(
        &mut self,
        regs: &mut RegSlice,
        dst: crate::bytecode::Reg,
        lhs: crate::bytecode::Reg,
        op: BinaryOp,
        rhs: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        if let (Some(left), Some(right)) =
            (regs[lhs as usize].as_int(), regs[rhs as usize].as_int())
            && let Some(result) = int_int_fast(left, right, op)
        {
            regs[dst as usize] = result;
            return Ok(());
        }
        if let Some(result) = num_binop_fast(&regs[lhs as usize], &regs[rhs as usize], op) {
            regs[dst as usize] = result;
            return Ok(());
        }
        if regs[lhs as usize].is_str() && regs[rhs as usize].is_str() {
            let right = regs[rhs as usize].clone();
            if try_str_inplace_concat(regs, dst, lhs, op, &right) {
                return Ok(());
            }
        }

        let left = vm_read(regs, lhs, num_locals)?;
        let right = vm_read(regs, rhs, num_locals)?;
        let result = if let Some(value) = self.try_inplace_op(&left, op, &right, true)? {
            value
        } else {
            self.eval_binary_aug(left, op, right)?
        };
        regs[dst as usize] = result;
        Ok(())
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_binop_const(
        &mut self,
        regs: &mut RegSlice,
        dst: crate::bytecode::Reg,
        lhs: crate::bytecode::Reg,
        op: BinaryOp,
        constant: &Value,
        is_augmented: bool,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        if let (Some(left), Some(right)) = (regs[lhs as usize].as_int(), constant.as_int())
            && let Some(result) = int_int_fast(left, right, op)
        {
            regs[dst as usize] = result;
            return Ok(());
        }
        if let Some(result) = num_binop_fast(&regs[lhs as usize], constant, op) {
            regs[dst as usize] = result;
            return Ok(());
        }
        if is_augmented && regs[lhs as usize].is_str() && constant.is_str() {
            let right = constant.clone();
            if try_str_inplace_concat(regs, dst, lhs, op, &right) {
                return Ok(());
            }
        }

        let left = vm_read(regs, lhs, num_locals)?;
        let right = constant.clone();
        let result = if let Some(value) = self.try_inplace_op(&left, op, &right, is_augmented)? {
            value
        } else if is_augmented {
            self.eval_binary_aug(left, op, right)?
        } else {
            self.eval_binary(left, op, right)?
        };
        regs[dst as usize] = result;
        Ok(())
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_binop_immediate(
        &mut self,
        regs: &mut RegSlice,
        dst: crate::bytecode::Reg,
        lhs: crate::bytecode::Reg,
        op: BinaryOp,
        immediate: i16,
        is_augmented: bool,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let right_int = immediate as i64;
        if let Some(left) = regs[lhs as usize].as_int()
            && let Some(result) = int_int_fast(left, right_int, op)
        {
            regs[dst as usize] = result;
            return Ok(());
        }

        let right = Value::int(right_int);
        if let Some(result) = num_binop_fast(&regs[lhs as usize], &right, op) {
            regs[dst as usize] = result;
            return Ok(());
        }

        let left = vm_read(regs, lhs, num_locals)?;
        let result = if let Some(value) = self.try_inplace_op(&left, op, &right, is_augmented)? {
            value
        } else if is_augmented {
            self.eval_binary_aug(left, op, right)?
        } else {
            self.eval_binary(left, op, right)?
        };
        regs[dst as usize] = result;
        Ok(())
    }

    /// Evaluate a compiler-fused, left-to-right `+` chain.
    ///
    /// Strings concatenate in one allocation and small integers accumulate
    /// without intermediate objects. An unsupported operand resumes the
    /// canonical binary protocol from the accumulated prefix.
    pub(super) fn eval_concat_fast(
        &mut self,
        regs: &RegSlice,
        base: crate::bytecode::Reg,
        count: u8,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let base_index = base as usize;
        let count = count as usize;

        if (0..count).all(|index| regs[base_index + index].as_str().is_some()) {
            let total_len = (0..count)
                .map(|index| regs[base_index + index].as_str().unwrap().len())
                .sum();
            let mut result = String::with_capacity(total_len);
            for index in 0..count {
                result.push_str(regs[base_index + index].as_str().unwrap());
            }
            return Ok(Value::string(result));
        }

        if let Some(mut accumulator) = regs[base_index].as_int()
            && let Some(second) = regs[base_index + 1].as_int()
            && let Some(sum) = accumulator.checked_add(second)
        {
            accumulator = sum;
            let mut index = 2;
            while index < count {
                let Some(value) = regs[base_index + index].as_int() else {
                    break;
                };
                let Some(sum) = accumulator.checked_add(value) else {
                    break;
                };
                accumulator = sum;
                index += 1;
            }
            if index == count {
                return Ok(Value::int(accumulator));
            }
            let mut result = Value::int(accumulator);
            while index < count {
                let next = vm_read(regs, base + index as u32, num_locals)?;
                result = self.eval_binary(result, BinaryOp::Add, next)?;
                index += 1;
            }
            return Ok(result);
        }

        let mut result = vm_read(regs, base, num_locals)?;
        for index in 1..count {
            let next = vm_read(regs, base + index as u32, num_locals)?;
            result = self.eval_binary(result, BinaryOp::Add, next)?;
        }
        Ok(result)
    }
}

#[inline(always)]
fn int_int_fast(a: i64, b: i64, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add => a.checked_add(b).map(Value::int),
        BinaryOp::Sub => a.checked_sub(b).map(Value::int),
        BinaryOp::Mul => a.checked_mul(b).map(Value::int),
        // `%` / `//`: `None` on `b == 0` (ZeroDivisionError) and on the sole
        // `i64::MIN // -1` overflow — eval_binary handles those. Every other
        // case matches `nb_mod` / `nb_floordiv`'s int-int arm exactly.
        BinaryOp::Mod => (b != 0).then(|| Value::int(py_mod_i64(a, b))),
        BinaryOp::FloorDiv => (b != 0)
            .then(|| py_mod_i64(a, b))
            .and_then(|m| a.checked_sub(m)?.checked_div(b))
            .map(Value::int),
        // `/` → float. Inline only when both operands are exactly f64-representable
        // (`|n| < 2^53`): IEEE division is then correctly rounded and byte-exact
        // with CPython. `b == 0` and larger magnitudes fall through to `nb_div`
        // for the ZeroDivisionError / exact bigint divider (#1923), matching its
        // int-int fast branch exactly.
        BinaryOp::Div => (b != 0 && a.unsigned_abs() < (1 << 53) && b.unsigned_abs() < (1 << 53))
            .then(|| Value::float(a as f64 / b as f64)),
        BinaryOp::BitAnd => Some(Value::int(a & b)),
        BinaryOp::BitOr => Some(Value::int(a | b)),
        BinaryOp::BitXor => Some(Value::int(a ^ b)),
        BinaryOp::LShift => {
            if b < 0 {
                // Negative shift → ValueError; fall through to eval_binary.
                None
            } else if b >= 64 {
                // Shift count ≥ 64: result is BigInt (or 0 for a==0).
                // Fall through to eval_binary which handles BigInt promotion.
                None
            } else {
                let n = b as u32;
                // Shift left then shift right; if we get back the original
                // value no significant bits were lost and the result fits i64.
                let r = a.wrapping_shl(n);
                if r.wrapping_shr(n) == a {
                    Some(Value::int(r))
                } else {
                    // Overflow: fall through for BigInt promotion.
                    None
                }
            }
        }
        BinaryOp::RShift => {
            if b < 0 {
                // Negative shift → ValueError; fall through to eval_binary.
                None
            } else if b >= 64 {
                // Saturate to sign bit (0 for non-negative, -1 for negative).
                // This is safe to handle here without BigInt.
                Some(Value::int(if a < 0 { -1 } else { 0 }))
            } else {
                Some(Value::int(a >> b))
            }
        }
        BinaryOp::Eq => Some(Value::bool_(a == b)),
        BinaryOp::Ne => Some(Value::bool_(a != b)),
        BinaryOp::Lt => Some(Value::bool_(a < b)),
        BinaryOp::Le => Some(Value::bool_(a <= b)),
        BinaryOp::Gt => Some(Value::bool_(a > b)),
        BinaryOp::Ge => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

/// Float-float fast path for arithmetic and comparison BinOps.
///
/// Returns `None` for:
/// - Ops that don't apply to floats (e.g. `BitAnd`).
/// - Cases where the Rust float result would diverge from CPython's
///   exception-raising behaviour: `Div`/`FloorDiv`/`Mod` by zero, and
///   `0.0 ** negative` for `Pow`.  The caller falls through to
///   `eval_binary` which raises the correct `ZeroDivisionError`.
///
/// NaN comparisons are handled correctly: Rust float comparisons with NaN
/// always return `false`, matching CPython's `float('nan') < x == False`.
#[inline(always)]
fn float_float_fast(a: f64, b: f64, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add => Some(Value::float(a + b)),
        BinaryOp::Sub => Some(Value::float(a - b)),
        BinaryOp::Mul => Some(Value::float(a * b)),
        BinaryOp::Div => {
            if b == 0.0 {
                // ZeroDivisionError: "float division by zero" — fall through.
                None
            } else {
                Some(Value::float(a / b))
            }
        }
        BinaryOp::FloorDiv => {
            if b == 0.0 {
                // ZeroDivisionError — fall through to eval_binary.
                None
            } else {
                // CPython's fmod-based float_divmod: handles inf/nan/signed-zero
                // and keeps `//` consistent with `divmod`/`%` (#2025).
                let (div, _) = float_divmod(a, b);
                Some(Value::float(div))
            }
        }
        BinaryOp::Mod => {
            if b == 0.0 {
                None
            } else {
                let mut r = a % b;
                // Match CPython float_rem: zero result copies sign of divisor;
                // non-zero result is adjusted so sign matches divisor.
                if r == 0.0 {
                    r = r.copysign(b);
                } else if r.signum() != b.signum() {
                    r += b;
                }
                Some(Value::float(r))
            }
        }
        BinaryOp::Pow => {
            // 0.0 ** negative → ZeroDivisionError in CPython; Rust returns ±inf.
            if a == 0.0 && b < 0.0 {
                None
            } else {
                Some(Value::float(a.powf(b)))
            }
        }
        BinaryOp::Eq => Some(Value::bool_(a == b)),
        BinaryOp::Ne => Some(Value::bool_(a != b)),
        BinaryOp::Lt => Some(Value::bool_(a < b)),
        BinaryOp::Le => Some(Value::bool_(a <= b)),
        BinaryOp::Gt => Some(Value::bool_(a > b)),
        BinaryOp::Ge => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

/// String fast path for BinOps that apply to `str`.
///
/// Currently handles `Add` (concatenation) and comparison operators.
/// Returns `None` for any op that doesn't apply to strings.
#[inline(always)]
fn str_str_fast(a: &str, b: &str, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add => {
            let mut s = String::with_capacity(a.len() + b.len());
            s.push_str(a);
            s.push_str(b);
            Some(Value::string(s))
        }
        BinaryOp::Eq => Some(Value::bool_(a == b)),
        BinaryOp::Ne => Some(Value::bool_(a != b)),
        BinaryOp::Lt => Some(Value::bool_(a < b)),
        BinaryOp::Le => Some(Value::bool_(a <= b)),
        BinaryOp::Gt => Some(Value::bool_(a > b)),
        BinaryOp::Ge => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

/// Classify two `Value` operands into a `BinopTypeTag` for the inline cache.
///
/// `Int` is returned only when both values tag as `TAG_INT` (or `BigInt`
/// that fits in i64 via `as_int()`).  `Float` requires both to be true floats
/// (`is_float()`).  `Str` requires both to be strings.  Everything else maps
/// to `Other`.  The tags are mutually exclusive because `as_int()` returns
/// `None` for floats and strings.
#[inline(always)]
fn classify_binop_tag(a: &Value, b: &Value) -> crate::bytecode::BinopTypeTag {
    use crate::bytecode::BinopTypeTag;
    // is_float() is a fast bit-mask check; check it first so Float beats Int
    // for Bool operands (Bool's tag is TAG_BOOL, not TAG_INT, so as_int won't
    // fire for bools, and this branch won't trip for them either).
    if a.is_float() && b.is_float() {
        return BinopTypeTag::Float;
    }
    if a.as_int().is_some() && b.as_int().is_some() {
        return BinopTypeTag::Int;
    }
    // Exactly one float + one int/bool: coercing numeric fast path.  (Both-float
    // and both-int are already handled above, so this only fires for the mixed
    // case.)
    if (a.is_float() && b.as_int().is_some()) || (a.as_int().is_some() && b.is_float()) {
        return BinopTypeTag::NumMixed;
    }
    if a.is_str() && b.is_str() {
        return BinopTypeTag::Str;
    }
    BinopTypeTag::Other
}

/// Coerce a numeric `Value` to `f64` for the mixed int/float fast path: floats
/// pass through; ints/bools convert via `as_int()` (round-to-nearest, matching
/// CPython's `PyLong_AsDouble`).  Returns `None` for non-numeric values and for
/// BigInts beyond i64 range (the caller falls through to `eval_binary`).
#[inline(always)]
fn coerce_num_f64(v: &Value) -> Option<f64> {
    if v.is_float() {
        Some(v.as_float_raw())
    } else {
        v.as_int().map(|i| i as f64)
    }
}

/// Mixed int/float fast path.  Only the arithmetic ops CPython itself computes
/// by coercing the int operand to `f64` are inlined — the coerced result is
/// byte-identical to CPython for every finite i64.  Comparisons (which need
/// exact int/float ordering for `|int| >= 2^53`) and `Pow` (which can yield a
/// complex result for a negative base) return `None` and fall through.
#[inline(always)]
fn num_mixed_fast(a: &Value, b: &Value, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add
        | BinaryOp::Sub
        | BinaryOp::Mul
        | BinaryOp::Div
        | BinaryOp::FloorDiv
        | BinaryOp::Mod => float_float_fast(coerce_num_f64(a)?, coerce_num_f64(b)?, op),
        _ => None,
    }
}

/// Unconditional float / mixed-numeric fast path for the `BinOpInPlace` /
/// `BinOpConst` / `BinOpImm` handlers, which don't carry an adaptive inline
/// cache (only `BinOp` does) and otherwise fall straight to `eval_binary` once
/// the int-int fast path misses.  Floats are immutable, so this is identical
/// for augmented (`s += x`) and const-folded plain ops.
///
/// Restricted to the coercion-safe arithmetic ops (same set as `num_mixed_fast`):
/// `Pow` (complex for a negative base) and comparisons fall through to
/// `eval_binary` unchanged.  Returns `None` for any non-float/non-mixed operands
/// so list/set/str in-place handling downstream is untouched.
#[inline(always)]
fn num_binop_fast(a: &Value, b: &Value, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add
        | BinaryOp::Sub
        | BinaryOp::Mul
        | BinaryOp::Div
        | BinaryOp::FloorDiv
        | BinaryOp::Mod => {}
        _ => return None,
    }
    if a.is_float() && b.is_float() {
        return float_float_fast(a.as_float_raw(), b.as_float_raw(), op);
    }
    if (a.is_float() && b.as_int().is_some()) || (a.as_int().is_some() && b.is_float()) {
        return num_mixed_fast(a, b, op);
    }
    None
}

#[inline(always)]
fn int_cmp(a: i64, b: i64, op: BinaryOp) -> Option<bool> {
    match op {
        BinaryOp::Eq => Some(a == b),
        BinaryOp::Ne => Some(a != b),
        BinaryOp::Lt => Some(a < b),
        BinaryOp::Le => Some(a <= b),
        BinaryOp::Gt => Some(a > b),
        BinaryOp::Ge => Some(a >= b),
        _ => None,
    }
}

#[inline(always)]
fn str_cmp(a: &str, b: &str, op: BinaryOp) -> Option<bool> {
    match op {
        BinaryOp::Eq => Some(a == b),
        BinaryOp::Ne => Some(a != b),
        BinaryOp::Lt => Some(a < b),
        BinaryOp::Le => Some(a <= b),
        BinaryOp::Gt => Some(a > b),
        BinaryOp::Ge => Some(a >= b),
        _ => None,
    }
}
