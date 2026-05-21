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
    NativeIterFrame, apply_format_spec, ascii_repr, bigint_divmod_floor, class_is_subclass_of,
    compare_values, compare_values_with_op, dir_names, instance_attrs_snapshot,
    int_pow_promoting, invoke_class_method,
    is_exception_class, iter_values, lookup_class_attr, modpow_i64, py_hash_bigint, py_hash_float,
    py_hash_int, py_mod_i64, py_round_half_even, py_round_half_even_f64,
    reject_keyword_args_expanded, resolve_zero_arg_super, snapshot_current_locals,
    sync_module_env_to_globals_dict,
    value_to_float, value_type_name_str,
};
use crate::value::{PyClass, PyKey, PyToPrimitive, PyZero, Value, ValueKind, range_len};
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
    ///
    /// Migrated to the typed-signature dialect (#400).  `iterable` is
    /// `PyValue` so user-defined iterables reach `collect_iterable`.
    /// `start` is `Option<PyValue>` with `#[default(None)]`; the body
    /// uses 0 when absent.  Known divergence: `sum([], None)` is 0
    /// (not CPython's `None`) because `Option<PyValue>` maps both
    /// "absent" and "Python None" to Rust `None`.  Tracked as a
    /// follow-up fixture under #400.
    #[pure]
    fn sum(
        #[positional_only] iterable: PyValue,
        #[positional_only]
        #[default(None)]
        start: Option<PyValue>,
    ) -> Result<Value> {
        let items = _interp.collect_iterable(iterable.0)?;
        let mut acc = match start {
            None => Value::int(0),
            Some(v) => v.0,
        };
        for item in items {
            acc = _interp.eval_binary(acc, BinaryOp::Add, item)?;
        }
        Ok(acc)
    }

    /// CPython: any(iterable) — true if any element is truthy.
    /// <https://docs.python.org/3/library/functions.html#any>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` as
    /// `iterable` so user-defined iterables (PyInstance with `__iter__`)
    /// reach `collect_iterable` rather than the registry-only path.
    #[pure]
    fn any(#[positional_only] iterable: PyValue) -> Result<Value> {
        let items = _interp.collect_iterable(iterable.0)?;
        for item in items {
            if _interp.truthy_value(&item)? {
                return Ok(Value::bool_(true));
            }
        }
        Ok(Value::bool_(false))
    }

    /// CPython: all(iterable) — true if every element is truthy (or empty).
    /// <https://docs.python.org/3/library/functions.html#all>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` as
    /// `iterable` — same rationale as `any`.
    #[pure]
    fn all(#[positional_only] iterable: PyValue) -> Result<Value> {
        let items = _interp.collect_iterable(iterable.0)?;
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
                return if matches!(result.kind(), ValueKind::Str(_)) {
                    Ok(result)
                } else {
                    Err(PyError::named(
                        "TypeError",
                        format!(
                            "__repr__ returned non-string (type {})",
                            pyrust_core::builtin_type_name(&result)
                        ),
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
        let hash_val = hash_value_with_interp(_interp, &value)?;
        Ok(Value::int(hash_val))
    }

    /// CPython: divmod(a, b) — `(a // b, a % b)`.
    /// <https://docs.python.org/3/library/functions.html#divmod>
    ///
    /// Migrated to the typed-signature dialect (#400).  Overloads cover every
    /// CPython-accepted type combination:
    ///  - `(int, int)` — including `BigInt`; `PyInt` wraps both small and big.
    ///  - `(bool, int)`, `(int, bool)`, `(bool, bool)` — `bool ⊆ int` in CPython,
    ///    so these return integer results (not float).
    ///  - `(float, float)`, `(float, int)`, `(float, bool)`, `(int, float)`,
    ///    `(bool, float)` — any mix involving a `float` returns floats.
    ///  - `(PyValue, PyValue)` catch-all raises `TypeError` with CPython's exact
    ///    `"unsupported operand type(s) for divmod(): 'X' and 'Y'"` wording.
    ///
    /// The `BigInt` arm inside each int-family overload promotes both operands via
    /// `to_bigint()` when either is `Big`, mirroring PR #485's cross-type arms in
    /// `expr.rs`.
    #[pure]
    fn divmod(#[positional_only] a: PyInt, #[positional_only] b: PyInt) -> Result<Value> {
        divmod_int_int(a, b)
    }

    #[pure]
    fn divmod(#[positional_only] a: PyBool, #[positional_only] b: PyInt) -> Result<Value> {
        // bool coerces to int: `True → 1`, `False → 0`.
        divmod_int_int(PyInt::from(a.0 as i64), b)
    }

    #[pure]
    fn divmod(#[positional_only] a: PyInt, #[positional_only] b: PyBool) -> Result<Value> {
        divmod_int_int(a, PyInt::from(b.0 as i64))
    }

    #[pure]
    fn divmod(#[positional_only] a: PyBool, #[positional_only] b: PyBool) -> Result<Value> {
        divmod_int_int(PyInt::from(a.0 as i64), PyInt::from(b.0 as i64))
    }

    #[pure]
    fn divmod(#[positional_only] a: PyFloat, #[positional_only] b: PyFloat) -> Result<Value> {
        divmod_float_float(*a, *b)
    }

    #[pure]
    fn divmod(#[positional_only] a: PyFloat, #[positional_only] b: PyInt) -> Result<Value> {
        // int→float coercion for the mixed arm.  BigInt may overflow f64 —
        // CPython raises `OverflowError` in that case.  For BigInt values that
        // fit in f64 (e.g. 2**100 ≈ 1.27e30) the conversion succeeds.
        let bf = pyint_to_f64(&b)?;
        divmod_float_float(*a, bf)
    }

    #[pure]
    fn divmod(#[positional_only] a: PyFloat, #[positional_only] b: PyBool) -> Result<Value> {
        divmod_float_float(*a, if b.0 { 1.0 } else { 0.0 })
    }

    #[pure]
    fn divmod(#[positional_only] a: PyInt, #[positional_only] b: PyFloat) -> Result<Value> {
        let af = pyint_to_f64(&a)?;
        divmod_float_float(af, *b)
    }

    #[pure]
    fn divmod(#[positional_only] a: PyBool, #[positional_only] b: PyFloat) -> Result<Value> {
        divmod_float_float(if a.0 { 1.0 } else { 0.0 }, *b)
    }

    /// Catch-all: any type combination not covered above → TypeError.
    #[pure]
    fn divmod(#[positional_only] a: PyValue, #[positional_only] b: PyValue) -> Result<Value> {
        Err(PyError::named(
            "TypeError",
            format!(
                "unsupported operand type(s) for divmod(): '{}' and '{}'",
                value_type_name_str(&a.0),
                value_type_name_str(&b.0),
            ),
        ))
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
    ///
    /// Migrated to the typed-signature dialect (#400).  `iterable` is
    /// `PyValue` (not `PyIterable`) so that user-defined `PyInstance`
    /// iterables reach `materialize_user_iter` — the registry-only path
    /// cannot dispatch `__iter__` dunders.  `start` is `Option<PyValue>`
    /// so the body can handle both `int` and `bool` inputs (CPython
    /// accepts both; `bool ⊆ int` in CPython) and produce the
    /// exact CPython `TypeError` wording for non-integer `start`.
    #[pure]
    fn enumerate(
        #[positional_only] iterable: PyValue,
        #[default(None)]
        start: Option<PyValue>,
    ) -> Result<Value> {
        let start_val: i64 = match start {
            None => 0,
            Some(v) => match v.0.kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        value_type_name_str(&v.0),
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
        let source = materialize_user_iter(_interp, iterable.0)?;
        Ok(pyrust_builtins::iter_helpers::enumerate(source, start_val))
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
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` accepts
    /// any object so that PyInstance / generator sources reach
    /// `materialize_user_iter` before handing off to the helper.
    #[pure]
    fn reversed(#[positional_only] seq: PyValue) -> Result<Value> {
        // Pre-materialise PyInstance sources (see `enumerate` for rationale).
        let source = materialize_user_iter(_interp, seq.0)?;
        Ok(pyrust_builtins::iter_helpers::reversed(source))
    }

    /// CPython: map(func, iterable) — apply func to each element.
    /// <https://docs.python.org/3/library/functions.html#map>
    ///
    /// Migrated to the typed-signature dialect (#400).  Both parameters
    /// use `PyValue`: `func` is a callable (any value), `iterable` is any
    /// iterable including user-defined PyInstance.
    fn map(
        #[positional_only] func: PyValue,
        #[positional_only] iterable: PyValue,
    ) -> Result<Value> {
        let items = _interp.collect_iterable(iterable.0)?;
        let mut result = Vec::with_capacity(items.len());
        for item in items {
            let mapped = _interp.call_function_expanded(
                func.0.clone(),
                &[ExpandedCallArg { name: None, value: item }],
            )?;
            result.push(mapped);
        }
        Ok(Value::list(result))
    }

    /// CPython: filter(func, iterable) — keep elements where func is truthy.
    /// <https://docs.python.org/3/library/functions.html#filter>
    ///
    /// Migrated to the typed-signature dialect (#400).  `func` and
    /// `iterable` are both `PyValue`: `func` may be `None` (identity
    /// truthiness test) or any callable; `iterable` may be any iterable
    /// including user-defined PyInstance.
    fn filter(
        #[positional_only] func: PyValue,
        #[positional_only] iterable: PyValue,
    ) -> Result<Value> {
        let items = _interp.collect_iterable(iterable.0)?;
        let use_identity = func.0.is_none();
        let mut result = Vec::new();
        for item in items {
            let keep = if use_identity {
                _interp.truthy_value(&item)?
            } else {
                let test = _interp.call_function_expanded(
                    func.0.clone(),
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
    ///
    /// Must stay in `(args)` dialect: `next(it, None)` is semantically
    /// distinct from `next(it)` — the former returns Python None when
    /// exhausted; the latter raises StopIteration.  `Option<PyValue>`
    /// collapses both into Rust None, which breaks the default=None case.
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
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyStr` for
    /// `name` enforces CPython's requirement that the attribute name be a
    /// string; `PyValue` for `obj` accepts any Python object.
    fn delattr(
        #[positional_only] obj: PyValue,
        #[positional_only] name: PyStr,
    ) -> Result<Value> {
        // Delegate to the canonical delete_attr path so that every value
        // kind (BuiltinFunction, UserFunction, BoundMethod, PyClass, …)
        // raises the correct error type and message instead of the old
        // catch-all "delattr() object has no writable attributes".
        _interp.delete_attr(obj.0, &*name)?;
        Ok(Value::none())
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
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::mapping_proxy::TYPE_NAME =>
                {
                    if let Some(class_rc) =
                        pyrust_builtins::mapping_proxy::as_class_rc(&args[2].value)
                    {
                        let class = class_rc.borrow();
                        for (k, v) in class.attrs.iter() {
                            attrs.insert(k.clone(), v.clone());
                        }
                    }
                }
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument 3 must be a dict"),
                )),
            }
            return Ok(Value::py_class(Rc::new(RefCell::new(PyClass {
                qualname: name.clone(),
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
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyStr` for
    /// `name` enforces CPython's requirement that the attribute name be a
    /// string; `PyValue` for `obj` and `value` accept any Python object.
    fn setattr(
        #[positional_only] obj: PyValue,
        #[positional_only] name: PyStr,
        #[positional_only] value: PyValue,
    ) -> Result<Value> {
        _interp.assign_attr(obj.0, &*name, value.0)?;
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
                Ok(pyrust_builtins::mapping_proxy::mapping_proxy(Rc::clone(class)))
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

    /// CPython: globals() — the live module namespace dict (issue #706).
    /// <https://docs.python.org/3/library/functions.html#globals>
    ///
    /// Returns `Interpreter::module_globals_dict`, a persistent `Value::dict`.
    /// On the first call (globals_accessed was false), syncs all current
    /// module env values into the dict so the snapshot is complete; sets
    /// `globals_accessed = true` so subsequent `assign_name` calls keep the
    /// dict live.  `globals() is globals()` is always `True` (same Rc).
    fn globals(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        sync_module_env_to_globals_dict(_interp);
        Ok(_interp.module_globals_dict.clone())
    }

    /// CPython: locals() — dict snapshot of the current local namespace.
    /// <https://docs.python.org/3/library/functions.html#locals>
    ///
    /// At module scope, `locals()` returns the same live dict as `globals()`
    /// (CPython parity: at module level the two namespaces are the same object).
    /// Inside a function body it returns a snapshot of the function's locals —
    /// CPython also snapshots and its docs warn that mutations to the returned
    /// dict aren't guaranteed to propagate.
    fn locals(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes no arguments ({} given)", args.len()),
            ));
        }
        // At module scope (the innermost frame is the Script frame, or there are
        // no function frames), return the persistent module_globals_dict — the
        // same object as `globals()` (CPython parity: locals() is globals() at
        // module level).
        let is_module_scope = _interp
            .vm_frame_views
            .last()
            .map(|v| v.kind == crate::interpreter::FrameKind::Script)
            .unwrap_or(true);
        if is_module_scope {
            sync_module_env_to_globals_dict(_interp);
            return Ok(_interp.module_globals_dict.clone());
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
    /// Not marked `#[pure]` because it dispatches user `__lt__` (and related
    /// comparison dunders) when sorting, and may invoke the user-supplied key
    /// function which can execute arbitrary user code.
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
            // Pre-scan: if no key is a PyInstance, all comparisons are
            // primitive — skip the interpreter-dispatch overhead entirely.
            let has_instance =
                keyed.iter().any(|(k, _)| matches!(k.kind(), ValueKind::PyInstance(_)));
            let mut sort_err: Option<PyError> = None;
            if has_instance {
                keyed.sort_by(|(a, _), (b, _)| {
                    if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                    match _interp.richcmp_order(a, b) {
                        Ok(ord) => ord,
                        Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                    }
                });
            } else {
                keyed.sort_by(|(a, _), (b, _)| {
                    if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                    match compare_values(a, b) {
                        Ok(ord) => ord,
                        Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                    }
                });
            }
            if let Some(e) = sort_err { return Err(e); }
            items = keyed.into_iter().map(|(_, v)| v).collect();
        } else {
            // Pre-scan: if no item is a PyInstance, use compare_values
            // directly — zero interpreter-dispatch overhead for the common
            // all-primitive case (ints, strings, …).
            let has_instance =
                items.iter().any(|v| matches!(v.kind(), ValueKind::PyInstance(_)));
            let mut sort_err: Option<PyError> = None;
            if has_instance {
                items.sort_by(|a, b| {
                    if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                    match _interp.richcmp_order(a, b) {
                        Ok(ord) => ord,
                        Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                    }
                });
            } else {
                items.sort_by(|a, b| {
                    if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                    match compare_values(a, b) {
                        Ok(ord) => ord,
                        Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                    }
                });
            }
            if let Some(e) = sort_err { return Err(e); }
        }
        if reverse {
            items.reverse();
        }
        Ok(Value::list(items))
    }

    /// CPython: min(iterable, /, *, key=None) or min(*args, key=None).
    /// <https://docs.python.org/3/library/functions.html#min>
    /// Not marked `#[pure]` — dispatches user `__lt__` (and related
    /// comparison dunders) when comparing elements, and may invoke the
    /// user-supplied key function.
    fn min(args) -> Result<Value> {
        min_max_impl(_interp, args, false, FN_NAME)
    }

    /// CPython: max(iterable, /, *, key=None) or max(*args, key=None).
    /// <https://docs.python.org/3/library/functions.html#max>
    /// Not marked `#[pure]` — dispatches user `__gt__` (with `__lt__` as
    /// reflected fallback) when comparing elements, and may invoke the
    /// user-supplied key function.
    fn max(args) -> Result<Value> {
        min_max_impl(_interp, args, true, FN_NAME)
    }

    /// CPython: round(number[, ndigits]) — banker's rounding.
    /// <https://docs.python.org/3/library/functions.html#round>
    ///
    /// Migrated to the typed-signature dialect (#400).  A single
    /// `#[positional_only]` signature is used (not an overload set) so
    /// that `#[default(None)]` on `ndigits` is legal — the macro forbids
    /// defaults in overload sets.  `PyValue` is used for both `x` and
    /// `ndigits` so the body can dispatch on `ValueKind` for full CPython
    /// parity: `bool ⊆ int` (both round unchanged), `float` uses
    /// half-even rounding, and everything else raises `TypeError`.
    #[pure]
    fn round(
        #[positional_only] x: PyValue,
        #[positional_only]
        #[default(None)]
        ndigits: Option<PyValue>,
    ) -> Result<Value> {
        let ndigits_i32: Option<i32> = match ndigits {
            None => None,
            Some(ref v) => match v.0.kind() {
                ValueKind::Int(n) => Some(n as i32),
                ValueKind::Bool(b) => Some(b as i32),
                ValueKind::None => None,
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() ndigits must be an integer or None"),
                )),
            },
        };
        // Extract in a scoped block to avoid holding the kind() borrow
        // across any Rc-cloning below.
        enum NumKind { Int(i64), Bool(bool), BigInt, Float(f64), Other }
        let num = match x.0.kind() {
            ValueKind::Int(v) => NumKind::Int(v),
            ValueKind::Bool(b) => NumKind::Bool(b),
            ValueKind::BigInt(_) => NumKind::BigInt,
            ValueKind::Float(v) => NumKind::Float(v),
            _ => NumKind::Other,
        };
        match num {
            NumKind::Int(v) => Ok(Value::int(v)),
            NumKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
            NumKind::BigInt => {
                // BigInt: return unchanged (any ndigits; int arithmetic exact).
                if let ValueKind::BigInt(b) = x.0.kind() {
                    Ok(Value::bigint(b.clone()))
                } else {
                    unreachable!()
                }
            }
            NumKind::Float(v) => match ndigits_i32 {
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
            },
            NumKind::Other => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() argument must be a number"),
            )),
        }
    }

    /// CPython: list([iterable]) — list constructor.
    /// <https://docs.python.org/3/library/functions.html#list>
    ///
    /// Not migrated to the typed-signature dialect in this batch: the
    /// macro's overload set requires all overloads to share the same arity,
    /// so the 0-arg / 1-arg split can't be expressed as two typed overloads.
    /// `Option<PyValue>` would conflate "absent" with "Python None",
    /// turning `list(None)` into `[]` instead of the correct `TypeError`.
    /// Remaining as `(args)` until the macro supports variable-arity
    /// overloads (tracked under #400).
    ///
    /// Not marked `#[pure]` because it dispatches user `__iter__` and
    /// `__next__` when consuming a user-defined iterable.
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
    ///
    /// Not migrated in this batch — same arity-split constraint as `list`.
    ///
    /// Not marked `#[pure]` because it dispatches user `__iter__` and
    /// `__next__` when consuming a user-defined iterable.
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
    /// Not marked `#[pure]` because the iterable fallback dispatches user
    /// `__iter__` and `__next__` when consuming a general iterable (e.g. range,
    /// generator expressions, user-defined iterables).
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
                ValueKind::Bool(b) => {
                    // bool is a subclass of int; True == 1, False == 0
                    Ok(Value::bytes(vec![0u8; b as usize]))
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
                            ValueKind::Int(_) | ValueKind::BigInt(_) => {
                                return Err(PyError::named(
                                    "ValueError",
                                    "bytes must be in range(0, 256)".to_string(),
                                ))
                            }
                            ValueKind::Bool(b) => out.push(b as u8),
                            _ => {
                                return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "'{}' object cannot be interpreted as an integer",
                                        pyrust_core::builtin_type_name(v),
                                    ),
                                ))
                            }
                        }
                    }
                    Ok(Value::bytes(out))
                }
                ValueKind::Tuple(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for v in items.iter() {
                        match v.kind() {
                            ValueKind::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
                            ValueKind::Int(_) | ValueKind::BigInt(_) => {
                                return Err(PyError::named(
                                    "ValueError",
                                    "bytes must be in range(0, 256)".to_string(),
                                ))
                            }
                            ValueKind::Bool(b) => out.push(b as u8),
                            _ => {
                                return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "'{}' object cannot be interpreted as an integer",
                                        pyrust_core::builtin_type_name(v),
                                    ),
                                ))
                            }
                        }
                    }
                    Ok(Value::bytes(out))
                }
                ValueKind::BigInt(_) => Err(PyError::named(
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer".to_string(),
                )),
                _ => {
                    // General iterable fallback: any object supporting __iter__ /
                    // __next__ (range, generators, user-defined iterables, etc.).
                    // Non-iterable arguments produce CPython-compatible
                    // "cannot convert 'X' object to bytes".
                    let type_name = pyrust_core::builtin_type_name(&args[0].value).into_owned();
                    let items =
                        _interp.collect_iterable(args[0].value.clone()).map_err(|e| {
                            if e.class_name_is("TypeError") {
                                PyError::named(
                                    "TypeError",
                                    format!("cannot convert '{type_name}' object to bytes"),
                                )
                            } else {
                                e
                            }
                        })?;
                    let mut out = Vec::with_capacity(items.len());
                    for v in &items {
                        match v.kind() {
                            ValueKind::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
                            ValueKind::Int(_) | ValueKind::BigInt(_) => {
                                return Err(PyError::named(
                                    "ValueError",
                                    "bytes must be in range(0, 256)".to_string(),
                                ))
                            }
                            _ => {
                                return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "'{}' object cannot be interpreted as an integer",
                                        pyrust_core::builtin_type_name(v),
                                    ),
                                ))
                            }
                        }
                    }
                    Ok(Value::bytes(out))
                }
                #[allow(unreachable_patterns)]
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
                // CPython checks the encoding argument before the source
                // argument: if encoding is not a str, report the type; only
                // once encoding is confirmed to be a str do we check whether
                // source is also a str (and give "encoding without a string
                // argument" if not).
                let encoding: String = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    // CPython 3.12 formats the type name of the encoding
                    // argument as "None" (not "NoneType") for the None
                    // singleton — matching the singleton's display name rather
                    // than its class name.  All other types use the class name.
                    ValueKind::None => return Err(PyError::named(
                        "TypeError",
                        "bytes() argument 'encoding' must be str, not None".to_string(),
                    )),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "bytes() argument 'encoding' must be str, not {}",
                            value_type_name_str(&args[1].value),
                        ),
                    )),
                };
                let source: String = match args[0].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::named(
                        "TypeError",
                        "encoding without a string argument".to_string(),
                    )),
                };
                let errors: String = if args.len() == 3 {
                    match args[2].value.kind() {
                        ValueKind::Str(s) => s.to_string(),
                        // Same None special-case as encoding above.
                        ValueKind::None => return Err(PyError::named(
                            "TypeError",
                            "bytes() argument 'errors' must be str, not None".to_string(),
                        )),
                        _ => return Err(PyError::named(
                            "TypeError",
                            format!(
                                "bytes() argument 'errors' must be str, not {}",
                                value_type_name_str(&args[2].value),
                            ),
                        )),
                    }
                } else {
                    "strict".to_string()
                };
                encode_str_to_bytes(&source, &encoding, &errors)
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "bytes() takes at most 3 arguments ({} given)",
                    args.len()
                ),
            )),
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
                ValueKind::BigInt(b) => {
                    let f = b.to_f64().unwrap_or(f64::INFINITY);
                    if f.is_finite() {
                        Ok(f)
                    } else {
                        Err(PyError::named(
                            "OverflowError",
                            "int too large to convert to float",
                        ))
                    }
                }
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
                ValueKind::Str(s) => {
                    let (re, im) = parse_complex_str(&s).ok_or_else(|| {
                        PyError::named("ValueError", "complex() arg is a malformed string")
                    })?;
                    Ok(Value::complex(re, im))
                }
                _ => Ok(Value::complex(to_f64(&args[0].value, "real")?, 0.0)),
            },
            2 => {
                if matches!(args[0].value.kind(), ValueKind::Str(_)) {
                    return Err(PyError::named(
                        "TypeError",
                        "complex() can't take second arg if first is a string",
                    ));
                }
                if matches!(args[1].value.kind(), ValueKind::Str(_)) {
                    return Err(PyError::named(
                        "TypeError",
                        "complex() second arg can't be a string",
                    ));
                }
                // CPython decomposition formula (Objects/complexobject.c):
                // When at least one arg is complex, apply:
                //   result.real = cr - di
                //   result.imag = ci + dr
                // where cr/ci are the real/imag parts of the first arg,
                // and dr/di are the real/imag parts of the second arg.
                // When neither arg is complex, assign real and imag directly
                // (preserving -0.0 sign, which the formula would lose via
                // 0.0 + (-0.0) = 0.0 in IEEE 754).
                let real_is_complex = matches!(args[0].value.kind(), ValueKind::Complex(_, _));
                let imag_is_complex = matches!(args[1].value.kind(), ValueKind::Complex(_, _));
                if real_is_complex || imag_is_complex {
                    let (cr, ci) = match args[0].value.kind() {
                        ValueKind::Complex(re, im) => (re, im),
                        _ => (to_f64(&args[0].value, "real")?, 0.0),
                    };
                    let (dr, di) = match args[1].value.kind() {
                        ValueKind::Complex(re, im) => (re, im),
                        _ => (to_f64(&args[1].value, "imag")?, 0.0),
                    };
                    Ok(Value::complex(cr - di, ci + dr))
                } else {
                    let re = to_f64(&args[0].value, "real")?;
                    let im = to_f64(&args[1].value, "imag")?;
                    Ok(Value::complex(re, im))
                }
            }
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most 2 arguments"))),
        }
    }

    /// CPython: set([iterable]) — set constructor.
    /// <https://docs.python.org/3/library/functions.html#func-set>
    ///
    /// Not migrated in this batch — same arity-split constraint as `list`.
    ///
    /// Not marked `#[pure]` because it dispatches user `__hash__` via
    /// `value_to_pykey` when building the set, and `__eq__` via `set_insert`.
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
    ///
    /// Not migrated in this batch — same arity-split constraint as `list`.
    ///
    /// Not marked `#[pure]` because it dispatches user `__hash__` via
    /// `value_to_pykey` when building the set, and `__eq__` via `set_insert`.
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
    /// Not marked `#[pure]` because it dispatches user `__str__` and
    /// (as fallback) `__repr__` on user-defined objects.
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
                ValueKind::BigInt(b) => b
                    .to_f64()
                    .filter(|f| f.is_finite())
                    .map(Value::float)
                    .ok_or_else(|| {
                        PyError::named(
                            "OverflowError",
                            "int too large to convert to float".to_string(),
                        )
                    }),
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
    ///
    /// Not marked `#[pure]` because it dispatches user `__bool__` and
    /// (as fallback) `__len__` on user-defined objects via `truthy_value`.
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
            return Ok(Value::dict(indexmap::IndexMap::new()));
        }
        if args.len() == 1 {
            match args[0].value.kind() {
                ValueKind::Dict(map) => {
                    return Ok(Value::dict(map.clone()));
                }
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::mapping_proxy::TYPE_NAME =>
                {
                    if let Some(class_rc) =
                        pyrust_builtins::mapping_proxy::as_class_rc(&args[0].value)
                    {
                        let class = class_rc.borrow();
                        let mut d: indexmap::IndexMap<PyKey, Value> =
                            indexmap::IndexMap::new();
                        for (k, v) in class.attrs.iter() {
                            d.insert(PyKey::Str(k.clone()), v.clone());
                        }
                        return Ok(Value::dict(d));
                    }
                }
                _ => {}
            }
        }
        Err(PyError::named(
            "TypeError",
            format!("{FN_NAME}() with arguments is not yet supported"),
        ))
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
        let (cls_val, inst_val) = if args.is_empty() {
            // Zero-argument super() — resolve __class__ cell and first param.
            resolve_zero_arg_super(_interp)?
        } else if args.len() == 2 {
            (args[0].value.clone(), args[1].value.clone())
        } else {
            return Err(PyError::Runtime(format!(
                "{FN_NAME}() takes at most 2 arguments ({} given)",
                args.len()
            )));
        };
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
            // Builtin bound methods (e.g. `"".upper`, `[].append`) are
            // callable.  Accessor partials (intermediate results of
            // prop.setter / prop.getter / prop.deleter) are callable too —
            // but a plain property descriptor isn't.
            ValueKind::BuiltinObject { .. } => {
                pyrust_builtins::bound_method::is_bound_method(value)
                    || pyrust_builtins::property::property_partial_slot(value)
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

    /// CPython: slice(stop) / slice(start, stop[, step]) — construct a slice
    /// object.  Used as both a callable constructor and an `isinstance` target.
    /// <https://docs.python.org/3/library/functions.html#slice>
    fn slice(args) -> Result<Value> {
        // CPython 3.12: slice() is positional-only; any keyword argument
        // raises TypeError with the message "slice() takes no keyword
        // arguments" regardless of which keyword was supplied (issue #848).
        if args.iter().any(|a| a.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                "slice() takes no keyword arguments".to_string(),
            ));
        }
        let (start, stop, step) = match args.len() {
            0 => {
                return Err(PyError::named(
                    "TypeError",
                    "slice expected at least 1 argument, got 0".to_string(),
                ));
            }
            1 => (None, Some(args[0].value.clone()), None),
            2 => (Some(args[0].value.clone()), Some(args[1].value.clone()), None),
            3 => (
                Some(args[0].value.clone()),
                Some(args[1].value.clone()),
                if args[2].value.is_none() { None } else { Some(args[2].value.clone()) },
            ),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("slice expected at most 3 arguments, got {}", args.len()),
                ));
            }
        };
        Ok(pyrust_builtins::slice::make_slice(start, stop, step))
    }
}

