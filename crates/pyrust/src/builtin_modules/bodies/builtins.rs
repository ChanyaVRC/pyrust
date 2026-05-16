// `builtins` module — included into `pub mod builtins { … }` declared by
// the `@flat builtins,` entry in `pyrust_builtin_modules!` in
// `builtin_modules/mod.rs`.
//
// `@flat` means functions register under their short name only (no
// `builtins.` prefix), so `abs` resolves to `BuiltinReg { name: "abs", … }`.
// Therefore `BuiltinFunction("abs")` from the global env (set up in
// `helpers.rs::register_builtins`) hits this dispatch via the registry
// probe in `calls.rs::call_function_expanded`.  Importable as
// `import builtins` too, which yields a `PyModule { name: "builtins", … }`
// containing every fn declared here plus declared constants.
//
// Reference: <https://docs.python.org/3/library/functions.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyBool, PyBytes, PyFloat, PyInt, PyStr, PyValue};
use crate::interpreter::{
    NativeIterFrame, apply_format_spec, ascii_repr, class_is_subclass_of, compare_values,
    dir_names, instance_attrs_snapshot, int_pow_promoting, invoke_class_method,
    is_exception_class, iter_values, lookup_class_attr, modpow_i64, py_mod_i64,
    py_round_half_even, py_round_half_even_f64, reject_keyword_args_expanded,
    snapshot_current_locals, snapshot_module_namespace, value_to_float, value_type_name_str,
};
use crate::value::{PyClass, PyKey, Value, ValueKind, range_len};
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: chr(i) — return the string of one Unicode codepoint i.
    /// <https://docs.python.org/3/library/functions.html#chr>
    ///
    /// Migrated to the typed-signature dialect (#400) as a three-element
    /// overload set: `PyInt` is the primary path; `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping (`chr(True) == '\x01'`), since
    /// strict `PyInt` doesn't auto-coerce `bool` in the typed dialect.
    /// A trailing `PyValue` catch-all preserves the legacy
    /// `"an integer is required (got type X)"` TypeError verbatim — the
    /// macro's default "unsupported argument type(s)" fallback would
    /// drift from that canonical wording.  All parameters are
    /// `#[positional_only]` so the macro's positional-only fast-path
    /// applies (no kwarg-validation work).  Bignum inputs raise
    /// `OverflowError` via `PyInt::expect_i64` *before* the range
    /// check — a deliberate CPython-parity improvement over the
    /// legacy body, which raised `ValueError("chr() arg not in
    /// range(0x110000)")` via the range check.  Modern CPython
    /// raises `OverflowError` here too (compare `hex(bignum)`
    /// behaviour above).
    #[pure]
    fn chr(#[positional_only] i: PyInt) -> Result<Value> {
        let code_point = i.expect_i64(FN_NAME, "i")?;
        chr_from_code_point(code_point)
    }

    #[pure]
    fn chr(#[positional_only] i: PyBool) -> Result<Value> {
        // CPython: `chr(True) == '\x01'`, `chr(False) == '\x00'`.
        chr_from_code_point(if i.0 { 1 } else { 0 })
    }

    #[pure]
    fn chr(#[positional_only] i: PyValue) -> Result<Value> {
        Err(PyError::named(
            "TypeError",
            format!(
                "an integer is required (got type {})",
                value_type_name_str(&i.0),
            ),
        ))
    }

    /// CPython: ord(c) — return the Unicode codepoint of a one-character string.
    /// <https://docs.python.org/3/library/functions.html#ord>
    ///
    /// Migrated to the typed-signature dialect (#400) as a three-element
    /// overload set: `PyStr` is the primary path; `PyBytes` mirrors
    /// CPython's acceptance of one-byte bytes (`ord(b"A") == 65`); a
    /// trailing `PyValue` catch-all preserves the legacy
    /// `"expected string of length 1, but got non-string"` TypeError
    /// verbatim.  Length-mismatch wording on the `PyStr` overload is also
    /// preserved verbatim from the legacy body so parity output is
    /// stable.  All parameters are `#[positional_only]` so the macro's
    /// positional-only fast-path applies.  The `PyBytes` overload is a
    /// new CPython-parity feature — the legacy body rejected `bytes`
    /// outright, but CPython has always accepted a 1-byte `bytes`
    /// (`ord(b"A") == 65`).
    #[pure]
    fn ord(#[positional_only] c: PyStr) -> Result<Value> {
        let s: &str = &c;
        let mut chars = s.chars();
        let first = chars.next();
        let second = chars.next();
        match (first, second) {
            (Some(ch), None) => Ok(Value::int(ch as i64)),
            (None, _) => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expected a character, but string of length 0 found"),
            )),
            (Some(_), Some(_)) => Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() expected a character, but string of length {} found",
                    s.chars().count()
                ),
            )),
        }
    }

    #[pure]
    fn ord(#[positional_only] c: PyBytes) -> Result<Value> {
        // CPython: `ord(b"A") == 65`; reject empty/multi-byte with the
        // same wording shape used by the `PyStr` overload above.
        match c.0.as_slice() {
            [b] => Ok(Value::int(*b as i64)),
            other => Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() expected a character, but string of length {} found",
                    other.len()
                ),
            )),
        }
    }

    #[pure]
    fn ord(#[positional_only] c: PyValue) -> Result<Value> {
        let _ = c;
        Err(PyError::named(
            "TypeError",
            format!("{FN_NAME}() expected string of length 1, but got non-string"),
        ))
    }

    /// CPython: bin(x) — integer to '0b…' / '-0b…' string.
    /// <https://docs.python.org/3/library/functions.html#bin>
    ///
    /// Migrated to the typed-signature dialect (#400) mirroring `hex`'s
    /// 3-overload pattern: `PyInt` is the primary path, `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping, and a trailing `PyValue`
    /// catch-all reproduces CPython's exact "'X' object cannot be
    /// interpreted as an integer" TypeError wording verbatim.
    /// Bignums not yet supported; raises `OverflowError` if `x` doesn't
    /// fit in i64 (deliberate divergence from CPython, tracked as
    /// follow-up under #400).
    #[pure]
    fn bin(#[positional_only] x: PyInt) -> Result<Value> {
        let v = x.expect_i64(FN_NAME, "x")?;
        Ok(Value::string(format_bin_i64(v)))
    }

    #[pure]
    fn bin(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `bin(True) == '0b1'`, `bin(False) == '0b0'`.
        Ok(Value::string(format_bin_i64(if x.0 { 1 } else { 0 })))
    }

    #[pure]
    fn bin(#[positional_only] x: PyValue) -> Result<Value> {
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(&x.0),
            ),
        ))
    }

    /// CPython: oct(x) — integer to '0o…' / '-0o…' string.
    /// <https://docs.python.org/3/library/functions.html#oct>
    ///
    /// Migrated to the typed-signature dialect (#400) mirroring `hex`'s
    /// 3-overload pattern: `PyInt` is the primary path, `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping, and a trailing `PyValue`
    /// catch-all reproduces CPython's exact "'X' object cannot be
    /// interpreted as an integer" TypeError wording verbatim.
    /// Bignums not yet supported; raises `OverflowError` if `x` doesn't
    /// fit in i64 (deliberate divergence from CPython, tracked as
    /// follow-up under #400).
    #[pure]
    fn oct(#[positional_only] x: PyInt) -> Result<Value> {
        let v = x.expect_i64(FN_NAME, "x")?;
        Ok(Value::string(format_oct_i64(v)))
    }

    #[pure]
    fn oct(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `oct(True) == '0o1'`, `oct(False) == '0o0'`.
        Ok(Value::string(format_oct_i64(if x.0 { 1 } else { 0 })))
    }

    #[pure]
    fn oct(#[positional_only] x: PyValue) -> Result<Value> {
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(&x.0),
            ),
        ))
    }

    /// CPython: hex(x) — integer to '0x…' / '-0x…' string.
    /// <https://docs.python.org/3/library/functions.html#hex>
    ///
    /// Migrated to the typed-signature dialect (#400) as a two-element
    /// overload set: `PyInt` is the primary path; `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping (without it, strict `PyInt`
    /// would reject `hex(True)` — `bool` doesn't auto-coerce in the
    /// typed dialect, see [`builtin_args`]).  A trailing `PyValue`
    /// catch-all reproduces CPython's exact "'X' object cannot be
    /// interpreted as an integer" TypeError wording (the macro's own
    /// "unsupported argument type(s)" fallback would drift from the
    /// canonical message — preserved verbatim from the legacy body).
    /// Genuine bignums raise `OverflowError` via `expect_i64`; that's
    /// a deliberate divergence from CPython, tracked as follow-up.
    #[pure]
    fn hex(#[positional_only] x: PyInt) -> Result<Value> {
        let v = x.expect_i64(FN_NAME, "x")?;
        Ok(Value::string(format_hex_i64(v)))
    }

    #[pure]
    fn hex(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `hex(True) == '0x1'`, `hex(False) == '0x0'`.
        Ok(Value::string(format_hex_i64(if x.0 { 1 } else { 0 })))
    }

    #[pure]
    fn hex(#[positional_only] x: PyValue) -> Result<Value> {
        Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(&x.0),
            ),
        ))
    }

    /// CPython: ascii(object) — ASCII-only escaped repr.
    /// <https://docs.python.org/3/library/functions.html#ascii>
    ///
    /// Migrated to the typed-signature dialect (#400): like `repr`,
    /// `ascii` accepts every Python object, so `PyValue` is the natural
    /// wrapper.  The body just delegates to the existing helper.
    #[pure]
    fn ascii(#[positional_only] obj: PyValue) -> Result<Value> {
        Ok(Value::string(ascii_repr(&obj.0)))
    }

    /// CPython: id(object) — identity (CPython returns memory address).
    /// <https://docs.python.org/3/library/functions.html#id>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` is the
    /// catch-all wrapper since `id` accepts every Python object; the
    /// existing per-kind dispatch becomes the body's only concern.
    #[pure]
    fn id(#[positional_only] obj: PyValue) -> Result<Value> {
        let value = &obj.0;
        let id_val: i64 = match value.kind() {
            ValueKind::PyInstance(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::PyClass(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::PyModule(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::UserFunction(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::Int(n) => n,
            ValueKind::Bool(b) => b as i64,
            ValueKind::None => 0,
            _ => value.value_id().unwrap_or(0),
        };
        Ok(Value::int(id_val))
    }

    /// CPython: abs(x) — absolute value.
    /// <https://docs.python.org/3/library/functions.html#abs>
    ///
    /// First builtin migrated to the overload-dispatch dialect (#395):
    /// declare one `fn abs` per concrete arg-type combination, with
    /// `PyValue` as the trailing catch-all for `complex`, `PyInstance`
    /// with `__abs__`, and the not-a-number error path.  The macro
    /// generates a dispatcher that tries each overload in declaration
    /// order.
    #[pure]
    fn abs(#[positional_only] x: PyInt) -> Result<Value> {
        // i64 fast path; fall back to BigInt for both genuine bignums
        // *and* the `i64::MIN` boundary case — `i64::MIN.checked_abs()`
        // returns `None` because `-i64::MIN` doesn't fit in i64 (one
        // more positive value than negative in two's complement).
        // CPython returns `9223372036854775808` for `abs(-i64::MIN)`;
        // matching that requires the BigInt path even though `as_i64`
        // succeeded.  We can't call `BigInt::abs` here without pulling
        // `num_traits` in as a direct dep, so do the negate-if-negative
        // dance manually.
        if let Some(n) = x.as_i64()
            && let Some(abs) = n.checked_abs()
        {
            return Ok(Value::int(abs));
        }
        let big = x.to_bigint();
        let zero: crate::value::PyBigInt = 0i64.into();
        let abs = if big < zero { -big } else { big };
        Ok(Value::bigint(abs))
    }

    #[pure]
    fn abs(#[positional_only] x: PyFloat) -> Result<Value> {
        Ok(Value::float(x.0.abs()))
    }

    #[pure]
    fn abs(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: abs(True) == 1, abs(False) == 0 — promoted to int.
        Ok(Value::int(if x.0 { 1 } else { 0 }))
    }

    #[pure]
    fn abs(#[positional_only] x: PyValue) -> Result<Value> {
        // Catch-all: complex magnitude, user-defined `__abs__`, and the
        // "not a number" error otherwise.  Reached when none of the
        // typed overloads above matched the call's argument.
        let val = x.0;
        if let ValueKind::PyInstance(inst) = val.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__abs__") {
                return invoke_class_method(
                    _interp,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[],
                );
            }
            return Err(PyError::named(
                "TypeError",
                format!("bad operand type for abs(): '{}'", class.borrow().name),
            ));
        }
        if let ValueKind::Complex(re, im) = val.kind() {
            return Ok(Value::float((re * re + im * im).sqrt()));
        }
        Err(PyError::named(
            "TypeError",
            format!("bad operand type for abs(): '{}'", value_type_name_str(&val)),
        ))
    }

    /// CPython: sum(iterable, /, start=0) — sum elements of an iterable.
    /// <https://docs.python.org/3/library/functions.html#sum>
    #[pure]
    fn sum(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 1 or 2 arguments")));
        }
        let items = _interp.collect_iterable(args[0].value.clone())?;
        let start = if args.len() == 2 { args[1].value.clone() } else { Value::int(0) };
        let mut acc = start;
        for item in items {
            acc = _interp.eval_binary(acc, BinaryOp::Add, item)?;
        }
        Ok(acc)
    }

    /// CPython: any(iterable) — true if any element is truthy.
    /// <https://docs.python.org/3/library/functions.html#any>
    #[pure]
    fn any(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let items = _interp.collect_iterable(args[0].value.clone())?;
        for item in items {
            if _interp.truthy_value(&item)? {
                return Ok(Value::bool_(true));
            }
        }
        Ok(Value::bool_(false))
    }

    /// CPython: all(iterable) — true if every element is truthy (or empty).
    /// <https://docs.python.org/3/library/functions.html#all>
    #[pure]
    fn all(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let items = _interp.collect_iterable(args[0].value.clone())?;
        for item in items {
            if !_interp.truthy_value(&item)? {
                return Ok(Value::bool_(false));
            }
        }
        Ok(Value::bool_(true))
    }

    /// CPython: repr(object) — printable representation string.
    /// <https://docs.python.org/3/library/functions.html#repr>
    ///
    /// Migrated to the typed-signature dialect (#400): the macro-emitted
    /// prelude validates positional count, rejects unknown kwargs, and
    /// binds `obj` as a typed local.  `PyValue` is the catch-all wrapper
    /// — `repr` accepts every Python object, so type-checking the input
    /// is exactly the prelude's "validate arity / reject kwargs" job.
    #[pure]
    fn repr(#[positional_only] obj: PyValue) -> Result<Value> {
        let obj = obj.0;
        if let ValueKind::PyInstance(instance) = obj.kind() {
            let instance_rc = Rc::clone(instance);
            let class = Rc::clone(&instance_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__repr__") {
                let result = invoke_class_method(
                    _interp,
                    method_val,
                    Value::py_instance(instance_rc),
                    &[],
                )?;
                let is_str = matches!(result.kind(), ValueKind::Str(_));
                return if is_str {
                    Ok(result)
                } else {
                    Err(PyError::named(
                        "TypeError",
                        "__repr__ returned non-string".to_string(),
                    ))
                };
            }
        }
        Ok(Value::string(obj.repr()))
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
    /// Not marked `#[pure]` because it dispatches user `__hash__` for
    /// `PyInstance` values, which may invoke arbitrary user code.
    fn hash(#[positional_only] obj: PyValue) -> Result<Value> {
        let value = obj.0;
        // Dispatch user-defined __hash__ for PyInstance before falling
        // through to the primitive hash_value path.  Mirrors CPython's
        // tp_hash slot lookup.
        if let ValueKind::PyInstance(inst) = value.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            let class_name = class.borrow().name.clone();
            if let Some(hash_method) = lookup_class_attr(&class, "__hash__") {
                // __hash__ = None means explicitly unhashable (CPython rule).
                if matches!(hash_method.kind(), ValueKind::None) {
                    return Err(PyError::named(
                        "TypeError",
                        format!("unhashable type: '{class_name}'"),
                    ));
                }
                let result = invoke_class_method(
                    _interp,
                    hash_method,
                    Value::py_instance(inst_rc),
                    &[],
                )?;
                // __hash__ must return an integer (bool is a subtype of int).
                // CPython maps -1 → -2 because -1 is the C-level error sentinel.
                // Large integers (BigInt) are reduced mod 2^61-1 (Py_HASH_MODULUS),
                // matching CPython's tp_hash for arbitrary-precision int results.
                const PY_HASH_MODULUS: i64 = (1i64 << 61) - 1;
                let raw: i64 = match result.kind() {
                    ValueKind::Int(n) => n,
                    ValueKind::Bool(b) => b as i64,
                    ValueKind::BigInt(n) => {
                        // Reduce mod Py_HASH_MODULUS (2^61 - 1), preserving sign.
                        // n: &BigInt (the borrowed inner value from the Rc).
                        use crate::value::PyBigInt;
                        use crate::value::PyToPrimitive;
                        let modulus = PyBigInt::from(PY_HASH_MODULUS);
                        let reduced = n.clone() % &modulus;
                        // `reduced` is in (-modulus, modulus); fits in i64.
                        reduced.to_i64().unwrap_or(0)
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "__hash__ method should return an integer".to_string(),
                        ));
                    }
                };
                let hash_val = if raw == -1 { -2 } else { raw };
                return Ok(Value::int(hash_val));
            }
            // No __hash__ at all: use object-identity hash, matching
            // CPython's default object.__hash__ behaviour.
            let ptr = Rc::as_ptr(&inst_rc) as i64;
            let ptr = if ptr == -1 { -2 } else { ptr };
            return Ok(Value::int(ptr));
        }
        let hash_val = hash_value(&value)?;
        Ok(Value::int(hash_val))
    }

    /// CPython: divmod(a, b) — `(a // b, a % b)`.
    /// <https://docs.python.org/3/library/functions.html#divmod>
    #[pure]
    fn divmod(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        match (args[0].value.kind(), args[1].value.kind()) {
            (ValueKind::Int(a), ValueKind::Int(b)) => {
                if b == 0 {
                    return Err(PyError::named(
                        "ZeroDivisionError",
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                let modulo = py_mod_i64(a, b);
                let quotient = (a - modulo) / b;
                Ok(Value::tuple(vec![Value::int(quotient), Value::int(modulo)]))
            }
            (ValueKind::Bool(a), ValueKind::Bool(b)) => {
                let a = a as i64;
                let b = b as i64;
                if b == 0 {
                    return Err(PyError::named(
                        "ZeroDivisionError",
                        "integer division or modulo by zero".to_string(),
                    ));
                }
                let modulo = py_mod_i64(a, b);
                let quotient = (a - modulo) / b;
                Ok(Value::tuple(vec![Value::int(quotient), Value::int(modulo)]))
            }
            _ => {
                let a = value_to_float(&args[0].value, FN_NAME)?;
                let b = value_to_float(&args[1].value, FN_NAME)?;
                if b == 0.0 {
                    return Err(PyError::named(
                        "ZeroDivisionError",
                        "float divmod()".to_string(),
                    ));
                }
                let quotient = (a / b).floor();
                let modulo = a - b * quotient;
                Ok(Value::tuple(vec![Value::float(quotient), Value::float(modulo)]))
            }
        }
    }

    /// CPython: pow(base, exp[, mod]) — exponentiation, optionally modular.
    /// <https://docs.python.org/3/library/functions.html#pow>
    #[pure]
    fn pow(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 || args.len() > 3 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 2 or 3 arguments")));
        }
        if args.len() == 3 {
            let base = match args[0].value.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    "pow() 3-argument form requires integers".to_string(),
                )),
            };
            let exp = match args[1].value.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    "pow() 3-argument form requires integers".to_string(),
                )),
            };
            let modulus = match args[2].value.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    "pow() 3-argument form requires integers".to_string(),
                )),
            };
            if modulus == 0 {
                return Err(PyError::named(
                    "ValueError",
                    "pow() 3rd argument cannot be 0".to_string(),
                ));
            }
            if exp < 0 {
                return Err(PyError::named(
                    "ValueError",
                    "pow() 2nd argument cannot be negative when 3rd argument specified".to_string(),
                ));
            }
            let result = modpow_i64(base, exp as u64, modulus);
            Ok(Value::int(result))
        } else {
            match (args[0].value.kind(), args[1].value.kind()) {
                (ValueKind::Int(a), ValueKind::Int(b)) if b >= 0 => {
                    Ok(int_pow_promoting(a, b))
                }
                (ValueKind::Bool(a), ValueKind::Int(b)) if b >= 0 => {
                    Ok(int_pow_promoting(a as i64, b))
                }
                _ => {
                    let a = value_to_float(&args[0].value, FN_NAME)?;
                    let b = value_to_float(&args[1].value, FN_NAME)?;
                    Ok(Value::float(a.powf(b)))
                }
            }
        }
    }

    /// CPython: enumerate(iterable, start=0) — enumerate iterator.
    /// <https://docs.python.org/3/library/functions.html#enumerate>
    #[pure]
    fn enumerate(args) -> Result<Value> {
        // Parse `iterable` and `start` (positional or keyword). CPython's
        // signature is `enumerate(iterable, start=0)`.
        let mut iterable: Option<Value> = None;
        let mut start_val: Option<Value> = None;
        for (i, arg) in args.iter().enumerate() {
            match arg.name.as_deref() {
                None => match i {
                    0 => iterable = Some(arg.value.clone()),
                    1 => start_val = Some(arg.value.clone()),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() takes at most 2 arguments ({} given)", args.len()),
                    )),
                },
                Some("iterable") => {
                    if iterable.is_some() {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{FN_NAME}() got multiple values for argument 'iterable'"),
                        ));
                    }
                    iterable = Some(arg.value.clone());
                }
                Some("start") => {
                    if start_val.is_some() {
                        return Err(PyError::named(
                            "TypeError",
                            format!("{FN_NAME}() got multiple values for argument 'start'"),
                        ));
                    }
                    start_val = Some(arg.value.clone());
                }
                Some(k) => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() got an unexpected keyword argument '{k}'"),
                )),
            }
        }
        let iterable = iterable.ok_or_else(|| PyError::named(
            "TypeError",
            format!("{FN_NAME}() missing required argument: 'iterable'"),
        ))?;
        let start = match start_val {
            None => 0i64,
            Some(v) => match v.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        value_type_name_str(&v),
                    ),
                )),
            },
        };
        // Pre-materialise PyInstance / Generator sources so user
        // `__iter__` dispatch (which requires the Interpreter) happens
        // here — the lazy helper in `iter_helpers` reaches the
        // registry callback, which can't dispatch dunders or resume
        // generators.  For other sources we still pass the raw value
        // so side effects (e.g. `open()`) defer to iteration start
        // (#446).
        let iterable = materialize_user_iter(_interp, iterable)?;
        Ok(pyrust_builtins::iter_helpers::enumerate(
            iterable,
            start,
        ))
    }

    /// CPython: zip(*iterables, strict=False) — parallel iterator.
    /// `strict=True` raises `ValueError` if lengths differ.
    /// <https://docs.python.org/3/library/functions.html#zip>
    #[pure]
    fn zip(args) -> Result<Value> {
        // `strict` is the only accepted keyword arg; everything else is a
        // CPython-style `TypeError`.
        let mut strict = false;
        for a in args.iter() {
            if let Some(name) = a.name.as_deref() {
                if name == "strict" {
                    strict = _interp.truthy_value(&a.value)?;
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() got an unexpected keyword argument '{name}'"),
                    ));
                }
            }
        }
        let sources: Vec<Value> = args
            .iter()
            .filter(|a| a.name.is_none())
            .map(|a| a.value.clone())
            .collect();
        // Pre-materialise PyInstance sources (see `enumerate` for rationale).
        let sources = sources
            .into_iter()
            .map(|v| materialize_user_iter(_interp, v))
            .collect::<Result<Vec<_>>>()?;
        Ok(pyrust_builtins::iter_helpers::zip(sources, strict))
    }

    /// CPython: reversed(seq) — reverse iterator.
    /// <https://docs.python.org/3/library/functions.html#reversed>
    #[pure]
    fn reversed(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        // Pre-materialise PyInstance sources (see `enumerate` for rationale).
        let source = materialize_user_iter(_interp, args[0].value.clone())?;
        Ok(pyrust_builtins::iter_helpers::reversed(source))
    }

    /// CPython: map(func, iterable) — apply func to each element.
    /// <https://docs.python.org/3/library/functions.html#map>
    fn map(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let func = args[0].value.clone();
        let items = _interp.collect_iterable(args[1].value.clone())?;
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            let mapped = _interp.call_function_expanded(
                func.clone(),
                &[ExpandedCallArg { name: None, value: item }],
            )?;
            result.push(mapped);
        }
        Ok(Value::list(result))
    }

    /// CPython: filter(func, iterable) — keep elements where func is truthy.
    /// <https://docs.python.org/3/library/functions.html#filter>
    fn filter(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let func = args[0].value.clone();
        let items = _interp.collect_iterable(args[1].value.clone())?;
        let use_identity = func.is_none();
        let mut result = Vec::new();
        for item in items {
            let keep = if use_identity {
                _interp.truthy_value(&item)?
            } else {
                let test = _interp.call_function_expanded(
                    func.clone(),
                    &[ExpandedCallArg { name: None, value: item.clone() }],
                )?;
                _interp.truthy_value(&test)?
            };
            if keep {
                result.push(item);
            }
        }
        Ok(Value::list(result))
    }

    /// CPython: iter(obj) — return an iterator over obj.
    /// <https://docs.python.org/3/library/functions.html#iter>
    #[pure]
    fn iter(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let val = args[0].value.clone();
        // Detect kind tag in a scoped block so the kind() borrow drops
        // before we may need to move `val` (#450).
        enum IterKind {
            Generator,
            PyInstance(Rc<RefCell<crate::value::PyInstance>>),
            Other,
        }
        let kind = match val.kind() {
            ValueKind::Generator(_) => IterKind::Generator,
            ValueKind::PyInstance(inst) => IterKind::PyInstance(Rc::clone(inst)),
            _ => IterKind::Other,
        };
        match kind {
            IterKind::Generator => Ok(val),
            IterKind::PyInstance(inst_rc) => {
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__iter__") {
                    invoke_class_method(
                        _interp,
                        method_val,
                        Value::py_instance(inst_rc),
                        &[],
                    )
                } else if lookup_class_attr(&class, "__getitem__").is_some() {
                    _interp.make_getitem_iter(inst_rc)
                } else {
                    Err(PyError::named(
                        "TypeError",
                        format!("'{}' object is not iterable", class.borrow().name),
                    ))
                }
            }
            IterKind::Other => {
                let items = iter_values(val.clone()).map_err(|_| {
                    PyError::named(
                        "TypeError",
                        format!("'{}' object is not iterable", value_type_name_str(&val)),
                    )
                })?;
                Ok(Value::generator(Box::new(NativeIterFrame { items, pos: 0 })))
            }
        }
    }

    /// CPython: next(iterator[, default]) — fetch the next element.
    /// <https://docs.python.org/3/library/functions.html#next>
    fn next(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 1 or 2 arguments")));
        }
        let gen_val = args[0].value.clone();
        let default_val = if args.len() == 2 {
            Some(args[1].value.clone())
        } else {
            None
        };
        _interp.call_next(gen_val, default_val)
    }

    /// CPython: issubclass(cls, classinfo) — true if `cls` is a subclass.
    /// <https://docs.python.org/3/library/functions.html#issubclass>
    #[pure]
    fn issubclass(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        // `cls` may be either a user-defined class (`PyClass`) or a
        // built-in type token (`BuiltinFunction("int")` etc.); anything
        // else is a `TypeError`, matching CPython.
        if !is_class_like(&args[0].value) {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() arg 1 must be a class"),
            ));
        }
        let result = issubclass_check(FN_NAME, &args[0].value, &args[1].value)?;
        Ok(Value::bool_(result))
    }

    /// CPython: delattr(obj, name) — delete an attribute.
    /// <https://docs.python.org/3/library/functions.html#delattr>
    fn delattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}(): attribute name must be a string"),
            )),
        };
        match args[0].value.kind() {
            ValueKind::PyInstance(instance) => {
                let instance = Rc::clone(instance);
                // `shift_remove` preserves the insertion order of the
                // surviving entries — matches CPython's `del obj.x`
                // semantics on a dict that's known to be insertion-ordered.
                if instance.borrow_mut().attrs.shift_remove(&name).is_none() {
                    let class_name = instance.borrow().class.borrow().name.clone();
                    return Err(PyError::named(
                        "AttributeError",
                        format!("'{class_name}' object has no attribute '{name}'"),
                    ));
                }
                Ok(Value::none())
            }
            ValueKind::PyClass(class) => {
                let class = Rc::clone(class);
                if class.borrow_mut().attrs.shift_remove(&name).is_none() {
                    let class_name = class.borrow().name.clone();
                    return Err(PyError::named(
                        "AttributeError",
                        format!("type object '{class_name}' has no attribute '{name}'"),
                    ));
                }
                Ok(Value::none())
            }
            _ => Err(PyError::named(
                "AttributeError",
                format!("{FN_NAME}() object has no writable attributes"),
            )),
        }
    }

    /// CPython: isinstance(obj, classinfo) — type check.
    /// <https://docs.python.org/3/library/functions.html#isinstance>
    #[pure]
    fn isinstance(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let result = isinstance_check(FN_NAME, &args[0].value, &args[1].value)?;
        Ok(Value::bool_(result))
    }

    /// CPython: type(object) → type / type(name, bases, namespace) → new class.
    /// <https://docs.python.org/3/library/functions.html#type>
    #[pure]
    fn r#type(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() == 3 {
            let name = match args[0].value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument 1 must be a str"),
                )),
            };
            // Extract the first element (if any) of the base-class
            // sequence in a scoped block so the kind() Ref guard drops
            // before we work with it (#450).
            let first_base: Option<Value> = match args[1].value.kind() {
                ValueKind::Tuple(items) => items.first().cloned(),
                ValueKind::List(items) => items.first().cloned(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument 2 must be a tuple"),
                )),
            };
            let base = match first_base {
                None => None,
                Some(first) => match first.kind() {
                    ValueKind::PyClass(c) => Some(Rc::clone(c)),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() argument 2 entries must be classes"),
                    )),
                },
            };
            let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
            match args[2].value.kind() {
                ValueKind::Dict(map) => {
                    for (k, v) in map.iter() {
                        if let PyKey::Str(key) = k {
                            attrs.insert(key.clone(), v.clone());
                        }
                    }
                }
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument 3 must be a dict"),
                )),
            }
            return Ok(Value::py_class(Rc::new(RefCell::new(PyClass {
                name,
                base,
                attrs,
            }))));
        }
        if args.len() != 1 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes exactly 1 argument (or 3 for type creation)",
            )));
        }
        let obj = &args[0].value;
        // For user-defined class instances return the actual Rc so that
        // `type(x) is type(x)` works via Rc::ptr_eq.
        //
        // Issue #462: the 11 migrated primitives (`int`, `str`, …) return
        // their per-thread `PyClass` singletons from `primitive_class_for_value`,
        // so `type(5).__name__ == "int"`, `bool.__bases__ == (int,)`, and
        // `isinstance(x, T)` work through the standard class machinery.
        //
        // Remaining variants (functions, modules, ranges, generators, …)
        // still emit a `BuiltinFunction(name)` sentinel — they're not part
        // of the primitive-class migration.
        if let ValueKind::PyInstance(inst) = obj.kind() {
            return Ok(Value::py_class(Rc::clone(&inst.borrow().class)));
        }
        if let Some(class) = crate::interpreter::primitive_class_for_value(obj) {
            return Ok(Value::py_class(class));
        }
        match obj.kind() {
            ValueKind::PyClass(_) => Ok(Value::builtin_function("type")),
            ValueKind::None => Ok(Value::builtin_function("NoneType")),
            ValueKind::Range { .. } => Ok(Value::builtin_function("range")),
            ValueKind::UserFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. } => Ok(Value::builtin_function("function")),
            ValueKind::BuiltinFunction(_) => Ok(Value::builtin_function("builtin_function_or_method")),
            ValueKind::PyModule(_) => Ok(Value::builtin_function("module")),
            ValueKind::SuperProxy { .. } | ValueKind::SuperProxyClass { .. } => Ok(Value::builtin_function("super")),
            ValueKind::Generator(_) => Ok(Value::builtin_function("generator")),
            ValueKind::NotImplemented => Ok(Value::builtin_function("NotImplementedType")),
            ValueKind::BuiltinObject { ops, .. } => {
                Ok(Value::builtin_function(ops.type_name()))
            }
            // Migrated primitives are handled above via
            // `primitive_class_for_value`; the explicit `unreachable!`
            // documents that and lets rustc verify exhaustiveness.
            ValueKind::Bool(_) | ValueKind::Int(_) | ValueKind::BigInt(_)
            | ValueKind::Float(_) | ValueKind::Str(_) | ValueKind::List(_)
            | ValueKind::Tuple(_) | ValueKind::Dict(_) | ValueKind::Set(_)
            | ValueKind::Bytes(_) | ValueKind::Complex(_, _)
            | ValueKind::PyInstance(_) => unreachable!(
                "primitive_class_for_value should have handled this variant"
            ),
        }
    }

    /// CPython: hasattr(obj, name) — true if `getattr(obj, name)` would succeed.
    /// <https://docs.python.org/3/library/functions.html#hasattr>
    fn hasattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}(): attribute name must be a string"),
            )),
        };
        let result = match _interp.get_attr(args[0].value.clone(), &name) {
            Ok(_) => true,
            Err(PyError::Named(ref cls, _)) if cls == "AttributeError" => false,
            Err(e) => return Err(e),
        };
        Ok(Value::bool_(result))
    }

    /// CPython: getattr(obj, name[, default]) — attribute access by name.
    /// <https://docs.python.org/3/library/functions.html#getattr>
    fn getattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 || args.len() > 3 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 2 or 3 arguments")));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}(): attribute name must be a string"),
            )),
        };
        match _interp.get_attr(args[0].value.clone(), &name) {
            Ok(v) => Ok(v),
            Err(PyError::Named(ref cls, _)) if cls == "AttributeError" && args.len() == 3 => {
                Ok(args[2].value.clone())
            }
            Err(e) => Err(e),
        }
    }

    /// CPython: setattr(obj, name, value) — attribute assignment by name.
    /// <https://docs.python.org/3/library/functions.html#setattr>
    fn setattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 3 arguments")));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}(): attribute name must be a string"),
            )),
        };
        _interp.assign_attr(args[0].value.clone(), &name, args[2].value.clone())?;
        Ok(Value::none())
    }

    /// CPython: vars([object]) — `__dict__` snapshot, or current env if no arg.
    /// <https://docs.python.org/3/library/functions.html#vars>
    fn vars(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 1 argument"),
            ));
        }
        if args.is_empty() {
            let mut dict: indexmap::IndexMap<PyKey, Value> = indexmap::IndexMap::new();
            for (k, v) in _interp.env.borrow().values.iter() {
                dict.insert(PyKey::Str(k.clone()), v.clone());
            }
            return Ok(Value::dict(dict));
        }
        match args[0].value.kind() {
            ValueKind::PyInstance(instance) => Ok(instance_attrs_snapshot(instance)),
            ValueKind::PyModule(module) => {
                let mut dict: indexmap::IndexMap<PyKey, Value> = indexmap::IndexMap::new();
                for (k, v) in module.borrow().attrs.iter() {
                    dict.insert(PyKey::Str(k.clone()), v.clone());
                }
                Ok(Value::dict(dict))
            }
            ValueKind::PyClass(class) => {
                let mut dict: indexmap::IndexMap<PyKey, Value> = indexmap::IndexMap::new();
                for (k, v) in class.borrow().attrs.iter() {
                    dict.insert(PyKey::Str(k.clone()), v.clone());
                }
                Ok(Value::dict(dict))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() argument must have __dict__ attribute (got '{}')",
                    value_type_name_str(&args[0].value),
                ),
            )),
        }
    }

    /// CPython: globals() — dict snapshot of the current module's namespace.
    /// <https://docs.python.org/3/library/functions.html#globals>
    ///
    /// Built via `snapshot_module_namespace`, which merges:
    ///   * the module-level `Environment::values` (built-in exceptions,
    ///     `NotImplemented`, plus any names that have been spilled out
    ///     of registers by the script-end flush in `program.rs`), and
    ///   * the active script frame's fastlocal registers (so a top-level
    ///     `x = 5; print(globals())` actually shows `x`, even though `x`
    ///     lives in a register until the script exits).
    ///
    /// When called from inside a function, this still returns the
    /// *module* globals — never the calling frame's locals.  CPython
    /// returns a live view of the module dict (mutations write through);
    /// pyrust returns a snapshot for v1 because the env values map is a
    /// `HashMap<String, Value>` plus a register slice, not a single
    /// `Value::dict` we can hand back by reference.  Issue #389 calls
    /// out the snapshot limitation as acceptable, and the common
    /// read-only iteration use case is fully supported.
    fn globals(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        Ok(Value::dict(snapshot_module_namespace(_interp)))
    }

    /// CPython: locals() — dict snapshot of the current local namespace.
    /// <https://docs.python.org/3/library/functions.html#locals>
    ///
    /// At module scope, `locals()` returns the same dict as `globals()`
    /// (CPython parity: at module level the two namespaces are the same
    /// object).  Inside a function body it returns a snapshot of the
    /// function's locals — CPython also snapshots and its docs warn that
    /// mutations to the returned dict aren't guaranteed to propagate.
    /// Both fastlocal-register and env-stored bindings are merged via
    /// `snapshot_current_locals`.
    fn locals(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        Ok(Value::dict(snapshot_current_locals(_interp)))
    }

    /// CPython: dir([object]) — list of attribute names.
    /// <https://docs.python.org/3/library/functions.html#dir>
    fn dir(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 1 argument"),
            ));
        }
        let mut names: Vec<String> = if args.is_empty() {
            _interp.env.borrow().values.keys().cloned().collect()
        } else {
            dir_names(&args[0].value)
        };
        names.sort();
        names.dedup();
        Ok(Value::list(names.into_iter().map(Value::string).collect()))
    }

    /// CPython: len(s) — number of items in a container.
    /// <https://docs.python.org/3/library/functions.html#len>
    #[pure]
    fn len(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let value = args[0].value.clone();
        let size = match value.kind() {
            ValueKind::Str(text) => text.chars().count() as i64,
            ValueKind::List(items) => items.len() as i64,
            ValueKind::Tuple(items) => items.len() as i64,
            ValueKind::Set(items) => items.len() as i64,
            ValueKind::Bytes(rc) => rc.len() as i64,
            ValueKind::Dict(items) => items.len() as i64,
            ValueKind::Range { start, stop, step } => range_len(start, stop, step),
            ValueKind::BuiltinObject { ops, state } => match ops.len(state) {
                Some(n) => n as i64,
                None => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("object of type '{}' has no len()", ops.type_name()),
                    ));
                }
            },
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__len__") {
                    let result = invoke_class_method(
                        _interp,
                        method_val,
                        Value::py_instance(inst_rc),
                        &[],
                    )?;
                    match result.kind() {
                        ValueKind::Int(n) if n >= 0 => n,
                        ValueKind::Int(_) => return Err(PyError::named(
                            "ValueError",
                            "__len__() should return >= 0".to_string(),
                        )),
                        ValueKind::Bool(b) => if b { 1 } else { 0 },
                        _ => return Err(PyError::named(
                            "TypeError",
                            "__len__ returned non-int".to_string(),
                        )),
                    }
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "object of type '{}' has no len()",
                            inst_rc.borrow().class.borrow().name,
                        ),
                    ));
                }
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "object of type '{}' has no len()",
                        pyrust_core::builtin_type_name(&value),
                    ),
                ));
            }
        };
        Ok(Value::int(size))
    }

    /// CPython: sorted(iterable, /, *, key=None, reverse=False) — new sorted list.
    /// <https://docs.python.org/3/library/functions.html#sorted>
    #[pure]
    fn sorted(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::Runtime(format!("{FN_NAME}() requires at least one argument")));
        }
        // `reverse=` is dispatched through `__bool__` (with `__len__`
        // fallback and default-truthy for instances without either) —
        // matches CPython 3.12+ which coerces via `bool()`.  An earlier
        // attempt routed through `__index__` based on Python 3.11
        // behaviour; 3.12 changed it (CPython commit history confirms),
        // so the truthy-dispatch path is the cross-version-safe choice
        // for the pyrust matrix.  See #432 review + CI parity failure
        // on `sorted-rev-justbool` / `-nothing` cases under 3.12.
        let reverse = match args.iter().find(|a| a.name.as_deref() == Some("reverse")) {
            Some(a) => _interp.truthy_value(&a.value)?,
            None => false,
        };
        let key_fn = args.iter().find(|a| a.name.as_deref() == Some("key"))
            .map(|a| a.value.clone());
        let positional: Vec<&ExpandedCallArg> = args.iter()
            .filter(|a| a.name.is_none())
            .collect();
        if positional.len() != 1 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes exactly one positional argument",
            )));
        }
        let mut items = _interp.collect_iterable(positional[0].value.clone())?;
        if let Some(kfn) = key_fn {
            let mut keyed: Vec<(Value, Value)> = items
                .into_iter()
                .map(|v| {
                    let k = _interp.call_function_expanded(
                        kfn.clone(),
                        &[ExpandedCallArg { name: None, value: v.clone() }],
                    )?;
                    Ok((k, v))
                })
                .collect::<Result<_>>()?;
            let mut sort_err: Option<PyError> = None;
            keyed.sort_by(|(a, _), (b, _)| {
                if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                match compare_values(a, b) {
                    Ok(ord) => ord,
                    Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                }
            });
            if let Some(e) = sort_err { return Err(e); }
            items = keyed.into_iter().map(|(_, v)| v).collect();
        } else {
            let mut sort_err: Option<PyError> = None;
            items.sort_by(|a, b| {
                if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                match compare_values(a, b) {
                    Ok(ord) => ord,
                    Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                }
            });
            if let Some(e) = sort_err { return Err(e); }
        }
        if reverse {
            items.reverse();
        }
        Ok(Value::list(items))
    }

    /// CPython: min(iterable, /, *, key=None) or min(*args, key=None).
    /// <https://docs.python.org/3/library/functions.html#min>
    #[pure]
    fn min(args) -> Result<Value> {
        min_max_impl(_interp, args, false, FN_NAME)
    }

    /// CPython: max(iterable, /, *, key=None) or max(*args, key=None).
    /// <https://docs.python.org/3/library/functions.html#max>
    #[pure]
    fn max(args) -> Result<Value> {
        min_max_impl(_interp, args, true, FN_NAME)
    }

    /// CPython: round(number[, ndigits]) — banker's rounding.
    /// <https://docs.python.org/3/library/functions.html#round>
    #[pure]
    fn round(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 1 or 2 arguments")));
        }
        let ndigits: Option<i32> = if args.len() == 2 {
            match args[1].value.kind() {
                ValueKind::Int(n) => Some(n as i32),
                ValueKind::None => None,
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() ndigits must be an integer or None"),
                )),
            }
        } else {
            None
        };
        match args[0].value.kind() {
            ValueKind::Int(v) => Ok(Value::int(v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
            ValueKind::Float(v) => {
                match ndigits {
                    None => Ok(Value::int(py_round_half_even(v))),
                    Some(n) => {
                        if n >= 0 {
                            let factor = 10f64.powi(n);
                            Ok(Value::float(py_round_half_even_f64(v * factor) / factor))
                        } else {
                            let factor = 10f64.powi(-n);
                            Ok(Value::float(py_round_half_even_f64(v / factor) * factor))
                        }
                    }
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() argument must be a number"),
            )),
        }
    }

    /// CPython: list([iterable]) — list constructor.
    /// <https://docs.python.org/3/library/functions.html#list>
    #[pure]
    fn list(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::list(vec![])),
            1 => Ok(Value::list(_interp.collect_iterable(args[0].value.clone())?)),
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most one argument"))),
        }
    }

    /// CPython: tuple([iterable]) — tuple constructor.
    /// <https://docs.python.org/3/library/functions.html#tuple>
    #[pure]
    fn tuple(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::tuple(vec![])),
            1 => Ok(Value::tuple(_interp.collect_iterable(args[0].value.clone())?)),
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most one argument"))),
        }
    }

    /// CPython: bytes() — bytes constructor.
    /// <https://docs.python.org/3/library/functions.html#func-bytes>
    #[pure]
    fn bytes(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::bytes(Vec::new())),
            1 => match args[0].value.kind() {
                ValueKind::Int(n) => {
                    if n < 0 {
                        return Err(PyError::named("ValueError", "negative count".to_string()));
                    }
                    Ok(Value::bytes(vec![0u8; n as usize]))
                }
                ValueKind::Bytes(rc) => Ok(Value::bytes((**rc).clone())),
                ValueKind::Str(_) => Err(PyError::named(
                    "TypeError",
                    "string argument without an encoding".to_string(),
                )),
                ValueKind::List(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for v in items.iter() {
                        match v.kind() {
                            ValueKind::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
                            ValueKind::Int(_) => return Err(PyError::named(
                                "ValueError",
                                "bytes must be in range(0, 256)".to_string(),
                            )),
                            _ => return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "'{}' object cannot be interpreted as an integer",
                                    pyrust_core::builtin_type_name(v),
                                ),
                            )),
                        }
                    }
                    Ok(Value::bytes(out))
                }
                ValueKind::Tuple(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for v in items.iter() {
                        match v.kind() {
                            ValueKind::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
                            ValueKind::Int(_) => return Err(PyError::named(
                                "ValueError",
                                "bytes must be in range(0, 256)".to_string(),
                            )),
                            _ => return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "'{}' object cannot be interpreted as an integer",
                                    pyrust_core::builtin_type_name(v),
                                ),
                            )),
                        }
                    }
                    Ok(Value::bytes(out))
                }
                _ => Err(PyError::named(
                    "TypeError",
                    "cannot convert to bytes".to_string(),
                )),
            },
            // bytes(source, encoding[, errors]) — encode `source` using
            // `encoding`.  CPython accepts a wide spectrum of codecs; we
            // support the common ASCII-compatible ones (utf-8, ascii,
            // latin-1) and reject the rest with `LookupError` for parity
            // with `LookupError: unknown encoding: <name>`. (#391)
            2 | 3 => {
                let source: String = match args[0].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::named(
                        "TypeError",
                        "encoding without a string argument".to_string(),
                    )),
                };
                let encoding: String = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::named(
                        "TypeError",
                        "bytes() argument 2 must be str, not non-string".to_string(),
                    )),
                };
                let errors: String = if args.len() == 3 {
                    match args[2].value.kind() {
                        ValueKind::Str(s) => s.to_string(),
                        _ => return Err(PyError::named(
                            "TypeError",
                            "bytes() argument 3 must be str, not non-string".to_string(),
                        )),
                    }
                } else {
                    "strict".to_string()
                };
                encode_str_to_bytes(&source, &encoding, &errors)
            }
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most 3 arguments"))),
        }
    }

    /// CPython: complex(real=0, imag=0) — complex number.
    /// <https://docs.python.org/3/library/functions.html#complex>
    #[pure]
    fn complex(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let to_f64 = |v: &Value, what: &str| -> Result<f64> {
            match v.kind() {
                ValueKind::Int(n) => Ok(n as f64),
                ValueKind::Float(f) => Ok(f),
                ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
                _ => Err(PyError::named(
                    "TypeError",
                    format!("complex() {what} argument must be a number"),
                )),
            }
        };
        match args.len() {
            0 => Ok(Value::complex(0.0, 0.0)),
            1 => match args[0].value.kind() {
                ValueKind::Complex(re, im) => Ok(Value::complex(re, im)),
                _ => Ok(Value::complex(to_f64(&args[0].value, "real")?, 0.0)),
            },
            2 => {
                let re = to_f64(&args[0].value, "real")?;
                let im = to_f64(&args[1].value, "imag")?;
                Ok(Value::complex(re, im))
            }
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most 2 arguments"))),
        }
    }

    /// CPython: set([iterable]) — set constructor.
    /// <https://docs.python.org/3/library/functions.html#func-set>
    #[pure]
    fn set(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::set(indexmap::IndexSet::new())),
            1 => {
                let items = _interp.collect_iterable(args[0].value.clone())?;
                let mut set = indexmap::IndexSet::new();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                Ok(Value::set(set))
            }
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most one argument"))),
        }
    }

    /// CPython: frozenset([iterable]) — frozenset constructor.
    /// <https://docs.python.org/3/library/functions.html#func-frozenset>
    #[pure]
    fn frozenset(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(pyrust_builtins::frozenset::frozenset(indexmap::IndexSet::new())),
            1 => {
                // frozenset(frozenset_instance) returns the same object (per CPython).
                if let Some(rc) = pyrust_builtins::frozenset::as_items(&args[0].value) {
                    return Ok(pyrust_builtins::frozenset::frozenset_rc(rc));
                }
                let items = _interp.collect_iterable(args[0].value.clone())?;
                let mut set = indexmap::IndexSet::new();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                Ok(pyrust_builtins::frozenset::frozenset(set))
            }
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most one argument"))),
        }
    }

    /// CPython: str(object='') — string constructor.
    /// <https://docs.python.org/3/library/functions.html#func-str>
    #[pure]
    fn str(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::string(String::new())),
            1 => Ok(Value::string(render_instance_str(_interp, &args[0].value)?)),
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most one argument"))),
        }
    }

    /// CPython: int(x=0, base=10) — integer constructor.
    /// <https://docs.python.org/3/library/functions.html#int>
    #[pure]
    fn int(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::int(0)),
            1 => match args[0].value.kind() {
                ValueKind::Int(v) => Ok(Value::int(v)),
                ValueKind::Float(v) => Ok(Value::int(v as i64)),
                ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                ValueKind::Str(s) => s.trim().parse::<i64>().map(Value::int).map_err(|_| {
                    PyError::named(
                        "ValueError",
                        format!("invalid literal for int() with base 10: '{s}'"),
                    )
                }),
                _ => Err(PyError::Runtime(format!(
                    "{FN_NAME}() argument must be a number or string",
                ))),
            },
            2 => {
                let base = match args[1].value.kind() {
                    ValueKind::Int(b) if (2..=36).contains(&b) => b as u32,
                    ValueKind::Int(b) => return Err(PyError::named(
                        "ValueError",
                        format!("int() base must be >= 2 and <= 36, or 0, not {b}"),
                    )),
                    _ => return Err(PyError::Runtime(format!("{FN_NAME}() base must be an integer"))),
                };
                match args[0].value.kind() {
                    ValueKind::Str(s) => {
                        let stripped = s.trim();
                        let stripped = if (base == 16 && (stripped.starts_with("0x") || stripped.starts_with("0X")))
                            || (base == 2 && (stripped.starts_with("0b") || stripped.starts_with("0B")))
                            || (base == 8 && (stripped.starts_with("0o") || stripped.starts_with("0O")))
                        {
                            &stripped[2..]
                        } else {
                            stripped
                        };
                        i64::from_str_radix(stripped, base).map(Value::int).map_err(|_| {
                            PyError::named(
                                "ValueError",
                                format!("invalid literal for int() with base {base}: '{}'", s.trim()),
                            )
                        })
                    }
                    _ => Err(PyError::Runtime(format!(
                        "{FN_NAME}() can't convert non-string with explicit base",
                    ))),
                }
            }
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most two arguments"))),
        }
    }

    /// CPython: float(x=0.0) — float constructor.
    /// <https://docs.python.org/3/library/functions.html#float>
    #[pure]
    fn float(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::float(0.0)),
            1 => match args[0].value.kind() {
                ValueKind::Float(v) => Ok(Value::float(v)),
                ValueKind::Int(v) => Ok(Value::float(v as f64)),
                ValueKind::Bool(b) => Ok(Value::float(if b { 1.0 } else { 0.0 })),
                ValueKind::Str(s) => s.trim().parse::<f64>().map(Value::float).map_err(|_| {
                    PyError::Runtime(format!("could not convert string to float: '{s}'"))
                }),
                _ => Err(PyError::Runtime(format!(
                    "{FN_NAME}() argument must be a number or string",
                ))),
            },
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most one argument"))),
        }
    }

    /// CPython: bool(x=False) — bool constructor.
    /// <https://docs.python.org/3/library/functions.html#bool>
    ///
    /// Migrated to the typed-signature dialect (#400).  `Option<PyValue>`
    /// + `#[default(None)]` is the natural shape for a single optional
    /// positional: `None` means "no arg" → False, `Some(v)` means
    /// "compute truthiness of v".  Conflating `bool()` with `bool(None)`
    /// is safe because CPython's truthiness of `None` is also False, so
    /// both paths land on the same answer.
    #[pure]
    fn bool(
        #[positional_only]
        #[default(None)]
        x: Option<PyValue>,
    ) -> Result<Value> {
        match x {
            // No-arg path returns `Value::bool_(false)` directly, skipping
            // `_interp.truthy_value`.  This is equivalent (not incidental):
            // `truthy_value(&Value::none())` would also resolve to False and
            // has no observable side effects, so the shortcut is intentional.
            None => Ok(Value::bool_(false)),
            Some(v) => {
                let result = _interp.truthy_value(&v.0)?;
                Ok(Value::bool_(result))
            }
        }
    }

    /// CPython: dict() — empty dict (rich constructor forms unsupported).
    /// <https://docs.python.org/3/library/functions.html#func-dict>
    #[pure]
    fn dict(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() {
            Ok(Value::dict(indexmap::IndexMap::new()))
        } else {
            Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() with arguments is not yet supported"),
            ))
        }
    }

    /// CPython: print(*objects, sep=' ', end='\n', file=sys.stdout, flush=False).
    /// <https://docs.python.org/3/library/functions.html#print>
    fn print(args) -> Result<Value> {
        let print_options = _interp.parse_print_options_expanded(args)?;
        let mut rendered = Vec::with_capacity(print_options.values.len());
        for value in &print_options.values {
            rendered.push(render_instance_str(_interp, value)?);
        }
        print!("{}{}", rendered.join(&print_options.sep), print_options.end);
        Ok(Value::none())
    }

    /// CPython: range(stop) / range(start, stop[, step]).
    /// <https://docs.python.org/3/library/functions.html#func-range>
    #[pure]
    fn range(args) -> Result<Value> {
        _interp.call_range_expanded(args)
    }

    /// CPython: open(file, mode='r', ...).
    /// <https://docs.python.org/3/library/functions.html#open>
    ///
    /// First builtin migrated to the typed-signature dialect (#395) — the
    /// macro-emitted prelude rejects unknown kwargs, validates the positional
    /// count, and binds `path` / `mode` as typed Rust locals.
    fn open(
        path: PyStr,
        #[default("r".into())]
        mode: PyStr,
    ) -> Result<Value> {
        pyrust_builtins::file::open(&path, &mode)
    }

    /// Internal: variadic-call helper used by call sites that unpack
    /// `*args` / `**kwargs`.  Not a Python-level public function, but
    /// shipped under this name so the generated bytecode can reach it.
    fn __vcall__(args) -> Result<Value> {
        if args.len() != 3 {
            return Err(PyError::Runtime(format!("{FN_NAME} requires 3 arguments")));
        }
        let func = args[0].value.clone();
        let pos_items = _interp.collect_iterable(args[1].value.clone())?;
        let mut expanded: Vec<ExpandedCallArg> = pos_items
            .into_iter()
            .map(|v| ExpandedCallArg { name: None, value: v })
            .collect();
        if let ValueKind::Dict(kw_map) = args[2].value.kind() {
            for (k, v) in kw_map.iter() {
                if let PyKey::Str(name) = k {
                    expanded.push(ExpandedCallArg { name: Some(name.clone()), value: v.clone() });
                }
            }
        }
        _interp.call_function_expanded(func, &expanded)
    }

    /// CPython: format(value[, format_spec]).
    /// <https://docs.python.org/3/library/functions.html#format>
    ///
    /// Migrated to the typed-signature dialect (#400).  Both params are
    /// `#[positional_only]` so the macro emits the fast-path prelude that
    /// skips kwarg validation entirely.  The optional `format_spec` is
    /// encoded as `Option<PyStr>` — absent → `None` (treated as `""`),
    /// present-and-str → `Some(PyStr)`, present-and-non-str → the typed
    /// dialect's standard "must be str or None" TypeError.
    fn format(
        #[positional_only] value: PyValue,
        #[positional_only]
        #[default(None)]
        format_spec: Option<PyStr>,
    ) -> Result<Value> {
        let value = &value.0;
        let spec: &str = format_spec.as_ref().map(|s| s.as_ref()).unwrap_or("");
        // Dispatch __format__(spec) for user instances.
        if let ValueKind::PyInstance(instance) = value.kind() {
            let instance_rc = Rc::clone(instance);
            let class = Rc::clone(&instance_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__format__") {
                let result = invoke_class_method(
                    _interp,
                    method_val,
                    Value::py_instance(instance_rc),
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string(spec),
                    }],
                )?;
                let is_str = matches!(result.kind(), ValueKind::Str(_));
                return if is_str {
                    Ok(result)
                } else {
                    Err(PyError::named(
                        "TypeError",
                        format!(
                            "__format__ must return a str, not {}",
                            value_type_name_str(&result),
                        ),
                    ))
                };
            }
        }
        apply_format_spec(value, spec)
    }

    /// CPython: classmethod(function) — class-method descriptor.
    /// <https://docs.python.org/3/library/functions.html#classmethod>
    fn classmethod(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::UserFunction(f) => Ok(Value::class_method(Rc::clone(f))),
            _ => Err(PyError::Runtime(format!("{FN_NAME}() argument must be a function"))),
        }
    }

    /// CPython: staticmethod(function) — static-method descriptor.
    /// <https://docs.python.org/3/library/functions.html#staticmethod>
    fn staticmethod(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::UserFunction(f) => Ok(Value::static_method(Rc::clone(f))),
            _ => Err(PyError::Runtime(format!("{FN_NAME}() argument must be a function"))),
        }
    }

    /// CPython: property(fget=None, fset=None, fdel=None, doc=None).
    /// <https://docs.python.org/3/library/functions.html#property>
    fn property(args) -> Result<Value> {
        // Accept up to 4 positional args (fget, fset, fdel, doc) or keyword args.
        if args.len() > 4 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes at most 4 arguments")));
        }
        let mut fget = Value::none();
        let mut fset = Value::none();
        let mut fdel = Value::none();
        for (i, arg) in args.iter().enumerate() {
            let name_ref = arg.name.as_deref();
            let idx = match name_ref {
                None => i,
                Some("fget") => 0,
                Some("fset") => 1,
                Some("fdel") => 2,
                Some("doc") => 3,
                Some(k) => return Err(PyError::Runtime(format!(
                    "{FN_NAME}() got unexpected keyword argument '{k}'",
                ))),
            };
            match idx {
                0 => fget = arg.value.clone(),
                1 => fset = arg.value.clone(),
                2 => fdel = arg.value.clone(),
                _ => {} // doc: ignore
            }
        }
        Ok(pyrust_builtins::property::property(fget, fset, fdel))
    }

    /// CPython: super(class, instance) — two-argument form only.
    /// Zero-argument `super()` (implicit `__class__` cell) is not supported;
    /// users must pass both arguments explicitly.
    /// <https://docs.python.org/3/library/functions.html#super>
    ///
    /// The Rust fn is named `super_fn` because `super` is a strict Rust
    /// keyword that is *also* rejected as a raw identifier — `r#super`
    /// won't parse — so the `#[py_name = "super"]` override is the only
    /// way to give this callable its Python-level name.
    #[py_name = "super"]
    fn super_fn(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() requires exactly 2 arguments: super(CurrentClass, self)",
            )));
        }
        let cls_val = args[0].value.clone();
        let inst_val = args[1].value.clone();
        let class = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => return Err(PyError::Runtime(format!(
                "{FN_NAME}() first argument must be a class",
            ))),
        };
        match inst_val.kind() {
            ValueKind::PyInstance(i) => {
                let instance = Rc::clone(i);
                // Bug #199: validate instance is an instance of class.
                if !class_is_subclass_of(&instance.borrow().class, &class) {
                    return Err(PyError::named(
                        "TypeError",
                        "super(type, obj): obj must be an instance or subtype of type".to_string(),
                    ));
                }
                Ok(Value::super_proxy(class, instance))
            }
            ValueKind::PyClass(obj_class) => {
                // Bug #197: classmethod case — second arg is a class.
                let obj_class = Rc::clone(obj_class);
                if !class_is_subclass_of(&obj_class, &class) {
                    return Err(PyError::named(
                        "TypeError",
                        "super(type, obj): obj must be an instance or subtype of type".to_string(),
                    ));
                }
                Ok(Value::super_proxy_class(class, obj_class))
            }
            _ => Err(PyError::Runtime(format!(
                "{FN_NAME}() second argument must be a class instance",
            ))),
        }
    }

    /// CPython: callable(object) — true if the object is callable.
    /// <https://docs.python.org/3/library/functions.html#callable>
    ///
    /// Migrated to the typed-signature dialect (#400).  Mirrors `ascii`
    /// / `id`: a single-body `PyValue` catch-all, since `callable`
    /// accepts every Python object and never raises `TypeError`.
    #[pure]
    fn callable(#[positional_only] obj: PyValue) -> Result<Value> {
        let value = &obj.0;
        let is_callable = match value.kind() {
            ValueKind::UserFunction(_)
            | ValueKind::BuiltinFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. }
            | ValueKind::PyClass(_) => true,
            // Only accessor partials (intermediate results of
            // prop.setter / prop.getter / prop.deleter) are callable —
            // a plain property descriptor isn't.
            ValueKind::BuiltinObject { .. } => {
                pyrust_builtins::property::property_partial_slot(value)
                    .is_some_and(|slot| slot.is_some())
            }
            ValueKind::PyInstance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                lookup_class_attr(&class, "__call__").is_some()
            }
            _ => false,
        };
        Ok(Value::bool_(is_callable))
    }
}

