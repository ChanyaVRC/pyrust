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
    dir_names, invoke_class_method, is_exception_class, iter_values, lookup_class_attr,
    modpow_i64, py_mod_i64, py_round_half_even, py_round_half_even_f64,
    reject_keyword_args_expanded, value_to_float, value_type_name_str,
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
    fn chr(#[positional_only] i: PyInt) -> Result<Value> {
        let code_point = i.expect_i64(FN_NAME, "i")?;
        chr_from_code_point(code_point)
    }

    fn chr(#[positional_only] i: PyBool) -> Result<Value> {
        // CPython: `chr(True) == '\x01'`, `chr(False) == '\x00'`.
        chr_from_code_point(if i.0 { 1 } else { 0 })
    }

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

    fn ord(#[positional_only] c: PyValue) -> Result<Value> {
        let _ = c;
        Err(PyError::named(
            "TypeError",
            format!("{FN_NAME}() expected string of length 1, but got non-string"),
        ))
    }

    /// CPython: bin(x) — integer to '0b…' / '-0b…' string.
    /// <https://docs.python.org/3/library/functions.html#bin>
    fn bin(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Int(v) => {
                if v < 0 {
                    // Widen to i128 first so `i64::MIN.abs()` doesn't overflow.
                    Ok(Value::string(format!("-0b{:b}", -(v as i128))))
                } else {
                    Ok(Value::string(format!("0b{:b}", v)))
                }
            }
            ValueKind::Bool(b) => Ok(Value::string(if b { "0b1".to_string() } else { "0b0".to_string() })),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    value_type_name_str(&args[0].value),
                ),
            )),
        }
    }

    /// CPython: oct(x) — integer to '0o…' / '-0o…' string.
    /// <https://docs.python.org/3/library/functions.html#oct>
    fn oct(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Int(v) => {
                if v < 0 {
                    Ok(Value::string(format!("-0o{:o}", -(v as i128))))
                } else {
                    Ok(Value::string(format!("0o{:o}", v)))
                }
            }
            ValueKind::Bool(b) => Ok(Value::string(if b { "0o1".to_string() } else { "0o0".to_string() })),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    value_type_name_str(&args[0].value),
                ),
            )),
        }
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
    fn hex(#[positional_only] x: PyInt) -> Result<Value> {
        let v = x.expect_i64(FN_NAME, "x")?;
        Ok(Value::string(format_hex_i64(v)))
    }

    fn hex(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `hex(True) == '0x1'`, `hex(False) == '0x0'`.
        Ok(Value::string(format_hex_i64(if x.0 { 1 } else { 0 })))
    }

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
    fn ascii(#[positional_only] obj: PyValue) -> Result<Value> {
        Ok(Value::string(ascii_repr(&obj.0)))
    }

    /// CPython: id(object) — identity (CPython returns memory address).
    /// <https://docs.python.org/3/library/functions.html#id>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` is the
    /// catch-all wrapper since `id` accepts every Python object; the
    /// existing per-kind dispatch becomes the body's only concern.
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

    fn abs(#[positional_only] x: PyFloat) -> Result<Value> {
        Ok(Value::float(x.0.abs()))
    }

    fn abs(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: abs(True) == 1, abs(False) == 0 — promoted to int.
        Ok(Value::int(if x.0 { 1 } else { 0 }))
    }

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
    fn sum(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() || args.len() > 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes 1 or 2 arguments")));
        }
        let items = iter_values(args[0].value.clone())?;
        let start = if args.len() == 2 { args[1].value.clone() } else { Value::int(0) };
        let mut acc = start;
        for item in items {
            acc = _interp.eval_binary(acc, BinaryOp::Add, item)?;
        }
        Ok(acc)
    }

    /// CPython: any(iterable) — true if any element is truthy.
    /// <https://docs.python.org/3/library/functions.html#any>
    fn any(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let items = iter_values(args[0].value.clone())?;
        for item in items {
            if item.truthy() {
                return Ok(Value::bool_(true));
            }
        }
        Ok(Value::bool_(false))
    }

    /// CPython: all(iterable) — true if every element is truthy (or empty).
    /// <https://docs.python.org/3/library/functions.html#all>
    fn all(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let items = iter_values(args[0].value.clone())?;
        for item in items {
            if !item.truthy() {
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
                return match result.kind() {
                    ValueKind::Str(_) => Ok(result),
                    _ => Err(PyError::named(
                        "TypeError",
                        "__repr__ returned non-string".to_string(),
                    )),
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
    fn hash(#[positional_only] obj: PyValue) -> Result<Value> {
        let value = obj.0;
        let hash_val = match value.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => b as i64,
            ValueKind::Float(v) => {
                if v.fract() == 0.0 && v.is_finite() { v as i64 }
                else { v.to_bits() as i64 }
            }
            ValueKind::Str(s) => {
                let mut h: u64 = 14695981039346656037u64;
                for b in s.bytes() {
                    h ^= b as u64;
                    h = h.wrapping_mul(1099511628211u64);
                }
                h as i64
            }
            ValueKind::None => 0,
            ValueKind::Tuple(items) => {
                let mut h: i64 = 3527539;
                for item in items {
                    let item_hash = match item.kind() {
                        ValueKind::Int(v) => v,
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::Float(fv) => {
                            if fv.fract() == 0.0 && fv.is_finite() { fv as i64 }
                            else { fv.to_bits() as i64 }
                        }
                        ValueKind::Str(s) => {
                            let mut sh: u64 = 14695981039346656037u64;
                            for byte in s.bytes() {
                                sh ^= byte as u64;
                                sh = sh.wrapping_mul(1099511628211u64);
                            }
                            sh as i64
                        }
                        ValueKind::None => 0,
                        _ => return Err(PyError::named(
                            "TypeError",
                            "unhashable type in tuple".to_string(),
                        )),
                    };
                    h = h.wrapping_mul(1000003).wrapping_add(item_hash);
                }
                h
            }
            ValueKind::List(_) => return Err(PyError::named(
                "TypeError",
                "unhashable type: 'list'".to_string(),
            )),
            ValueKind::Dict(_) => return Err(PyError::named(
                "TypeError",
                "unhashable type: 'dict'".to_string(),
            )),
            ValueKind::Set(_) => return Err(PyError::named(
                "TypeError",
                "unhashable type: 'set'".to_string(),
            )),
            _ => return Err(PyError::named(
                "TypeError",
                "unhashable type".to_string(),
            )),
        };
        Ok(Value::int(hash_val))
    }

    /// CPython: divmod(a, b) — `(a // b, a % b)`.
    /// <https://docs.python.org/3/library/functions.html#divmod>
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
                    Ok(Value::int(a.wrapping_pow(b as u32)))
                }
                (ValueKind::Bool(a), ValueKind::Int(b)) if b >= 0 => {
                    Ok(Value::int((a as i64).wrapping_pow(b as u32)))
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
        // Pass the source Value directly — `iter_helpers` materialises
        // lazily on first iter_next so side effects of e.g. `open()`
        // happen at iteration start, not at construction.
        Ok(pyrust_builtins::iter_helpers::enumerate(
            iterable,
            start,
        ))
    }

    /// CPython: zip(*iterables, strict=False) — parallel iterator.
    /// `strict=True` raises `ValueError` if lengths differ.
    /// <https://docs.python.org/3/library/functions.html#zip>
    fn zip(args) -> Result<Value> {
        // `strict` is the only accepted keyword arg; everything else is a
        // CPython-style `TypeError`.
        let mut strict = false;
        for a in args.iter() {
            if let Some(name) = a.name.as_deref() {
                if name == "strict" {
                    strict = a.value.truthy();
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
        Ok(pyrust_builtins::iter_helpers::zip(sources, strict))
    }

    /// CPython: reversed(seq) — reverse iterator.
    /// <https://docs.python.org/3/library/functions.html#reversed>
    fn reversed(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        Ok(pyrust_builtins::iter_helpers::reversed(args[0].value.clone()))
    }

    /// CPython: map(func, iterable) — apply func to each element.
    /// <https://docs.python.org/3/library/functions.html#map>
    fn map(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let func = args[0].value.clone();
        let items = iter_values(args[1].value.clone())?;
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
        let items = iter_values(args[1].value.clone())?;
        let use_identity = func.is_none();
        let mut result = Vec::new();
        for item in items {
            let keep = if use_identity {
                item.truthy()
            } else {
                let test = _interp.call_function_expanded(
                    func.clone(),
                    &[ExpandedCallArg { name: None, value: item.clone() }],
                )?;
                test.truthy()
            };
            if keep {
                result.push(item);
            }
        }
        Ok(Value::list(result))
    }

    /// CPython: iter(obj) — return an iterator over obj.
    /// <https://docs.python.org/3/library/functions.html#iter>
    fn iter(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let val = args[0].value.clone();
        match val.kind() {
            // Generators are their own iterators.
            ValueKind::Generator(_) => Ok(val),
            // User-defined objects: call __iter__().
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method_val) = lookup_class_attr(&class, "__iter__") {
                    invoke_class_method(
                        _interp,
                        method_val,
                        Value::py_instance(inst_rc),
                        &[],
                    )
                } else if lookup_class_attr(&class, "__next__").is_some() {
                    // Already an iterator (has __next__ but no separate __iter__).
                    Ok(val)
                } else {
                    Err(PyError::named(
                        "TypeError",
                        format!("'{}' object is not iterable", class.borrow().name),
                    ))
                }
            }
            // Built-in iterables: materialise into a NativeIterFrame so that
            // next() works on the returned value.
            _ => {
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
    fn issubclass(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let cls = match args[0].value.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() arg 1 must be a class"),
            )),
        };
        let result = match args[1].value.kind() {
            ValueKind::PyClass(expected) => class_is_subclass_of(&cls, expected),
            ValueKind::Tuple(items) => {
                let mut found = false;
                for item in items {
                    if let ValueKind::PyClass(expected) = item.kind()
                        && class_is_subclass_of(&cls, expected)
                    {
                        found = true;
                        break;
                    }
                }
                found
            }
            _ => return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() arg 2 must be a class or tuple of classes"),
            )),
        };
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
                if instance.borrow_mut().attrs.remove(&name).is_none() {
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
                if class.borrow_mut().attrs.remove(&name).is_none() {
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
    fn isinstance(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 2 arguments")));
        }
        let obj = &args[0].value;
        let cls = &args[1].value;
        let result = match (obj.kind(), cls.kind()) {
            (ValueKind::PyInstance(inst), ValueKind::PyClass(expected)) => {
                class_is_subclass_of(&inst.borrow().class, expected)
            }
            (ValueKind::Int(_) | ValueKind::Bool(_), ValueKind::BuiltinFunction("int")) => true,
            (ValueKind::Float(_), ValueKind::BuiltinFunction("float")) => true,
            (ValueKind::Str(_), ValueKind::BuiltinFunction("str")) => true,
            (ValueKind::Bool(_), ValueKind::BuiltinFunction("bool")) => true,
            (ValueKind::None, ValueKind::BuiltinFunction("NoneType")) => true,
            (ValueKind::List(_), ValueKind::BuiltinFunction("list")) => true,
            (ValueKind::Tuple(_), ValueKind::BuiltinFunction("tuple")) => true,
            (ValueKind::Set(_), ValueKind::BuiltinFunction("set")) => true,
            (ValueKind::BuiltinObject { ops, .. }, ValueKind::BuiltinFunction(name)) => {
                ops.type_name() == name
            }
            (ValueKind::Bytes(_), ValueKind::BuiltinFunction("bytes")) => true,
            (ValueKind::Complex(_, _), ValueKind::BuiltinFunction("complex")) => true,
            (ValueKind::Dict(_), ValueKind::BuiltinFunction("dict")) => true,
            _ => false,
        };
        Ok(Value::bool_(result))
    }

    /// CPython: type(object) → type / type(name, bases, namespace) → new class.
    /// <https://docs.python.org/3/library/functions.html#type>
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
            let base = match args[1].value.kind() {
                ValueKind::Tuple(items) | ValueKind::List(items) => {
                    if items.is_empty() {
                        None
                    } else {
                        // Multiple inheritance: PyRust supports only single base; take the first.
                        match items[0].kind() {
                            ValueKind::PyClass(c) => Some(Rc::clone(c)),
                            _ => return Err(PyError::named(
                                "TypeError",
                                format!("{FN_NAME}() argument 2 entries must be classes"),
                            )),
                        }
                    }
                }
                _ => return Err(PyError::named(
                    "TypeError",
                    format!("{FN_NAME}() argument 2 must be a tuple"),
                )),
            };
            let mut attrs: std::collections::HashMap<String, Value> =
                std::collections::HashMap::new();
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
        // `type(x) is type(x)` works via Rc::ptr_eq.  For builtin types
        // return a BuiltinFunction value (singleton-like) so
        // `type(5) is type(5)` works and isinstance(x, type(x)) succeeds.
        match obj.kind() {
            ValueKind::PyInstance(inst) => Ok(Value::py_class(Rc::clone(&inst.borrow().class))),
            ValueKind::PyClass(_) => Ok(Value::builtin_function("type")),
            ValueKind::Bool(_) => Ok(Value::builtin_function("bool")),
            ValueKind::Int(_) => Ok(Value::builtin_function("int")),
            ValueKind::Float(_) => Ok(Value::builtin_function("float")),
            ValueKind::Str(_) => Ok(Value::builtin_function("str")),
            ValueKind::None => Ok(Value::builtin_function("NoneType")),
            ValueKind::List(_) => Ok(Value::builtin_function("list")),
            ValueKind::Tuple(_) => Ok(Value::builtin_function("tuple")),
            ValueKind::Dict(_) => Ok(Value::builtin_function("dict")),
            ValueKind::Set(_) => Ok(Value::builtin_function("set")),
            ValueKind::Range { .. } => Ok(Value::builtin_function("range")),
            ValueKind::UserFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. } => Ok(Value::builtin_function("function")),
            ValueKind::BuiltinFunction(_) => Ok(Value::builtin_function("builtin_function_or_method")),
            ValueKind::PyModule(_) => Ok(Value::builtin_function("module")),
            ValueKind::BigInt(_) => Ok(Value::builtin_function("int")),
            ValueKind::SuperProxy { .. } | ValueKind::SuperProxyClass { .. } => Ok(Value::builtin_function("super")),
            ValueKind::Generator(_) => Ok(Value::builtin_function("generator")),
            ValueKind::NotImplemented => Ok(Value::builtin_function("NotImplementedType")),
            ValueKind::Bytes(_) => Ok(Value::builtin_function("bytes")),
            ValueKind::Complex(_, _) => Ok(Value::builtin_function("complex")),
            ValueKind::BuiltinObject { ops, .. } => {
                Ok(Value::builtin_function(ops.type_name()))
            }
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
            ValueKind::PyInstance(instance) => {
                let mut dict: indexmap::IndexMap<PyKey, Value> = indexmap::IndexMap::new();
                for (k, v) in instance.borrow().attrs.iter() {
                    dict.insert(PyKey::Str(k.clone()), v.clone());
                }
                Ok(Value::dict(dict))
            }
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
                    return Err(PyError::Runtime("object has no len()".to_string()));
                }
            }
            _ => {
                return Err(PyError::Runtime("object has no len()".to_string()));
            }
        };
        Ok(Value::int(size))
    }

    /// CPython: sorted(iterable, /, *, key=None, reverse=False) — new sorted list.
    /// <https://docs.python.org/3/library/functions.html#sorted>
    fn sorted(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::Runtime(format!("{FN_NAME}() requires at least one argument")));
        }
        let reverse = args.iter().find(|a| a.name.as_deref() == Some("reverse"))
            .map(|a| a.value.truthy())
            .unwrap_or(false);
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
        let mut items = iter_values(positional[0].value.clone())?;
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
    fn min(args) -> Result<Value> {
        min_max_impl(_interp, args, false, FN_NAME)
    }

    /// CPython: max(iterable, /, *, key=None) or max(*args, key=None).
    /// <https://docs.python.org/3/library/functions.html#max>
    fn max(args) -> Result<Value> {
        min_max_impl(_interp, args, true, FN_NAME)
    }

    /// CPython: round(number[, ndigits]) — banker's rounding.
    /// <https://docs.python.org/3/library/functions.html#round>
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
                ValueKind::List(items) | ValueKind::Tuple(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for v in items {
                        match v.kind() {
                            ValueKind::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
                            ValueKind::Int(_) => return Err(PyError::named(
                                "ValueError",
                                "bytes must be in range(0, 256)".to_string(),
                            )),
                            _ => return Err(PyError::named(
                                "TypeError",
                                "bytes element must be an integer".to_string(),
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
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most 1 positional argument"))),
        }
    }

    /// CPython: complex(real=0, imag=0) — complex number.
    /// <https://docs.python.org/3/library/functions.html#complex>
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
    fn bool(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            0 => Ok(Value::bool_(false)),
            1 => {
                let val = args[0].value.clone();
                let result = _interp.truthy_value(&val)?;
                Ok(Value::bool_(result))
            }
            _ => Err(PyError::Runtime(format!("{FN_NAME}() takes at most one argument"))),
        }
    }

    /// CPython: dict() — empty dict (rich constructor forms unsupported).
    /// <https://docs.python.org/3/library/functions.html#func-dict>
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
        let pos_items = iter_values(args[1].value.clone())?;
        let mut expanded: Vec<ExpandedCallArg> = pos_items
            .into_iter()
            .map(|v| ExpandedCallArg { name: None, value: v })
            .collect();
        if let ValueKind::Dict(kw_map) = args[2].value.kind() {
            for (k, v) in kw_map {
                if let PyKey::Str(name) = k {
                    expanded.push(ExpandedCallArg { name: Some(name.clone()), value: v.clone() });
                }
            }
        }
        _interp.call_function_expanded(func, &expanded)
    }

    /// CPython: format(value[, format_spec]).
    /// <https://docs.python.org/3/library/functions.html#format>
    fn format(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let (value, spec) = match args.len() {
            1 => (args[0].value.clone(), String::new()),
            2 => {
                let value = args[0].value.clone();
                let spec = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    _ => return Err(PyError::Runtime("format spec must be a string".to_string())),
                };
                (value, spec)
            }
            _ => return Err(PyError::Runtime(format!("{FN_NAME}() takes 1 or 2 arguments"))),
        };
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
                        value: Value::string(spec.clone()),
                    }],
                )?;
                return match result.kind() {
                    ValueKind::Str(_) => Ok(result),
                    _ => Err(PyError::named(
                        "TypeError",
                        format!(
                            "__format__ must return a str, not {}",
                            value_type_name_str(&result),
                        ),
                    )),
                };
            }
        }
        apply_format_spec(&value, &spec)
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
    fn callable(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let is_callable = match args[0].value.kind() {
            ValueKind::UserFunction(_)
            | ValueKind::BuiltinFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. }
            | ValueKind::PyClass(_) => true,
            // Only accessor partials (intermediate results of
            // prop.setter / prop.getter / prop.deleter) are callable —
            // a plain property descriptor isn't.
            ValueKind::BuiltinObject { .. } => {
                pyrust_builtins::property::property_partial_slot(&args[0].value)
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
        iter_values(positional[0].value.clone())?
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