/// Integer divmod shared by all `int`/`bool` overload combinations.
///
/// When either operand is `Big` (heap-stored BigInt whose magnitude exceeds
/// `i64::MAX`) we promote both to `BigInt` for exact arithmetic.  The small-int
/// fast path avoids any heap allocation for the common case.
fn divmod_int_int(
    a: crate::interpreter::builtin_args::PyInt<'_>,
    b: crate::interpreter::builtin_args::PyInt<'_>,
) -> crate::error::Result<Value> {
    // Fast path: both fit in i64.
    if let (Some(a), Some(b)) = (a.as_i64(), b.as_i64()) {
        if b == 0 {
            return Err(PyError::named(
                "ZeroDivisionError",
                "integer division or modulo by zero".to_string(),
            ));
        }
        let modulo = py_mod_i64(a, b);
        let quotient = (a - modulo) / b;
        return Ok(Value::tuple(vec![Value::int(quotient), Value::int(modulo)]));
    }
    // BigInt path: at least one operand doesn't fit in i64.
    let a_big = a.to_bigint();
    let b_big = b.to_bigint();
    if b_big.is_zero() {
        return Err(PyError::named(
            "ZeroDivisionError",
            "integer division or modulo by zero".to_string(),
        ));
    }
    let (q, r) = bigint_divmod_floor(&a_big, &b_big);
    Ok(Value::tuple(vec![Value::bigint(q), Value::bigint(r)]))
}