/// If `v` is a user `PyInstance` (which needs `__iter__` / `__getitem__`
/// dispatch via the interpreter) or a `Generator` (which needs the
/// interpreter to drive the `GeneratorFrame`), drain it eagerly into a
/// `Value::list` so downstream lazy iter helpers (`enumerate` / `zip` /
/// `reversed` / `chain`) can reach its items through
/// `iter_values_via_registry` — that callback can't dispatch dunders or
/// resume generators by itself.  Non-user sources are passed through
/// unchanged, preserving lazy evaluation for builtin iterables (e.g.
/// `enumerate(open(path))` still defers file-reading until iter).
///
/// Issue #446.
pub(super) fn materialize_user_iter(
    interp: &mut crate::Interpreter,
    v: Value,
) -> Result<Value> {
    if matches!(v.kind(), ValueKind::PyInstance(_) | ValueKind::Generator(_)) {
        let items = interp.collect_iterable(v)?;
        Ok(Value::list(items))
    } else {
        Ok(v)
    }
}

/// Compute the hash of a `Value` for the `hash()` builtin.  Mirrors
/// CPython's semantics:
/// - numeric types use their integer value (so `hash(True) == hash(1)`
///   and `hash(1.0) == hash(1)`);
/// - strings use an FNV-1a-style byte hash;
/// - tuples fold each element's hash with a CPython-style xor/mul mix,
///   recursing through nested tuples (issue #382);
/// - mutable containers (list / dict / set) raise `TypeError`.
fn hash_value(value: &Value) -> Result<i64> {
    match value.kind() {
        ValueKind::Int(v) => Ok(v),
        ValueKind::Bool(b) => Ok(b as i64),
        ValueKind::Float(v) => {
            if v.fract() == 0.0 && v.is_finite() {
                Ok(v as i64)
            } else {
                Ok(v.to_bits() as i64)
            }
        }
        ValueKind::Str(s) => {
            let mut h: u64 = 14695981039346656037u64;
            for b in s.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(1099511628211u64);
            }
            Ok(h as i64)
        }
        ValueKind::None => Ok(0),
        ValueKind::Tuple(items) => {
            let mut h: i64 = 3527539;
            for item in items {
                let item_hash = hash_value(item)?;
                h = h.wrapping_mul(1000003).wrapping_add(item_hash);
            }
            Ok(h)
        }
        ValueKind::List(_) => Err(PyError::named(
            "TypeError",
            "unhashable type: 'list'".to_string(),
        )),
        ValueKind::Dict(_) => Err(PyError::named(
            "TypeError",
            "unhashable type: 'dict'".to_string(),
        )),
        ValueKind::Set(_) => Err(PyError::named(
            "TypeError",
            "unhashable type: 'set'".to_string(),
        )),
        // PyInstance arriving here means either the caller didn't intercept
        // it for __hash__ dispatch (e.g. a tuple element), or no __hash__
        // method exists.  Use the actual class name rather than the generic
        // "object" returned by builtin_type_name.
        ValueKind::PyInstance(inst) => {
            let class_name = inst.borrow().class.borrow().name.clone();
            Err(PyError::named(
                "TypeError",
                format!("unhashable type: '{class_name}'"),
            ))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "unhashable type: '{}'",
                pyrust_core::builtin_type_name(value)
            ),
        )),
    }
}

