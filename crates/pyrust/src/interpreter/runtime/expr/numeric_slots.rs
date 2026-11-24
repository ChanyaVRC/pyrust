pub(super) use pyrust_core::bigint_to_float_or_overflow;
pub(crate) use pyrust_core::{bigint_divmod_floor, value_from_bigint, value_to_bigint};

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
    // O(log n) doubling.
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
/// user-defined `__ne__` or to fall back to negating `__eq__`.
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

/// Build the CPython operand-type `TypeError` for a binary numeric op when
/// at least one operand is non-numeric:
/// `unsupported operand type(s) for OP: 'lt' and 'rt'`.  `op_sym` is the
/// operator token CPython uses in the message (e.g. `/`, `//`, `%`,
/// `** or pow()`).
fn unsupported_operand(op_sym: &str, left: &Value, right: &Value) -> PyError {
    let lt = crate::interpreter::error_type_name_str(left);
    let rt = crate::interpreter::error_type_name_str(right);
    pyrust_core::type_err!("unsupported operand type(s) for {op_sym}: '{lt}' and '{rt}'")
}

/// The plain and augmented operator tokens an operand-type `TypeError` should
/// carry for `op`, or `None` for operators that never appear in an augmented
/// assignment (comparisons, `in`, `is`, …).
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
fn rewrite_aug_operand_error(op: BinaryOp, err: PyError) -> PyError {
    let Some((base, aug)) = aug_operand_symbols(op) else {
        return err;
    };
    let pyrust_core::PyErrorKind::Named(class, msg) = err.kind() else {
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

/// Map an AST [`BinaryOp`] to a `pyrust_core::NumericBinOp`, or `None` for
/// non-numeric operators (comparisons, membership, identity, matmul, logical).
#[inline]
pub(crate) fn binary_op_to_numeric(op: BinaryOp) -> Option<pyrust_core::NumericBinOp> {
    use pyrust_core::NumericBinOp;
    Some(match op {
        BinaryOp::Add => NumericBinOp::Add,
        BinaryOp::Sub => NumericBinOp::Sub,
        BinaryOp::Mul => NumericBinOp::Mul,
        BinaryOp::Div => NumericBinOp::Div,
        BinaryOp::FloorDiv => NumericBinOp::FloorDiv,
        BinaryOp::Mod => NumericBinOp::Mod,
        BinaryOp::Pow => NumericBinOp::Pow,
        BinaryOp::LShift => NumericBinOp::LShift,
        BinaryOp::RShift => NumericBinOp::RShift,
        BinaryOp::BitAnd => NumericBinOp::BitAnd,
        BinaryOp::BitOr => NumericBinOp::BitOr,
        BinaryOp::BitXor => NumericBinOp::BitXor,
        _ => return None,
    })
}

/// Map an AST comparison [`BinaryOp`] to a `pyrust_core::NumericCmpOp`.
#[inline]
pub(crate) fn binary_op_to_numeric_cmp(op: BinaryOp) -> Option<pyrust_core::NumericCmpOp> {
    use pyrust_core::NumericCmpOp;
    Some(match op {
        BinaryOp::Eq => NumericCmpOp::Eq,
        BinaryOp::Ne => NumericCmpOp::Ne,
        BinaryOp::Lt => NumericCmpOp::Lt,
        BinaryOp::Le => NumericCmpOp::Le,
        BinaryOp::Gt => NumericCmpOp::Gt,
        BinaryOp::Ge => NumericCmpOp::Ge,
        _ => return None,
    })
}

/// Unwrap scalar numeric subclass backings (`MyInt`, `MyFloat`, `MyComplex`)
/// without collapsing `Bool` to `Int`, so `pyrust_core::NumericSlot` preserves
/// `bool` subtype semantics (`True & False -> False`, `'bool'` in diagnostics).
#[inline]
fn unwrap_numeric_subclass(v: &Value) -> Value {
    if let Some(backing) = builtin_data_backing(v)
        && matches!(
            backing.kind(),
            ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _)
        )
    {
        return backing;
    }
    v.clone()
}

/// Dispatch a binary numeric operation through `pyrust_core::NumericOps`
/// (issue #458).
#[inline]
pub(crate) fn dispatch_numeric_binop(
    op: BinaryOp,
    lhs: &Value,
    rhs: &Value,
) -> Option<Result<Value>> {
    pyrust_core::dispatch_numeric_binop(binary_op_to_numeric(op)?, lhs, rhs)
}

/// Like [`dispatch_numeric_binop`], but when a subclass (`PyInstance`) operand
/// causes an unsupported-pair `TypeError` (e.g. `MyComplex(1, 2) // 2` or
/// `MyFloat(1.5) & 1`), preserves the original subclass type names in the
/// diagnostic to match CPython 3.12.
#[inline]
fn dispatch_numeric_binop_with_originals(
    op: BinaryOp,
    orig_lhs: &Value,
    orig_rhs: &Value,
    coerced_lhs: &Value,
    coerced_rhs: &Value,
) -> Option<Result<Value>> {
    let num_op = binary_op_to_numeric(op)?;
    let res = pyrust_core::dispatch_numeric_binop(num_op, coerced_lhs, coerced_rhs)?;
    Some(res.map_err(|err| {
        if matches!(err.kind(), pyrust_core::PyErrorKind::Named(cls, _) if cls.as_ref() == "TypeError")
            && (matches!(orig_lhs.kind(), ValueKind::PyInstance(_))
                || matches!(orig_rhs.kind(), ValueKind::PyInstance(_))
                || orig_lhs.is_bool()
                || orig_rhs.is_bool())
        {
            unsupported_operand(num_op.symbol(), orig_lhs, orig_rhs)
        } else {
            err
        }
    }))
}