/// Convert a `PyInt` to `f64` for mixed int/float arithmetic.
///
/// - Small ints: lossless cast (for values beyond 2^53 the cast loses
///   precision but CPython behaves the same way).
/// - BigInt that fits in f64 (e.g. 2^100): uses `BigInt::to_f64()`.
/// - BigInt too large for f64 (e.g. 2^2000): raises `OverflowError`,
///   matching CPython's `float(2**2000)`.
fn pyint_to_f64(v: &crate::interpreter::builtin_args::PyInt<'_>) -> crate::error::Result<f64> {
    match v.as_i64() {
        Some(n) => Ok(n as f64),
        None => {
            // Must be a genuine BigInt (is_big() would be true here).
            let b = v.to_bigint();
            use crate::value::PyToPrimitive;
            match b.to_f64() {
                Some(f) if f.is_finite() => Ok(f),
                _ => Err(PyError::named(
                    "OverflowError",
                    "int too large to convert to float".to_string(),
                )),
            }
        }
    }
}

/// Float divmod shared by all `float`-family overload combinations.
fn divmod_float_float(a: f64, b: f64) -> crate::error::Result<Value> {
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

/// Parse a string argument to `complex()`, mirroring CPython 3.12 semantics.
///
/// Accepts the forms:
///   - real-only:      `"1"`, `"1.5"`, `"inf"`, `"-nan"`, `"+1e2"`, …
///   - imaginary-only: `"3j"`, `"-j"`, `"+j"`, `"infj"`, `"nanj"`, …
///   - combined:       `"1+2j"`, `"-1-2j"`, `"1.5e+2-3j"`, `"nan+nanj"`, …
///   - parenthesized:  `"(1+2j)"` (CPython also accepts this form)
///
/// Leading/trailing whitespace (and spaces inside parentheses) is stripped.
/// Internal whitespace (e.g. `"1 + 2j"`) is rejected (returns `None`).
/// Returns `None` for any malformed input so the caller can raise `ValueError`.
fn parse_complex_str(s: &str) -> Option<(f64, f64)> {
    let s = s.trim();

    // Parenthesized form: "(1+2j)" — strip parens and recurse once.
    let s = if s.starts_with('(') && s.ends_with(')') {
        s[1..s.len() - 1].trim()
    } else {
        s
    };

    if s.is_empty() {
        return None;
    }

    // Reject any internal whitespace.
    if s.bytes().any(|b| b == b' ' || b == b'\t') {
        return None;
    }

    // Parse a float token that may be "inf", "nan", "+inf", "-nan", digits, etc.
    // Returns None if the token is empty or malformed.
    let parse_float = |tok: &str| -> Option<f64> {
        if tok.is_empty() {
            return None;
        }
        tok.parse::<f64>().ok()
    };

    // Handle the bare-j shorthand: "+j" and "-j" used as imag-part coefficient.
    let parse_float_or_bare_j = |tok: &str| -> Option<f64> {
        match tok {
            "+j" => Some(1.0),
            "-j" => Some(-1.0),
            _ => parse_float(tok),
        }
    };

    if s.ends_with('j') || s.ends_with('J') {
        // Has an imaginary part.  Strip the trailing 'j'/'J'.
        let body = &s[..s.len() - 1];

        // Find the last '+' or '-' that is NOT at position 0 and NOT
        // immediately after an 'e'/'E' (scientific-notation exponent sign).
        // Scan right-to-left; take the first qualifying split point.
        let split_pos = body
            .char_indices()
            .rev()
            .find(|&(i, c)| {
                if i == 0 {
                    return false; // leading sign belongs to the number
                }
                if c != '+' && c != '-' {
                    return false;
                }
                // Exclude exponent signs: the preceding char must not be 'e'/'E'.
                let prev = body[..i].chars().next_back().unwrap_or('\0');
                prev != 'e' && prev != 'E'
            })
            .map(|(i, _)| i);

        match split_pos {
            None => {
                // Pure imaginary: "3j", "infj", "+j", "-j", "j", etc.
                let im = match body {
                    "" | "+" => Some(1.0),
                    "-" => Some(-1.0),
                    _ => parse_float(body),
                }?;
                Some((0.0, im))
            }
            Some(i) => {
                // Combined: real part is body[..i], imag part is body[i..].
                let real_tok = &body[..i];
                let imag_tok = &body[i..]; // includes the leading sign

                let re = parse_float(real_tok)?;
                let im = parse_float_or_bare_j(imag_tok)?;
                Some((re, im))
            }
        }
    } else {
        // Real only.
        let re = parse_float(s)?;
        Some((re, 0.0))
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


// `int_hash` and `bigint_hash` were previously defined here.  They are now
// shared helpers (`py_hash_int` / `py_hash_bigint`) in
// `crate::interpreter::helpers` so that `value_to_pykey` (dict/set key
// storage) and the `hash()` builtin both apply identical Mersenne-prime
// reduction and the `-1 → -2` sentinel remap (issue #503).
// Local aliases preserve the call sites below unchanged.
#[inline(always)]
fn int_hash(v: i64) -> i64 { py_hash_int(v) }
#[inline(always)]
fn bigint_hash(n: &crate::value::PyBigInt) -> i64 { py_hash_bigint(n) }

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
        // ValueKind::Int arrives here for values in [-2^47, 2^47-1] (inline i48)
        // *and* for Opaque::PyBigInt values that happen to fit in i64 (the `kind()`
        // accessor promotes them).  Both need the full Mersenne reduction so that
        // e.g. hash(2**62) and hash(-1) match CPython.
        ValueKind::Int(v) => Ok(int_hash(v)),
        // bool: True==1, False==0 — both well within (-M, M), so int_hash is a
        // no-op for the reduction, but the -1→-2 remap can never fire here either.
        ValueKind::Bool(b) => Ok(b as i64),
        // BigInt arrives only when the value doesn't fit in i64 (|n| > i64::MAX).
        ValueKind::BigInt(n) => Ok(bigint_hash(n)),
        ValueKind::Float(v) => Ok(py_hash_float(v)),
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
        // BuiltinObject: probe the BuiltinTypeOps hash hook (added in PR #781).
        // Types that override BuiltinTypeOps::hash (e.g. frozenset) return
        // Some(u64); anything that leaves it at the default None is unhashable.
        ValueKind::BuiltinObject { ops, state } => match ops.hash(state) {
            Some(h) => Ok(h as i64),
            None => Err(PyError::named(
                "TypeError",
                format!("unhashable type: '{}'", ops.type_name()),
            )),
        },
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
        // Built-in objects (GenericAlias, frozenset, …) opt in to hashing via
        // `BuiltinTypeOps::hash`.  Return `None` → TypeError.
        ValueKind::BuiltinObject { ops, state } => {
            match ops.hash(state) {
                Some(h) => Ok(h as i64),
                None => Err(PyError::named(
                    "TypeError",
                    format!("unhashable type: '{}'", ops.type_name()),
                )),
            }
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

/// Interpreter-aware hash that dispatches `__hash__` for `PyInstance` values
/// and handles `Tuple` elements by recursing with interpreter access.
///
/// This is the entry point used by the `hash` builtin.  `hash_value` (above)
/// remains a pure helper for primitive leaf types; this function calls it for
/// those cases to avoid duplicating their logic.
///
/// `Tuple`: uses the same initial value and multiply-add mixing as
/// `hash_value`'s Tuple arm (`h = h.wrapping_mul(1000003).wrapping_add(elem)`),
/// but each element is hashed via this function rather than `hash_value`, so
/// `PyInstance` elements dispatch `__hash__` correctly (issue #502).
///
/// Returns `true` if `v` (or any value recursively nested inside it) is a
/// `PyInstance` that requires interpreter access for `__hash__` dispatch.
///
/// Recurses into `Tuple` elements and `slice` components so that a
/// `PyInstance` hidden inside `(inst, 1)` or `slice((inst, 1), 2)` is
/// correctly detected and routed through the interpreter hashing path.
pub(crate) fn value_needs_interp(v: &Value) -> bool {
    match v.kind() {
        ValueKind::PyInstance(_) => true,
        ValueKind::Tuple(items) => items.iter().any(value_needs_interp),
        ValueKind::BuiltinObject { ops, state }
            if ops.type_name() == pyrust_builtins::slice::TYPE_NAME =>
        {
            let borrow = state.borrow();
            if let Some(s) = borrow.downcast_ref::<pyrust_builtins::slice::SliceState>() {
                value_needs_interp(&s.start)
                    || value_needs_interp(&s.stop)
                    || value_needs_interp(&s.step)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn tuple_needs_interp(items: &[Value]) -> bool {
    items.iter().any(value_needs_interp)
}

/// Returns `true` when hashing `v` requires the slow per-element path of
/// `hash_value_with_interp` rather than the pure `hash_value` shortcut.
///
/// Two cases force the slow path:
/// 1. A `PyInstance` anywhere in the tree (needs interpreter `__hash__` dispatch).
/// 2. A `slice` whose components contain an unhashable primitive (list/dict/set/…)
///    at any nesting depth — the pure `hash_value` path would blame `'slice'` via
///    `SliceOps::hash` returning `None`, but the slow path properly names the leaf
///    unhashable type (issue #893).
fn value_needs_slow_hash(v: &Value) -> bool {
    if value_needs_interp(v) {
        return true;
    }
    // Check for slice with unhashable primitive components (no PyInstance involved,
    // so value_needs_interp returned false, but hash_value would still produce the
    // wrong error message).
    if let ValueKind::BuiltinObject { ops, state } = v.kind() {
        if ops.type_name() == pyrust_builtins::slice::TYPE_NAME {
            let borrow = state.borrow();
            if let Some(s) = borrow.downcast_ref::<pyrust_builtins::slice::SliceState>() {
                return s.start.to_key().is_none()
                    || s.stop.to_key().is_none()
                    || s.step.to_key().is_none();
            }
        }
    }
    // Recurse into tuple elements.
    if let ValueKind::Tuple(items) = v.kind() {
        return items.iter().any(value_needs_slow_hash);
    }
    false
}

pub(crate) fn hash_value_with_interp(interp: &mut crate::Interpreter, value: &Value) -> Result<i64> {
    match value.kind() {
        ValueKind::Tuple(items) => {
            // Fast path: if no element at any depth requires the slow path
            // (PyInstance needing __hash__ dispatch, or a slice with unhashable
            // primitive components that hash_value would misreport as 'slice'),
            // delegate to the pure hash_value helper — no Vec allocation needed.
            if !items.iter().any(value_needs_slow_hash) {
                return hash_value(value);
            }
            // At least one element needs interpreter access (PyInstance or a
            // nested tuple that may contain one).  Clone the slice to release
            // the borrow of `value` before the mutable `interp` calls.
            let items: Vec<Value> = items.to_vec();
            let mut h: i64 = 3527539;
            for item in &items {
                let item_hash = hash_value_with_interp(interp, item)?;
                h = h.wrapping_mul(1000003).wrapping_add(item_hash);
            }
            Ok(h)
        }
        // Slices: CPython 3.12 makes slice hashable when all components are
        // hashable.  `pyrust-builtins::component_hash` uses `Value::to_key()`
        // which returns `None` for `PyInstance` (no interpreter access), so
        // `hash(slice(inst, 2))` fails even though CPython 3.12 succeeds via
        // identity hash.  Handle slices here with full interpreter dispatch so
        // that each component can dispatch `__hash__` and produce the precise
        // per-component "unhashable type: 'X'" message (issue #850).
        ValueKind::BuiltinObject { ops, state }
            if ops.type_name() == pyrust_builtins::slice::TYPE_NAME =>
        {
            let borrow = state.borrow();
            let s = borrow
                .downcast_ref::<pyrust_builtins::slice::SliceState>()
                .expect("SliceOps: bad state");
            let needs_interp = value_needs_interp(&s.start)
                || value_needs_interp(&s.stop)
                || value_needs_interp(&s.step);
            if !needs_interp {
                // Check for unhashable primitive components (e.g. list, dict,
                // set inside the slice).  hash_value's BuiltinObject arm would
                // blame 'slice'; instead name the actual offending leaf type
                // (issue #893).
                let unhashable = [&s.start, &s.stop, &s.step].iter().find_map(|c| {
                    if c.to_key().is_none() {
                        Some(pyrust_builtins::set::leaf_unhashable_type_name(c))
                    } else {
                        None
                    }
                });
                drop(borrow);
                if let Some(bad_type) = unhashable {
                    return Err(PyError::named(
                        "TypeError",
                        format!("unhashable type: '{bad_type}'"),
                    ));
                }
                // All components hashable: delegate to the pure path (SliceOps::hash
                // via hash_value) to keep dict-key hashes consistent with the
                // SliceOps::to_key path used in value_to_pykey.
                return hash_value(value);
            }
            // Clone components to release the borrow before mutable interp calls.
            let (start, stop, step) = (s.start.clone(), s.stop.clone(), s.step.clone());
            drop(borrow);
            let hstart = hash_value_with_interp(interp, &start)?;
            let hstop = hash_value_with_interp(interp, &stop)?;
            let hstep = hash_value_with_interp(interp, &step)?;
            // Mix the three component hashes using the same multiply-add
            // accumulator as the Tuple arm.
            let mut h: i64 = 3527539;
            for &c in &[hstart, hstop, hstep] {
                h = h.wrapping_mul(1000003).wrapping_add(c);
            }
            Ok(h)
        }
        ValueKind::PyInstance(inst) => {
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
                    interp,
                    hash_method,
                    Value::py_instance(inst_rc),
                    &[],
                )?;
                // CPython's slot_tp_hash semantics (issue #503):
                // - Int: apply only the `-1 → -2` sentinel remap.
                // - BigInt: apply Mersenne-prime reduction (long_hash).
                let hash_val: i64 = match result.kind() {
                    ValueKind::Int(n) => {
                        if n == -1 { -2 } else { n }
                    }
                    ValueKind::Bool(b) => b as i64,
                    ValueKind::BigInt(n) => bigint_hash(n),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "__hash__ method should return an integer".to_string(),
                        ));
                    }
                };
                return Ok(hash_val);
            }
            // No __hash__ at all: identity hash (CPython's default
            // object.__hash__), with the -1 → -2 sentinel remap.
            let ptr = Rc::as_ptr(&inst_rc) as i64;
            Ok(if ptr == -1 { -2 } else { ptr })
        }
        // All other types are primitives; delegate to the pure hash_value helper.
        _ => hash_value(value),
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
        // Fast path: `object` is the universal base — every Python value
        // is an instance of `object`.  Check before the primitive-class
        // dispatch so that `isinstance(None, object)`,
        // `isinstance(print, object)`, etc. all return `True`.
        if Rc::ptr_eq(expected, &crate::interpreter::object_class_singleton()) {
            return true;
        }
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
        // Pre-scan: if no key is a PyInstance, all comparisons are primitive —
        // use compare_values_with_op directly to avoid interpreter-dispatch
        // overhead while still emitting the correct operator token.
        let has_instance =
            keyed.iter().any(|(k, _)| matches!(k.kind(), ValueKind::PyInstance(_)));
        let mut result_err: Option<PyError> = None;
        let result = keyed.into_iter().reduce(|acc, item| {
            if result_err.is_some() { return acc; }
            let cmp = if has_instance {
                // max uses richcmp_order_gt (tries __gt__ first, '>' error on
                // miss); min uses richcmp_order (__lt__ first, '<' error on
                // miss). Matches CPython's Py_GT / Py_LT reduction paths.
                if is_max {
                    interp.richcmp_order_gt(&item.0, &acc.0)
                } else {
                    interp.richcmp_order(&item.0, &acc.0)
                }
            } else {
                // CPython max() emits '>' in its TypeError; min() emits '<'.
                compare_values_with_op(&item.0, &acc.0, if is_max { ">" } else { "<" })
            };
            match cmp {
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
        // Pre-scan: if no item is a PyInstance, use compare_values_with_op
        // directly — zero interpreter-dispatch overhead for all-primitive
        // sequences, while emitting the correct operator token.
        let has_instance =
            items.iter().any(|v| matches!(v.kind(), ValueKind::PyInstance(_)));
        let mut result_err: Option<PyError> = None;
        let result = items.into_iter().reduce(|acc, v| {
            if result_err.is_some() { return acc; }
            let cmp = if has_instance {
                if is_max {
                    interp.richcmp_order_gt(&v, &acc)
                } else {
                    interp.richcmp_order(&v, &acc)
                }
            } else {
                // CPython max() emits '>' in its TypeError; min() emits '<'.
                compare_values_with_op(&v, &acc, if is_max { ">" } else { "<" })
            };
            match cmp {
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
    Ok(value.repr())
}