/// Single-class `isinstance` check — `obj` against one concrete class
/// value (i.e. *not* a tuple).  Issue #462: the 11 migrated primitive
/// types (`int`, `str`, `list`, …) are real `PyClass` values now, so
/// their `isinstance` resolves through the standard `class_is_subclass_of`
/// walk — no per-type hard-coded arms.  Only `NoneType` and `BuiltinObject`
/// (frozenset, range, enumerate, …) still take the legacy
/// `BuiltinFunction(name)` path until they're migrated too.
fn isinstance_single(obj: &Value, cls: &Value) -> bool {
    // Migrated primitives: `type(obj)` returns the per-thread PyClass
    // singleton, so a class-vs-class walk handles every primitive check
    // (including `bool` → `int` via base inheritance).
    if let ValueKind::PyClass(expected) = cls.kind() {
        // Fast path: if `expected` is one of the 11 primitive class
        // singletons, do a direct `ValueKind` tag check.  Skips the
        // `primitive_class_for_value` thread_local + Rc::clone + the
        // base-chain walk, recovering most of the master-vs-PR
        // `isinstance` regression (#462).
        if let Some(hit) = crate::interpreter::primitive_class_isinstance_fast(obj, expected) {
            return hit;
        }
        let actual_class = match obj.kind() {
            ValueKind::PyInstance(inst) => Some(Rc::clone(&inst.borrow().class)),
            _ => crate::interpreter::primitive_class_for_value(obj),
        };
        if let Some(actual) = actual_class {
            return class_is_subclass_of(&actual, expected);
        }
        return false;
    }
    // Non-class `cls` operands are an error at the API boundary
    // (`isinstance_check` rejects them); the only remaining match here is
    // the legacy `BuiltinFunction(name)` path for types that haven't been
    // migrated to PyClass yet (`NoneType` and any future builtin-only
    // type tokens).
    match (obj.kind(), cls.kind()) {
        (ValueKind::None, ValueKind::BuiltinFunction("NoneType")) => true,
        (ValueKind::BuiltinObject { ops, .. }, ValueKind::BuiltinFunction(name)) => {
            ops.type_name() == name
        }
        _ => false,
    }
}

/// `isinstance(obj, classinfo)` — accept a class *or* an
/// arbitrarily-nested tuple of classes, matching CPython's recursive
/// contract.  Raises `TypeError` if a leaf is neither a class nor a
/// tuple.  See <https://docs.python.org/3/library/functions.html#isinstance>.
fn isinstance_check(fn_name: &str, obj: &Value, cls: &Value) -> Result<bool> {
    if let ValueKind::Tuple(items) = cls.kind() {
        for item in items {
            if isinstance_check(fn_name, obj, item)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    if !is_class_like(cls) {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() arg 2 must be a type, a tuple of types, or a union"),
        ));
    }
    Ok(isinstance_single(obj, cls))
}

/// `issubclass(cls, classinfo)` — same tuple-recursive contract as
/// `isinstance_check`, but compares classes rather than instances.
fn issubclass_check(fn_name: &str, cls: &Value, classinfo: &Value) -> Result<bool> {
    if let ValueKind::Tuple(items) = classinfo.kind() {
        for item in items {
            if issubclass_check(fn_name, cls, item)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    match (cls.kind(), classinfo.kind()) {
        // User-defined → user-defined: walk the `base` chain.
        (ValueKind::PyClass(c), ValueKind::PyClass(expected)) => {
            Ok(class_is_subclass_of(&c, &expected))
        }
        // User-defined → builtin type token: never a match in PyRust
        // (user classes don't inherit from built-in types here).
        (ValueKind::PyClass(_), ValueKind::BuiltinFunction(_)) => Ok(false),
        // Builtin type token → builtin type token: handle the small
        // hard-coded relations (`bool` ⊂ `int`, anything ⊂ itself,
        // anything ⊂ `object`).
        (ValueKind::BuiltinFunction(a), ValueKind::BuiltinFunction(b)) => {
            Ok(builtin_is_subclass_of(a, b))
        }
        // Builtin → user-defined: never matches.
        (ValueKind::BuiltinFunction(_), ValueKind::PyClass(_)) => Ok(false),
        (_, ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_)) => Ok(false),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() arg 2 must be a class, a tuple of classes, or a union"),
        )),
    }
}

/// True if built-in type token `a` is a subclass of token `b`.  Only
/// the CPython-documented built-in relations matter here: every type
/// is a subclass of itself and of `object`; `bool` is a subclass of
/// `int`.
fn builtin_is_subclass_of(a: &str, b: &str) -> bool {
    if a == b || b == "object" {
        return true;
    }
    matches!((a, b), ("bool", "int"))
}

/// True if `v` looks like a class-info leaf accepted by
/// `isinstance`/`issubclass` — either a user-defined `PyClass` or a
/// built-in type token (`BuiltinFunction("int")` etc.).
fn is_class_like(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_),
    )
}

/// Format an i64 as Python's `hex()` output — `"0xN"` / `"-0xN"`.  Used
/// by both the `PyInt` and `PyBool` overloads of the typed `hex`
/// builtin (#400).  Widens through i128 first so `i64::MIN.abs()`
/// doesn't overflow.
fn format_hex_i64(v: i64) -> String {
    if v < 0 {
        format!("-0x{:x}", -(v as i128))
    } else {
        format!("0x{:x}", v)
    }
}

/// Validate a codepoint and return the corresponding single-char `str`
/// `Value`.  Shared by the `PyInt` and `PyBool` overloads of the typed
/// `chr` builtin (#400).  Out-of-range codepoints raise `ValueError`
/// with CPython-style wording preserved verbatim from the legacy body.
fn chr_from_code_point(code_point: i64) -> Result<Value> {
    if !(0..=1114111).contains(&code_point) {
        return Err(PyError::named(
            "ValueError",
            format!("chr() arg not in range(0x110000): {code_point}"),
        ));
    }
    let ch = char::from_u32(code_point as u32).ok_or_else(|| {
        PyError::named(
            "ValueError",
            format!("chr() arg not in range(0x110000): {code_point}"),
        )
    })?;
    Ok(Value::string(ch.to_string()))
}

/// Format a single Unicode codepoint the way CPython does in
/// `UnicodeEncodeError` messages: `\xXX` for `< 0x100`, `\uXXXX` for
/// `< 0x10000`, otherwise `\UXXXXXXXX`.  Keeps the error wording
/// byte-for-byte aligned with CPython so the parity tests can compare
/// stderr verbatim.
fn format_codepoint_repr(cp: u32) -> String {
    if cp < 0x100 {
        format!("\\x{:02x}", cp)
    } else if cp < 0x10000 {
        format!("\\u{:04x}", cp)
    } else {
        format!("\\U{:08x}", cp)
    }
}

/// Encode a Python `str` to `bytes` for `bytes(source, encoding[, errors])`
/// (#391).  Supports `utf-8`, `ascii`, `latin-1` (and CPython aliases) —
/// the realistic minimum the issue requested.  Other encoding names
/// raise `LookupError: unknown encoding: <name>` for CPython parity.
///
/// `errors="strict"` (default) raises `UnicodeEncodeError` on bytes the
/// target codec can't represent; `"ignore"` silently drops them; and
/// `"replace"` substitutes `b'?'` (matching CPython's ASCII-codec
/// replacement byte for both `ascii` and `latin-1`).  Richer handlers
/// (`backslashreplace`, `xmlcharrefreplace`, …) are still out of scope
/// and reported via `LookupError: unknown error handler`.
fn encode_str_to_bytes(source: &str, encoding: &str, errors: &str) -> Result<Value> {
    // CPython normalises encoding names by lowercasing and treating
    // `_` and `-` interchangeably; do the same so `UTF-8`, `utf_8`,
    // `UTF8` all resolve to the same codec.
    fn normalize(name: &str) -> String {
        name.to_ascii_lowercase().replace('_', "-")
    }
    let canonical = normalize(encoding);

    // Error-handler dispatch.  CPython only consults the error handler
    // when the codec actually encounters an unencodable character, so
    // `bytes("hi", "ascii", "bogus")` succeeds — the bad handler name
    // is never reached.  We therefore resolve the handler lazily inside
    // `encode_single_byte_codec`, at the first unencodable codepoint.
    // Unknown encoding names still fail unconditionally (before any
    // encoding work) so the encoding check always runs first.
    enum Handler {
        Strict,
        Ignore,
        Replace,
    }

    /// Resolve the error handler name.  Called only when an unencodable
    /// codepoint is actually encountered, matching CPython's lazy lookup.
    fn resolve_handler(errors: &str) -> Result<Handler> {
        match errors {
            "strict" => Ok(Handler::Strict),
            "ignore" => Ok(Handler::Ignore),
            "replace" => Ok(Handler::Replace),
            other => Err(PyError::named(
                "LookupError",
                format!("unknown error handler name '{other}'"),
            )),
        }
    }

    // Single-codec encoder kernel.  `fits(cp)` returns true if the
    // codepoint can be emitted as a single byte; `range_label` is
    // the `ordinal not in range(N)` suffix CPython uses.  Hoisting
    // the loop out of the ascii/latin-1 arms keeps the handler logic
    // (and any future codec aliases) in one place.
    //
    // For the strict path CPython groups a contiguous run of unencodable
    // characters into one error: e.g. `bytes("éé", "ascii")` reports
    // "characters in position 0-1" rather than stopping at the first
    // one.  We scan ahead to find the end of the run before building
    // the message, matching CPython's UnicodeEncodeError wording exactly.
    fn encode_single_byte_codec(
        source: &str,
        codec_name: &str,
        fits: impl Fn(u32) -> bool,
        range_label: &str,
        errors: &str,
    ) -> Result<Value> {
        let chars: Vec<char> = source.chars().collect();
        let mut out = Vec::with_capacity(source.len());
        let mut idx = 0usize;
        while idx < chars.len() {
            let cp = chars[idx] as u32;
            if fits(cp) {
                out.push(cp as u8);
                idx += 1;
            } else {
                // Lazy handler resolution — only reached on unencodable char.
                match resolve_handler(errors)? {
                    Handler::Ignore => {
                        idx += 1;
                    }
                    Handler::Replace => {
                        out.push(b'?');
                        idx += 1;
                    }
                    Handler::Strict => {
                        // Scan the contiguous run of unencodable characters
                        // starting at `idx`.  CPython's UnicodeEncodeError
                        // covers the entire run: "characters in position S-E"
                        // (inclusive, 0-based) when len > 1, or the single-
                        // char form "character '\\xNN' in position S" when the
                        // run is length 1.
                        let run_start = idx;
                        let mut run_end = idx + 1; // exclusive
                        while run_end < chars.len() && !fits(chars[run_end] as u32) {
                            run_end += 1;
                        }
                        let run_len = run_end - run_start;
                        let msg = if run_len == 1 {
                            format!(
                                "'{codec_name}' codec can't encode character '{}' in position {run_start}: ordinal not in range({range_label})",
                                format_codepoint_repr(cp),
                            )
                        } else {
                            format!(
                                "'{codec_name}' codec can't encode characters in position {run_start}-{}: ordinal not in range({range_label})",
                                run_end - 1,
                            )
                        };
                        return Err(PyError::named("UnicodeEncodeError", msg));
                    }
                }
            }
        }
        Ok(Value::bytes(out))
    }

    match canonical.as_str() {
        // pyrust strings are UTF-8 internally — encoding to utf-8 is a
        // direct copy and can never fail, so the error handler doesn't
        // matter.
        "utf-8" | "utf8" | "u8" | "utf" => Ok(Value::bytes(source.as_bytes().to_vec())),
        "ascii" | "us-ascii" | "646" => {
            encode_single_byte_codec(source, "ascii", |cp| cp < 0x80, "128", errors)
        }
        "latin-1" | "iso-8859-1" | "8859" | "cp819" | "latin1" | "l1" => {
            encode_single_byte_codec(source, "latin-1", |cp| cp < 0x100, "256", errors)
        }
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {encoding}"),
        )),
    }
}

/// Format an i64 as Python's `bin()` output — `"0bN"` / `"-0bN"`.  Used
/// by both the `PyInt` and `PyBool` overloads of the typed `bin`
/// builtin (#400).  Widens through i128 first so `i64::MIN.abs()`
/// doesn't overflow.
fn format_bin_i64(v: i64) -> String {
    if v < 0 {
        format!("-0b{:b}", -(v as i128))
    } else {
        format!("0b{:b}", v)
    }
}

/// Format an i64 as Python's `oct()` output — `"0oN"` / `"-0oN"`.  Used
/// by both the `PyInt` and `PyBool` overloads of the typed `oct`
/// builtin (#400).  Widens through i128 first so `i64::MIN.abs()`
/// doesn't overflow.
fn format_oct_i64(v: i64) -> String {
    if v < 0 {
        format!("-0o{:o}", -(v as i128))
    } else {
        format!("0o{:o}", v)
    }
}

/// Shared implementation for `min` / `max` — both accept the same
/// argument shapes (single-iterable or 2+ positionals, plus a `key=`
/// kwarg) and only differ in the ordering direction.
fn min_max_impl(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    is_max: bool,
    fn_name: &str,
) -> Result<Value> {
    let key_fn = args.iter().find(|a| a.name.as_deref() == Some("key"))
        .map(|a| a.value.clone());
    for a in args.iter().filter(|a| a.name.is_some()) {
        if a.name.as_deref() != Some("key") {
            return Err(PyError::Runtime(format!(
                "{fn_name}() got an unexpected keyword argument '{}'",
                a.name.as_ref().unwrap()
            )));
        }
    }
    let positional: Vec<&ExpandedCallArg> =
        args.iter().filter(|a| a.name.is_none()).collect();
    let items: Vec<Value> = if positional.len() == 1 {
        interp.collect_iterable(positional[0].value.clone())?
    } else if positional.len() >= 2 {
        positional.iter().map(|a| a.value.clone()).collect()
    } else {
        return Err(PyError::Runtime(format!("{fn_name}() expected at least one argument")));
    };
    if items.is_empty() {
        return Err(PyError::Runtime(format!("{fn_name}() arg is an empty sequence")));
    }
    if let Some(kfn) = key_fn {
        let keyed: Vec<(Value, Value)> = items
            .into_iter()
            .map(|v| {
                let k = interp.call_function_expanded(
                    kfn.clone(),
                    &[ExpandedCallArg { name: None, value: v.clone() }],
                )?;
                Ok((k, v))
            })
            .collect::<Result<_>>()?;
        let mut result_err: Option<PyError> = None;
        let result = keyed.into_iter().reduce(|acc, item| {
            if result_err.is_some() { return acc; }
            match compare_values(&item.0, &acc.0) {
                Ok(cmp) => {
                    if (is_max && cmp == std::cmp::Ordering::Greater)
                        || (!is_max && cmp == std::cmp::Ordering::Less) { item }
                    else { acc }
                }
                Err(e) => { result_err = Some(e); acc }
            }
        }).unwrap();
        if let Some(e) = result_err { return Err(e); }
        Ok(result.1)
    } else {
        let mut result_err: Option<PyError> = None;
        let result = items.into_iter().reduce(|acc, v| {
            if result_err.is_some() { return acc; }
            match compare_values(&v, &acc) {
                Ok(cmp) => {
                    if (is_max && cmp == std::cmp::Ordering::Greater)
                        || (!is_max && cmp == std::cmp::Ordering::Less) { v }
                    else { acc }
                }
                Err(e) => { result_err = Some(e); acc }
            }
        }).unwrap();
        if let Some(e) = result_err { return Err(e); }
        Ok(result)
    }
}

/// Render `value` to its Python-string form, honouring `__str__` / `__repr__`
/// on user instances (in that priority order) and falling back to
/// `<ClassName object>` for instances of classes that define neither.
///
/// Shared by `print` and `str(x)` — both want the same dunder-aware
/// rendering, just wrapped differently (`print` collects into a `Vec<String>`,
/// `str(x)` returns a `Value::string(...)`).  Exception instances bypass
/// the dunder lookup and use the built-in `Value::to_py_str()` formatting,
/// matching CPython's special-cased `BaseException.__str__`.
fn render_instance_str(interp: &mut crate::Interpreter, value: &Value) -> Result<String> {
    let ValueKind::PyInstance(inst) = value.kind() else {
        return Ok(value.to_py_str());
    };
    let inst_rc = Rc::clone(&inst);
    let class = Rc::clone(&inst_rc.borrow().class);
    if is_exception_class(&class) {
        return Ok(value.to_py_str());
    }
    for dunder in &["__str__", "__repr__"] {
        if let Some(method_val) = lookup_class_attr(&class, dunder) {
            let result = invoke_class_method(
                interp,
                method_val,
                Value::py_instance(Rc::clone(&inst_rc)),
                &[],
            )?;
            return match result.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(PyError::named(
                    "TypeError",
                    format!("{dunder} returned non-string"),
                )),
            };
        }
    }
    Ok(format!("<{} object>", class.borrow().name))
}
