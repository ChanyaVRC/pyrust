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
    AsyncGenASend, BigRangeIter, CallableIter, ChainFromIterableIter, EnumerateIter, FilterIter, GeneratorFrame, GetItemIter, GuardVersion, IterSrcBuf, MapIter, NativeIterFrame, NativeIterGuard, ZipIter, apply_format_spec, apply_format_spec_named, ascii_repr_interp, bigint_divmod_floor,
    class_chain_contains_name, class_hash_inherits_builtin_none, class_is_subclass_of,
    class_suppresses_instance_dict,
    compare_values, compare_values_with_op, coerce_numeric, coerce_subclass_backing, dir_names,
    dispatch_numeric_binop,
    find_immutable_primitive_base, find_mutable_primitive_base, find_scalar_primitive_base,
    builtin_data_backing, extract_str_value, float_divmod, float_to_bigint, instance_builtin_data,
    invoke_class_method,
    is_exception_class, is_str_or_str_subclass, iter_values, key_to_value, lookup_class_attr, mapping_pairs_via_protocol, modinv_bigint, modinv_i64, modpow_bigint, modpow_i64, primitive_class_by_name, py_hash_bigint, py_hash_float,
    py_hash_int, py_mod_i64, py_round_half_even_checked, round_float_ndigits,
    bind_constructor_kwargs, reject_keyword_args_expanded, resolve_zero_arg_super, round_bigint_neg_ndigits, snapshot_current_locals,
    function_type_singleton, method_type_singleton,
    sync_module_env_to_globals_dict, type_class_singleton,
    unicode_exc_set_attrs,
    value_to_float, value_type_name_str,
};
use crate::value::{InstanceAttrs, PyBigInt, PyClass, PyDict, PyKey, PySet, PyToPrimitive, PyZero, UserFunctionKind, Value, ValueKind, range_len};
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: chr(i) — return the string of one Unicode codepoint i.
    /// <https://docs.python.org/3/library/functions.html#chr>
    ///
    /// Migrated to the typed-signature dialect (#400) as a three-element
    /// overload set: `PyInt` is the primary path; `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping (`chr(True) == '\x01'`), since
    /// strict `PyInt` doesn't auto-coerce `bool` in the typed dialect.
    /// A trailing `PyValue` catch-all resolves a user object's `__index__`
    /// (CPython 3.12 honors the index protocol for `chr`, #1908) and
    /// otherwise raises `"'X' object cannot be interpreted as an integer"`,
    /// matching CPython 3.12 verbatim.  All parameters are
    /// `#[positional_only]` so the macro's positional-only fast-path
    /// applies (no kwarg-validation work).  Bignum inputs that don't fit
    /// in i64 raise `OverflowError("Python int too large to convert to C
    /// int")` — matching CPython 3.12's exact wording (#1584).  Values
    /// that fit in i64 but exceed the Unicode range raise `ValueError` via
    /// `chr_from_code_point`.
    #[arity_style(takes_exactly_one)]
    fn chr(#[positional_only] i: PyInt) -> Result<Value> {
        let code_point = i.as_i64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )
        })?;
        chr_from_code_point(code_point)
    }

    #[arity_style(takes_exactly_one)]
    fn chr(#[positional_only] i: PyBool) -> Result<Value> {
        // CPython: `chr(True) == '\x01'`, `chr(False) == '\x00'`.
        chr_from_code_point(if i.0 { 1 } else { 0 })
    }

    #[arity_style(takes_exactly_one)]
    fn chr(#[positional_only] i: PyValue) -> Result<Value> {
        // CPython 3.12: chr() honors the __index__ protocol. A plain int /
        // bool is handled by the typed overloads above; here we resolve a
        // user object's __index__ (mirroring bin/oct/hex) via the shared
        // index protocol (issue #2022), then apply the same range check as
        // the PyInt path. __int__ alone is not enough.
        let resolved = _interp.value_to_index(&i.0, not_an_integer_err)?;
        match resolved.kind() {
            ValueKind::Bool(b) => chr_from_code_point(if b { 1 } else { 0 }),
            ValueKind::Int(v) => chr_from_code_point(v),
            // A bignum codepoint always exceeds the Unicode range; CPython's
            // chr() converts via a C int first, so it overflows there.
            ValueKind::BigInt(_) => Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            )),
            _ => unreachable!("value_to_index guarantees an integer"),
        }
    }

    /// CPython: ord(c) — return the Unicode codepoint of a one-character string.
    /// <https://docs.python.org/3/library/functions.html#ord>
    ///
    /// Migrated to the typed-signature dialect (#400) as a three-element
    /// overload set: `PyStr` is the primary path; `PyBytes` mirrors
    /// CPython's acceptance of one-byte bytes (`ord(b"A") == 65`); a
    /// trailing `PyValue` catch-all raises `"expected string of length 1,
    /// but TYPE found"` using the actual type name (CPython 3.12 wording,
    /// fixed in #1339).  Length-mismatch wording on the `PyStr` overload
    /// is preserved verbatim from CPython so parity output is stable.
    /// All parameters are `#[positional_only]` so the macro's
    /// positional-only fast-path applies.  The `PyBytes` overload is a
    /// new CPython-parity feature — the legacy body rejected `bytes`
    /// outright, but CPython has always accepted a 1-byte `bytes`
    /// (`ord(b"A") == 65`).
    #[pure]
    #[arity_style(takes_exactly_one)]
    fn ord(#[positional_only] c: PyStr) -> Result<Value> {
        let s: &str = &c;
        // Use the surrogate-safe codepoint iterator, not `str::chars()`:
        // a string built from a lone-surrogate escape (`"\udc80"`) is stored
        // CESU-8, and `chars()` would feed the surrogate to
        // `char::from_u32_unchecked` → UB / debug-build abort (issue #1893).
        let mut cps = pyrust_core::cesu8_codepoints(s);
        let first = cps.next();
        let second = cps.next();
        match (first, second) {
            (Some(cp), None) => Ok(Value::int(cp as i64)),
            (None, _) => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expected a character, but string of length 0 found"),
            )),
            (Some(_), Some(_)) => Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() expected a character, but string of length {} found",
                    pyrust_core::cesu8_codepoints(s).count()
                ),
            )),
        }
    }

    #[pure]
    #[arity_style(takes_exactly_one)]
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
    #[arity_style(takes_exactly_one)]
    fn ord(#[positional_only] c: PyValue) -> Result<Value> {
        Err(PyError::named(
            "TypeError",
            format!(
                "{FN_NAME}() expected string of length 1, but {} found",
                value_type_name_str(&c.0),
            ),
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
    /// Both small (`i64`) and BigInt arguments are handled (#1226).
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `bin() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn bin(#[positional_only] x: PyInt) -> Result<Value> {
        if let Some(v) = x.as_i64() {
            Ok(Value::string(format_bin_i64(v)))
        } else {
            Ok(Value::string(format_bigint_radix(&x.to_bigint(), 2, "0b")))
        }
    }

    #[arity_style(takes_exactly_one)]
    fn bin(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `bin(True) == '0b1'`, `bin(False) == '0b0'`.
        Ok(Value::string(format_bin_i64(if x.0 { 1 } else { 0 })))
    }

    #[arity_style(takes_exactly_one)]
    fn bin(#[positional_only] x: PyValue) -> Result<Value> {
        // Issue #1929 / #2022: resolve int/bool subclasses and `__index__`
        // objects uniformly via the shared index protocol, then format.
        let resolved = _interp.value_to_index(&x.0, not_an_integer_err)?;
        Ok(Value::string(format_index_radix(&resolved, 2, "0b", format_bin_i64)))
    }

    /// CPython: oct(x) — integer to '0o…' / '-0o…' string.
    /// <https://docs.python.org/3/library/functions.html#oct>
    ///
    /// Migrated to the typed-signature dialect (#400) mirroring `hex`'s
    /// 3-overload pattern: `PyInt` is the primary path, `PyBool` mirrors
    /// CPython's `bool ⊆ int` subtyping, and a trailing `PyValue`
    /// catch-all reproduces CPython's exact "'X' object cannot be
    /// interpreted as an integer" TypeError wording verbatim.
    /// Both small (`i64`) and BigInt arguments are handled (#1226).
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `oct() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn oct(#[positional_only] x: PyInt) -> Result<Value> {
        if let Some(v) = x.as_i64() {
            Ok(Value::string(format_oct_i64(v)))
        } else {
            Ok(Value::string(format_bigint_radix(&x.to_bigint(), 8, "0o")))
        }
    }

    #[arity_style(takes_exactly_one)]
    fn oct(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `oct(True) == '0o1'`, `oct(False) == '0o0'`.
        Ok(Value::string(format_oct_i64(if x.0 { 1 } else { 0 })))
    }

    #[arity_style(takes_exactly_one)]
    fn oct(#[positional_only] x: PyValue) -> Result<Value> {
        // Issue #1929 / #2022: resolve via the shared index protocol, format.
        let resolved = _interp.value_to_index(&x.0, not_an_integer_err)?;
        Ok(Value::string(format_index_radix(&resolved, 8, "0o", format_oct_i64)))
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
    /// Both small (`i64`) and BigInt arguments are handled (#1226).
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `hex() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn hex(#[positional_only] x: PyInt) -> Result<Value> {
        if let Some(v) = x.as_i64() {
            Ok(Value::string(format_hex_i64(v)))
        } else {
            Ok(Value::string(format_bigint_radix(&x.to_bigint(), 16, "0x")))
        }
    }

    #[arity_style(takes_exactly_one)]
    fn hex(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: `hex(True) == '0x1'`, `hex(False) == '0x0'`.
        Ok(Value::string(format_hex_i64(if x.0 { 1 } else { 0 })))
    }

    #[arity_style(takes_exactly_one)]
    fn hex(#[positional_only] x: PyValue) -> Result<Value> {
        // Issue #1929 / #2022: resolve via the shared index protocol, format.
        let resolved = _interp.value_to_index(&x.0, not_an_integer_err)?;
        Ok(Value::string(format_index_radix(&resolved, 16, "0x", format_hex_i64)))
    }

    /// CPython: ascii(object) — ASCII-only escaped repr.
    /// <https://docs.python.org/3/library/functions.html#ascii>
    ///
    /// Migrated to the typed-signature dialect (#400): like `repr`,
    /// `ascii` accepts every Python object, so `PyValue` is the natural
    /// wrapper.  Not marked `#[pure]` because it dispatches user `__repr__`
    /// for `PyInstance` values, which may invoke arbitrary user code.
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `ascii() takes exactly one argument (N given)`.
    #[arity_style(takes_exactly_one)]
    fn ascii(#[positional_only] obj: PyValue) -> Result<Value> {
        Ok(Value::string(ascii_repr_interp(_interp, &obj.0)?))
    }

    /// CPython: id(object) — identity (CPython returns memory address).
    /// <https://docs.python.org/3/library/functions.html#id>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` is the
    /// catch-all wrapper since `id` accepts every Python object; the
    /// existing per-kind dispatch becomes the body's only concern.
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `id() takes exactly one argument (N given)`.
    #[pure]
    #[arity_style(takes_exactly_one)]
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
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `abs() takes exactly one argument (N given)`.
    #[pure]
    #[arity_style(takes_exactly_one)]
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
    #[arity_style(takes_exactly_one)]
    fn abs(#[positional_only] x: PyFloat) -> Result<Value> {
        Ok(Value::float(x.0.abs()))
    }

    #[pure]
    #[arity_style(takes_exactly_one)]
    fn abs(#[positional_only] x: PyBool) -> Result<Value> {
        // CPython: abs(True) == 1, abs(False) == 0 — promoted to int.
        Ok(Value::int(if x.0 { 1 } else { 0 }))
    }

    #[pure]
    #[arity_style(takes_exactly_one)]
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
            // Issue #1204: fall through to the scalar backing value for
            // primitive subclasses (MyInt, MyFloat, …) that have no user
            // __abs__ defined.
            if let Some(backing) = instance_builtin_data(&inst_rc) {
                match backing.kind() {
                    ValueKind::Int(v) => {
                        // i64::MIN.checked_abs() returns None because
                        // -i64::MIN overflows i64; promote to BigInt to
                        // match CPython (mirrors the PyInt arm above).
                        return Ok(match v.checked_abs() {
                            Some(abs) => Value::int(abs),
                            None => {
                                let big: crate::value::PyBigInt = v.into();
                                Value::bigint(-big)
                            }
                        });
                    }
                    ValueKind::BigInt(v) => {
                        let zero: crate::value::PyBigInt = 0i64.into();
                        let abs = if v < &zero { -v.clone() } else { v.clone() };
                        return Ok(Value::bigint(abs));
                    }
                    ValueKind::Float(v) => return Ok(Value::float(v.abs())),
                    ValueKind::Bool(b) => return Ok(Value::int(if b { 1 } else { 0 })),
                    _ => {}
                }
            }
            return Err(PyError::named(
                "TypeError",
                format!("bad operand type for abs(): '{}'", class.borrow().name),
            ));
        }
        if let ValueKind::Complex(re, im) = val.kind() {
            // Overflow/underflow-safe magnitude (CPython's `_Py_c_abs` uses
            // `hypot`): `(re*re + im*im).sqrt()` spuriously yields inf/0.0 for
            // huge/tiny components.  `f64::hypot` matches CPython byte-for-byte,
            // including inf-beats-nan on infinite components.
            return Ok(Value::float(re.hypot(im)));
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
    /// `PyValue` so user-defined iterables reach the iterator protocol.
    /// `start` is `Option<PyValue>` with `#[default(None)]`; the body
    /// uses 0 when absent.  Known divergence: `sum([], None)` is 0
    /// (not CPython's `None`) because `Option<PyValue>` maps both
    /// "absent" and "Python None" to Rust `None`.  Tracked as a
    /// follow-up fixture under #400.
    ///
    /// Mirrors CPython 3.12 `builtin_sum` (#1975 / #2050):
    ///   * iterates the argument lazily — no full materialisation;
    ///   * keeps an `i64` int fast path (drops to generic `__add__` on
    ///     overflow or a big int, exactly like CPython's C-long path);
    ///   * switches to Neumaier (Kahan–Babuška) compensated summation
    ///     the moment a `float` is involved, for bit-exact float sums;
    ///   * falls back to the general `__add__` loop for a non-numeric
    ///     start or the first non-numeric element.
    #[pure]
    fn sum(
        #[positional_only] iterable: PyValue,
        #[positional_only]
        #[default(None)]
        start: Option<PyValue>,
    ) -> Result<Value> {
        let start = start.map(|v| v.0);
        // CPython rejects str/bytes/bytearray as the *start* (accumulator)
        // before entering the loop; element-level rejection falls out of the
        // normal `int + str` TypeError in the generic path.
        if let Some(ref s) = start {
            if s.is_str() {
                return Err(PyError::named(
                    "TypeError",
                    "sum() can't sum strings [use ''.join(seq) instead]",
                ));
            }
            if matches!(s.kind(), ValueKind::Bytes(_)) {
                return Err(PyError::named(
                    "TypeError",
                    "sum() can't sum bytes [use b''.join(seq) instead]",
                ));
            }
            if let ValueKind::BuiltinObject { ops, .. } = s.kind()
                && ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME
            {
                return Err(PyError::named(
                    "TypeError",
                    "sum() can't sum bytearray [use b''.join(seq) instead]",
                ));
            }
        }

        // Accumulator state machine matching CPython's int/float fast paths
        // plus a generic `__add__` fallback.  `Float` carries the running
        // sum and the Neumaier compensation term.  There is deliberately no
        // BigInt fast accumulator: CPython's int fast path uses a C long, and
        // on overflow (or a too-large int item) it drops straight into the
        // generic `PyNumber_Add` loop for the remainder — we mirror that so
        // bit results stay identical when a big int meets a float.
        enum Acc {
            Int(i64),
            Float(f64, f64),
            Generic(Value),
        }

        // Neumaier (Kahan–Babuška) compensated step, bit-identical to
        // CPython 3.12's float fast path.
        fn neumaier(sum: f64, c: f64, x: f64) -> (f64, f64) {
            let t = sum + x;
            let c = if sum.abs() >= x.abs() {
                c + ((sum - t) + x)
            } else {
                c + ((x - t) + sum)
            };
            (t, c)
        }
        // CPython returns `f_result + c`, but drops `c` when it is non-finite
        // so an infinite running sum keeps its sign instead of collapsing to
        // NaN (e.g. `sum([inf, 1.0])` → inf, not nan).
        fn finalize_float(sum: f64, c: f64) -> f64 {
            if c.is_finite() { sum + c } else { sum }
        }

        // Seed the accumulator from `start`.  CPython only enters the int fast
        // path for an *exact* int (`PyLong_CheckExact`): a `bool` start is a
        // subclass, so it (and any non-numeric start) begins in generic mode,
        // which also preserves `sum([], start) == start` unchanged.
        let mut acc = match &start {
            None => Acc::Int(0),
            Some(v) => match v.kind() {
                ValueKind::Int(n) => Acc::Int(n),
                ValueKind::Float(f) => Acc::Float(f, 0.0),
                _ => Acc::Generic(v.clone()),
            },
        };

        let iter = _interp.call_function_expanded(
            Value::builtin_function("iter"),
            &[ExpandedCallArg { name: None, value: iterable.0 }],
        )?;

        loop {
            let item = match _interp.call_next(&iter, None) {
                Ok(item) => item,
                Err(ref e) if e.class_name_is("StopIteration") => break,
                Err(e) => return Err(e),
            };

            // Classify the element without holding a borrow of `item` (so it
            // can still be moved into the generic fallback).  `bool` items DO
            // participate in the int/float fast paths (CPython's `PyBool_Check`
            // branch); only big ints / non-numerics break out.
            enum Num {
                Int(i64),
                Float(f64),
                Other,
            }
            let num = match item.kind() {
                ValueKind::Int(n) => Num::Int(n),
                ValueKind::Bool(b) => Num::Int(b as i64),
                ValueKind::Float(f) => Num::Float(f),
                _ => Num::Other,
            };

            match &mut acc {
                Acc::Int(s) => match num {
                    Num::Int(n) => match s.checked_add(n) {
                        Some(r) => *s = r,
                        // Overflow: fall to the generic loop with `s + item`,
                        // exactly as CPython does (no BigInt fast path).
                        None => {
                            let cur = Value::int(*s);
                            acc = Acc::Generic(_interp.eval_binary(cur, BinaryOp::Add, item)?);
                        }
                    },
                    // CPython seeds the float path with `f_result =
                    // (double)i_result` then adds the first float plainly
                    // (compensation only kicks in from the *next* element).
                    Num::Float(f) => acc = Acc::Float(*s as f64 + f, 0.0),
                    // Big int (or any non-fast item): continue in generic mode.
                    Num::Other => {
                        let cur = Value::int(*s);
                        acc = Acc::Generic(_interp.eval_binary(cur, BinaryOp::Add, item)?);
                    }
                },
                Acc::Float(s, c) => match num {
                    // Floats go through the compensated step …
                    Num::Float(f) => {
                        let (sum, comp) = neumaier(*s, *c, f);
                        *s = sum;
                        *c = comp;
                    }
                    // … but ints are added plainly (no compensation), exactly
                    // as CPython's float loop handles small `PyLong` items.
                    Num::Int(n) => *s += n as f64,
                    // Big int / non-numeric: hand `f_result + c` to the generic
                    // loop (CPython rebuilds a PyFloat and calls PyNumber_Add).
                    Num::Other => {
                        let cur = Value::float(finalize_float(*s, *c));
                        acc = Acc::Generic(_interp.eval_binary(cur, BinaryOp::Add, item)?);
                    }
                },
                Acc::Generic(s) => {
                    let cur = std::mem::replace(s, Value::int(0));
                    *s = _interp.eval_binary(cur, BinaryOp::Add, item)?;
                }
            }
        }

        Ok(match acc {
            Acc::Int(s) => Value::int(s),
            Acc::Float(s, c) => Value::float(finalize_float(s, c)),
            Acc::Generic(s) => s,
        })
    }

    /// CPython: any(iterable) — true if any element is truthy.
    /// <https://docs.python.org/3/library/functions.html#any>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` as
    /// `iterable` so user-defined iterables (PyInstance with `__iter__`)
    /// reach the iter protocol rather than the registry-only path.
    /// Iterates lazily so it short-circuits on the first truthy value
    /// without consuming the rest of the iterator (fixes #1224).
    #[pure]
    fn any(#[positional_only] iterable: PyValue) -> Result<Value> {
        let iter = _interp.call_function_expanded(
            Value::builtin_function("iter"),
            &[ExpandedCallArg { name: None, value: iterable.0 }],
        )?;
        loop {
            match _interp.call_next(&iter, None) {
                Ok(item) => {
                    if _interp.truthy_value(&item)? {
                        return Ok(Value::bool_(true));
                    }
                }
                Err(ref e) if e.class_name_is("StopIteration") => break,
                Err(e) => return Err(e),
            }
        }
        Ok(Value::bool_(false))
    }

    /// CPython: all(iterable) — true if every element is truthy (or empty).
    /// <https://docs.python.org/3/library/functions.html#all>
    ///
    /// Migrated to the typed-signature dialect (#400).  `PyValue` as
    /// `iterable` — same rationale as `any`.
    /// Iterates lazily so it short-circuits on the first falsy value
    /// without consuming the rest of the iterator (fixes #1224).
    #[pure]
    fn all(#[positional_only] iterable: PyValue) -> Result<Value> {
        let iter = _interp.call_function_expanded(
            Value::builtin_function("iter"),
            &[ExpandedCallArg { name: None, value: iterable.0 }],
        )?;
        loop {
            match _interp.call_next(&iter, None) {
                Ok(item) => {
                    if !_interp.truthy_value(&item)? {
                        return Ok(Value::bool_(false));
                    }
                }
                Err(ref e) if e.class_name_is("StopIteration") => break,
                Err(e) => return Err(e),
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
    ///
    /// Not marked `#[pure]` because it dispatches user `__repr__` for
    /// `PyInstance` values (and transitively for instances inside containers),
    /// which may invoke arbitrary user code.
    #[arity_style(takes_exactly_one)]
    fn repr(#[positional_only] obj: PyValue) -> Result<Value> {
        // Fast path (#alloc): `repr(int)` == the digits, formatted straight into
        // the string Value (one allocation, no intermediate heap `String`).
        if let ValueKind::Int(n) = obj.0.kind() {
            return Ok(Value::int_string(n));
        }
        pyrust_core::check_int_str_conversion(&obj.0)?;
        let s = render_value_repr(_interp, &obj.0)?;
        Ok(Value::string(s))
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
    #[arity_style(takes_exactly_one)]
    fn hash(#[positional_only] obj: PyValue) -> Result<Value> {
        let value = obj.0;
        let hash_val = hash_value_with_interp(_interp, &value)?;
        Ok(Value::int(hash_val))
    }

    /// CPython: divmod(a, b) — `(a // b, a % b)`.
    /// <https://docs.python.org/3/library/functions.html#divmod>
    ///
    /// Migrated to the typed-signature dialect (#400/#2331):
    /// `#[arity_style(expected_got)]` reproduces CPython's METH_VARARGS
    /// wording (`divmod expected 2 arguments, got N`) that previously
    /// forced the raw `(args)` dispatch style.  Type dispatch for the
    /// primitive fast paths (int/bool/float combinations) is done inline by
    /// kind-matching; the dunder-dispatch and coerce_numeric fallback paths
    /// are unchanged.
    #[arity_style(expected_got)]
    fn divmod(
        #[positional_only] a: PyValue,
        #[positional_only] b: PyValue,
    ) -> Result<Value> {
        let a = &a.0;
        let b = &b.0;

        // Fast paths: primitive int/bool/float combinations, mirroring the
        // former typed overloads.  bool ⊆ int in CPython: bool arms coerce
        // to i64 before calling the int helper.
        let a_is_bool = a.is_bool();
        let b_is_bool = b.is_bool();
        let a_is_int = matches!(a.kind(), ValueKind::Int(_) | ValueKind::BigInt(_));
        let b_is_int = matches!(b.kind(), ValueKind::Int(_) | ValueKind::BigInt(_));
        let a_is_float = matches!(a.kind(), ValueKind::Float(_));
        let b_is_float = matches!(b.kind(), ValueKind::Float(_));

        if (a_is_int || a_is_bool) && (b_is_int || b_is_bool) {
            // Both are int-family (int or bool); no float involved.
            let ia = if a_is_bool {
                PyInt::from(if let ValueKind::Bool(v) = a.kind() { v as i64 } else { 0 })
            } else {
                PyInt::try_from_value(a, "divmod", "a")?
            };
            let ib = if b_is_bool {
                PyInt::from(if let ValueKind::Bool(v) = b.kind() { v as i64 } else { 0 })
            } else {
                PyInt::try_from_value(b, "divmod", "b")?
            };
            return divmod_int_int(ia, ib);
        }

        if (a_is_float || a_is_int || a_is_bool) && (b_is_float || b_is_int || b_is_bool) {
            // At least one operand is float (since the all-int case was handled
            // above); promote both to f64.
            let af = if a_is_float {
                if let ValueKind::Float(f) = a.kind() { f } else { unreachable!() }
            } else if a_is_bool {
                if let ValueKind::Bool(v) = a.kind() { v as i64 as f64 } else { unreachable!() }
            } else {
                let pi = PyInt::try_from_value(a, "divmod", "a")?;
                pyint_to_f64(&pi)?
            };
            let bf = if b_is_float {
                if let ValueKind::Float(f) = b.kind() { f } else { unreachable!() }
            } else if b_is_bool {
                if let ValueKind::Bool(v) = b.kind() { v as i64 as f64 } else { unreachable!() }
            } else {
                let pi = PyInt::try_from_value(b, "divmod", "b")?;
                pyint_to_f64(&pi)?
            };
            return divmod_float_float(af, bf);
        }

        // Dunder dispatch: consult `__divmod__` / `__rdivmod__` before raising
        // TypeError.  CPython's `PyNumber_Divmod` (Objects/abstract.c) tries
        // `nb_divmod` on the left operand first, then the right operand's slot,
        // and only raises `TypeError` when both return `NotImplemented` or are
        // absent.
        //
        // Subtype rule (mirrors CPython `binary_op1`): when `b`'s type is a
        // *proper* subtype of `a`'s type, try `b.__rdivmod__(a)` first.
        let a_class = if let ValueKind::PyInstance(inst) = a.kind() {
            Some(Rc::clone(&inst.borrow().class))
        } else {
            None
        };
        let b_class = if let ValueKind::PyInstance(inst) = b.kind() {
            Some(Rc::clone(&inst.borrow().class))
        } else {
            None
        };

        let b_is_proper_subtype_of_a = match (&a_class, &b_class) {
            (Some(ac), Some(bc)) => {
                !Rc::ptr_eq(ac, bc)
                    && class_is_subclass_of(bc, ac)
                    && bc.borrow().attrs.contains_key("__rdivmod__")
            }
            _ => false,
        };

        if b_is_proper_subtype_of_a
            && let (Some(bc), ValueKind::PyInstance(inst)) = (&b_class, b.kind())
                && let Some(m) = lookup_class_attr(bc, "__rdivmod__") {
                    let self_val = Value::py_instance(Rc::clone(inst));
                    let arg = ExpandedCallArg { name: None, value: a.clone() };
                    match invoke_class_method(_interp, m, self_val, &[arg]) {
                        Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }

        // Try a.__divmod__(b).
        if let (Some(ac), ValueKind::PyInstance(inst)) = (&a_class, a.kind())
            && let Some(m) = lookup_class_attr(ac, "__divmod__") {
                let self_val = Value::py_instance(Rc::clone(inst));
                let arg = ExpandedCallArg { name: None, value: b.clone() };
                match invoke_class_method(_interp, m, self_val, &[arg]) {
                    Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                    Err(e) => return Err(e),
                    _ => {}
                }
            }

        // Try b.__rdivmod__(a) (skipped above if already tried via subtype rule).
        if !b_is_proper_subtype_of_a
            && let (Some(bc), ValueKind::PyInstance(inst)) = (&b_class, b.kind())
                && let Some(m) = lookup_class_attr(bc, "__rdivmod__") {
                    let self_val = Value::py_instance(Rc::clone(inst));
                    let arg = ExpandedCallArg { name: None, value: a.clone() };
                    match invoke_class_method(_interp, m, self_val, &[arg]) {
                        Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }

        // Issue #1433: if no user-defined dunder was found (or all returned
        // NotImplemented), coerce int/float subclass instances to their primitive
        // backing and try the numeric helpers.  This handles `divmod(MyInt(10), 3)`
        // where `MyInt` does not define its own `__divmod__` — CPython delegates
        // through the `nb_divmod` slot inherited from `int`; pyrust mirrors that
        // with explicit coercion here.
        let ca = coerce_numeric(a);
        let cb = coerce_numeric(b);
        let ca_is_numeric = matches!(
            ca.kind(),
            ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Float(_) | ValueKind::Bool(_)
        );
        let cb_is_numeric = matches!(
            cb.kind(),
            ValueKind::Int(_) | ValueKind::BigInt(_) | ValueKind::Float(_) | ValueKind::Bool(_)
        );
        if ca_is_numeric && cb_is_numeric {
            let ca_is_float = matches!(ca.kind(), ValueKind::Float(_));
            let cb_is_float = matches!(cb.kind(), ValueKind::Float(_));
            if ca_is_float || cb_is_float {
                // At least one is float — promote both to f64.
                let af = match ca.kind() {
                    ValueKind::Float(f) => f,
                    ValueKind::Int(n) => n as f64,
                    ValueKind::Bool(b) => b as i64 as f64,
                    ValueKind::BigInt(_) => {
                        let pi = PyInt::try_from_value(&ca, "divmod", "a")?;
                        pyint_to_f64(&pi)?
                    }
                    _ => unreachable!(),
                };
                let bf = match cb.kind() {
                    ValueKind::Float(f) => f,
                    ValueKind::Int(n) => n as f64,
                    ValueKind::Bool(b) => b as i64 as f64,
                    ValueKind::BigInt(_) => {
                        let pi = PyInt::try_from_value(&cb, "divmod", "b")?;
                        pyint_to_f64(&pi)?
                    }
                    _ => unreachable!(),
                };
                return divmod_float_float(af, bf);
            } else {
                // Promote Bool → Int so PyInt::try_from_value can match
                // (Bool is not accepted by PyInt::matches).  Use nested blocks
                // so the borrow from kind() is dropped before ca/cb are moved.
                let ca = {
                    if let ValueKind::Bool(b) = ca.kind() {
                        Value::int(b as i64)
                    } else {
                        ca
                    }
                };
                let cb = {
                    if let ValueKind::Bool(b) = cb.kind() {
                        Value::int(b as i64)
                    } else {
                        cb
                    }
                };
                let ia = PyInt::try_from_value(&ca, "divmod", "a")?;
                let ib = PyInt::try_from_value(&cb, "divmod", "b")?;
                return divmod_int_int(ia, ib);
            }
        }

        Err(PyError::named(
            "TypeError",
            format!(
                "unsupported operand type(s) for divmod(): '{}' and '{}'",
                value_type_name_str(a),
                value_type_name_str(b),
            ),
        ))
    }

    /// CPython: pow(base, exp[, mod]) — exponentiation, optionally modular.
    /// <https://docs.python.org/3/library/functions.html#pow>
    ///
    /// Not marked `#[pure]` because it dispatches user `__pow__` / `__rpow__`
    /// for `PyInstance` values, which may invoke arbitrary user code.
    fn pow(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() missing required argument 'base' (pos 1)"),
            ));
        }
        if args.len() == 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() missing required argument 'exp' (pos 2)"),
            ));
        }
        if args.len() > 3 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 3 arguments ({} given)", args.len()),
            ));
        }
        if args.len() == 3 {
            let base_val = &args[0].value;
            let exp_val = &args[1].value;
            let mod_val = &args[2].value;

            // User-defined type as base: dispatch __pow__(exp, mod) first.
            if let ValueKind::PyInstance(inst) = base_val.kind() {
                let inst_rc = Rc::clone(inst);
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(method) = lookup_class_attr(&class, "__pow__") {
                    let self_val = Value::py_instance(inst_rc);
                    let exp_arg = ExpandedCallArg { name: None, value: exp_val.clone() };
                    let mod_arg = ExpandedCallArg { name: None, value: mod_val.clone() };
                    match invoke_class_method(_interp, method, self_val, &[exp_arg, mod_arg]) {
                        Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }
            }

            // Fall through to built-in integer modpow.
            //
            // CPython distinguishes two TypeError messages for 3-arg pow:
            //   - If any argument is a user-defined type (PyInstance): the
            //     "unsupported operand type(s)" format (three type names).
            //   - Otherwise (e.g. float args): "3rd argument not allowed unless
            //     all arguments are integers".
            let any_instance = matches!(base_val.kind(), ValueKind::PyInstance(_))
                || matches!(exp_val.kind(), ValueKind::PyInstance(_))
                || matches!(mod_val.kind(), ValueKind::PyInstance(_));
            if any_instance {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "unsupported operand type(s) for ** or pow(): '{}', '{}', '{}'",
                        value_type_name_str(base_val),
                        value_type_name_str(exp_val),
                        value_type_name_str(mod_val),
                    ),
                ));
            }
            let three_arg_type_error = || PyError::named(
                "TypeError",
                "pow() 3rd argument not allowed unless all arguments are integers".to_string(),
            );
            // Promote to BigInt when any argument is a BigInt so that values
            // outside the i64 range are handled correctly.  The i64 fast path
            // is kept for the common case where all three args fit in i64.
            let any_bigint = matches!(base_val.kind(), ValueKind::BigInt(_))
                || matches!(exp_val.kind(), ValueKind::BigInt(_))
                || matches!(mod_val.kind(), ValueKind::BigInt(_));
            if any_bigint {
                let base_big: PyBigInt = match base_val.kind() {
                    ValueKind::Int(v) => PyBigInt::from(v),
                    ValueKind::Bool(b) => PyBigInt::from(b as i64),
                    ValueKind::BigInt(b) => (*b).clone(),
                    _ => return Err(three_arg_type_error()),
                };
                let exp_big: PyBigInt = match exp_val.kind() {
                    ValueKind::Int(v) => PyBigInt::from(v),
                    ValueKind::Bool(b) => PyBigInt::from(b as i64),
                    ValueKind::BigInt(b) => (*b).clone(),
                    _ => return Err(three_arg_type_error()),
                };
                let mod_big: PyBigInt = match mod_val.kind() {
                    ValueKind::Int(v) => PyBigInt::from(v),
                    ValueKind::Bool(b) => PyBigInt::from(b as i64),
                    ValueKind::BigInt(b) => (*b).clone(),
                    _ => return Err(three_arg_type_error()),
                };
                if mod_big.is_zero() {
                    return Err(PyError::named(
                        "ValueError",
                        "pow() 3rd argument cannot be 0".to_string(),
                    ));
                }
                if exp_big.sign() == crate::value::PyBigIntSign::Minus {
                    // Negative exponent: compute base^|exp| mod |m|, then find modinv.
                    let abs_exp = -&exp_big;
                    let abs_mod = if mod_big.sign() == crate::value::PyBigIntSign::Minus {
                        -&mod_big
                    } else {
                        mod_big.clone()
                    };
                    let powered = modpow_bigint(&base_big, &abs_exp, &abs_mod);
                    let powered_big: PyBigInt = match powered.kind() {
                        ValueKind::Int(v) => PyBigInt::from(v),
                        ValueKind::BigInt(b) => (*b).clone(),
                        _ => unreachable!("modpow_bigint always returns Int or BigInt"),
                    };
                    match modinv_bigint(&powered_big, &abs_mod) {
                        None => return Err(PyError::named(
                            "ValueError",
                            "base is not invertible for the given modulus".to_string(),
                        )),
                        Some(inv) => {
                            use num_traits::ToPrimitive;
                            // inv is in [0, abs_mod).  Adjust for negative modulus:
                            // Python semantics: result has the same sign as modulus.
                            let result = if mod_big.sign() == crate::value::PyBigIntSign::Minus
                                && inv != PyBigInt::from(0i64)
                            {
                                inv - &abs_mod
                            } else {
                                inv
                            };
                            return Ok(match result.to_i64() {
                                Some(v) => Value::int(v),
                                None => Value::bigint(result),
                            });
                        }
                    }
                }
                return Ok(modpow_bigint(&base_big, &exp_big, &mod_big));
            }
            let base = match base_val.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(three_arg_type_error()),
            };
            let exp = match exp_val.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(three_arg_type_error()),
            };
            let modulus = match mod_val.kind() {
                ValueKind::Int(v) => v,
                ValueKind::Bool(b) => b as i64,
                _ => return Err(three_arg_type_error()),
            };
            if modulus == 0 {
                return Err(PyError::named(
                    "ValueError",
                    "pow() 3rd argument cannot be 0".to_string(),
                ));
            }
            if exp < 0 {
                // Negative exponent: compute base^|exp| mod |m|, then find modinv.
                let powered = modpow_i64(base, exp.unsigned_abs(), modulus);
                match modinv_i64(powered, modulus) {
                    None => return Err(PyError::named(
                        "ValueError",
                        "base is not invertible for the given modulus".to_string(),
                    )),
                    Some(inv) => {
                        // inv is in [0, |modulus|).  Adjust for negative modulus:
                        // Python semantics: result has the same sign as modulus.
                        let result = if modulus < 0 && inv != 0 {
                            inv - modulus.unsigned_abs() as i64
                        } else {
                            inv
                        };
                        return Ok(Value::int(result));
                    }
                }
            }
            let result = modpow_i64(base, exp as u64, modulus);
            Ok(Value::int(result))
        } else {
            let base_val = &args[0].value;
            let exp_val = &args[1].value;

            // Extract classes for the subtype rule (mirrors CPython `binary_op1`).
            let base_class = if let ValueKind::PyInstance(inst) = base_val.kind() {
                Some(Rc::clone(&inst.borrow().class))
            } else {
                None
            };
            let exp_class = if let ValueKind::PyInstance(inst) = exp_val.kind() {
                Some(Rc::clone(&inst.borrow().class))
            } else {
                None
            };

            // Subtype rule: if exp's class is a proper subtype of base's class
            // AND directly defines __rpow__, try exp.__rpow__(base) first.
            let exp_is_proper_subtype_of_base = match (&base_class, &exp_class) {
                (Some(bc), Some(ec)) => {
                    !Rc::ptr_eq(bc, ec)
                        && class_is_subclass_of(ec, bc)
                        && ec.borrow().attrs.contains_key("__rpow__")
                }
                _ => false,
            };

            if exp_is_proper_subtype_of_base
                && let (Some(ec), ValueKind::PyInstance(inst)) = (&exp_class, exp_val.kind())
                    && let Some(m) = lookup_class_attr(ec, "__rpow__") {
                        let self_val = Value::py_instance(Rc::clone(inst));
                        let arg = ExpandedCallArg { name: None, value: base_val.clone() };
                        match invoke_class_method(_interp, m, self_val, &[arg]) {
                            Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                            Err(e) => return Err(e),
                            _ => {}
                        }
                    }

            // Try base.__pow__(exp).
            if let (Some(bc), ValueKind::PyInstance(inst)) = (&base_class, base_val.kind())
                && let Some(m) = lookup_class_attr(bc, "__pow__") {
                    let self_val = Value::py_instance(Rc::clone(inst));
                    let arg = ExpandedCallArg { name: None, value: exp_val.clone() };
                    match invoke_class_method(_interp, m, self_val, &[arg]) {
                        Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                        Err(e) => return Err(e),
                        _ => {}
                    }
                }

            // Try exp.__rpow__(base), but only when:
            //   - the subtype rule didn't already try it, AND
            //   - the types differ (when type(exp) is type(base), CPython skips __rpow__).
            let types_differ = match (&base_class, &exp_class) {
                (Some(bc), Some(ec)) => !Rc::ptr_eq(bc, ec),
                // exp is a PyInstance but base is not: types clearly differ.
                (None, Some(_)) => true,
                // base is PyInstance but exp is not, or both non-instance: skip reflected.
                _ => false,
            };
            if !exp_is_proper_subtype_of_base && types_differ
                && let (Some(ec), ValueKind::PyInstance(inst)) = (&exp_class, exp_val.kind())
                    && let Some(m) = lookup_class_attr(ec, "__rpow__") {
                        let self_val = Value::py_instance(Rc::clone(inst));
                        let arg = ExpandedCallArg { name: None, value: base_val.clone() };
                        match invoke_class_method(_interp, m, self_val, &[arg]) {
                            Ok(v) if !matches!(v.kind(), ValueKind::NotImplemented) => return Ok(v),
                            Err(e) => return Err(e),
                            _ => {}
                        }
                    }

            // Fall through to built-in numeric pow.  Route through the same
            // NumericOps slot dispatch the `**` operator uses (#458) so that
            // bool operands are treated as int (bool ⊆ int): a non-negative
            // bool/int exponent yields an int, while a negative or float
            // exponent yields a float — matching the operator path exactly.
            if let Some(result) = dispatch_numeric_binop(BinaryOp::Pow, base_val, exp_val) {
                return result;
            }
            if base_class.is_some() || exp_class.is_some() {
                // At least one PyInstance — neither __pow__ nor __rpow__ succeeded.
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "unsupported operand type(s) for ** or pow(): '{}' and '{}'",
                        value_type_name_str(base_val),
                        value_type_name_str(exp_val),
                    ),
                ));
            }
            let a = value_to_float(base_val, FN_NAME)?;
            let b = value_to_float(exp_val, FN_NAME)?;
            Ok(Value::float(a.powf(b)))
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
        // `start` accepts any int (incl. BigInt and bool); a non-int raises the
        // CPython TypeError.  The counter is kept as a `Value` so it promotes to
        // BigInt on overflow instead of wrapping (#2125).
        let start_val: Value = match start {
            None => Value::int(0),
            Some(v) => match v.0.kind() {
                ValueKind::Int(_) | ValueKind::BigInt(_) => v.0.clone(),
                ValueKind::Bool(b) => Value::int(b as i64),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        value_type_name_str(&v.0),
                    ),
                )),
            },
        };
        // Convert the iterable to a lazy iterator without consuming any elements.
        // Elements are pulled lazily by step_enumerate_iter via call_next.
        let source = make_iterator(_interp, &iterable.0)?;
        Ok(Value::generator(Box::new(EnumerateIter {
            source,
            counter: start_val,
            done: false,
        })))
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
        // Convert each iterable to a lazy iterator without consuming any elements.
        // Elements are pulled lazily by step_zip_iter via call_next.
        let sources = args
            .iter()
            .filter(|a| a.name.is_none())
            .map(|a| make_iterator(_interp, &a.value))
            .collect::<Result<Vec<_>>>()?;
        Ok(Value::generator(Box::new(ZipIter {
            sources,
            strict,
            done: false,
            count: 0,
        })))
    }

    /// CPython: reversed(seq) — reverse iterator.
    /// <https://docs.python.org/3/library/functions.html#reversed>
    ///
    /// CPython's protocol (in order):
    ///   1. `__reversed__` — call it, return the iterator it produces.
    ///   2. `__len__` + `__getitem__` — collect via sequence protocol, reverse.
    ///   3. Otherwise: TypeError "'X' object is not reversible".
    ///
    /// For non-PyInstance values only sequences (list, tuple, str, bytes) and
    /// range are reversible; all other types (Generator, BuiltinObject
    /// iterators, …) raise TypeError.
    fn reversed(#[positional_only] seq: PyValue) -> Result<Value> {
        if let ValueKind::PyInstance(inst) = seq.0.kind() {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            // Protocol step 1: __reversed__
            if let Some(method_val) = lookup_class_attr(&class, "__reversed__") {
                return invoke_class_method(
                    _interp,
                    method_val,
                    Value::py_instance(inst_rc),
                    &[],
                );
            }
            // Protocol step 2: __getitem__ + __len__ (sequence protocol).
            // CPython checks __getitem__ first; if present but __len__ is
            // absent it raises "no len()" rather than "not reversible".
            if let Some(getitem_method) = lookup_class_attr(&class, "__getitem__") {
                let len_method = match lookup_class_attr(&class, "__len__") {
                    Some(m) => m,
                    None => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "object of type '{}' has no len()",
                                class.borrow().name,
                            ),
                        ))
                    }
                };
                let len_val = invoke_class_method(
                    _interp,
                    len_method,
                    Value::py_instance(Rc::clone(&inst_rc)),
                    &[],
                )?;
                let n = match len_val.kind() {
                    ValueKind::Int(n) if n >= 0 => n,
                    ValueKind::Bool(b) => if b { 1 } else { 0 },
                    ValueKind::Int(_) => {
                        return Err(PyError::named(
                            "ValueError",
                            "__len__() should return >= 0".to_string(),
                        ))
                    }
                    ValueKind::BigInt(b) => {
                        use num_bigint::Sign;
                        if b.sign() == Sign::Minus {
                            return Err(PyError::named(
                                "ValueError",
                                "__len__() should return >= 0".to_string(),
                            ));
                        }
                        return Err(PyError::named(
                            "OverflowError",
                            "cannot fit 'int' into an index-sized integer".to_string(),
                        ))
                    }
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "'{}' object cannot be interpreted as an integer",
                                pyrust_core::builtin_type_name(&len_val),
                            ),
                        ))
                    }
                };
                let obj = Value::py_instance(inst_rc);
                let mut items = Vec::with_capacity(n as usize);
                for i in 0..n {
                    let arg = ExpandedCallArg { name: None, value: Value::int(i) };
                    let item = invoke_class_method(
                        _interp,
                        getitem_method.clone(),
                        obj.clone(),
                        &[arg],
                    )?;
                    items.push(item);
                }
                let source = Value::list(items);
                return Ok(pyrust_builtins::iter_helpers::reversed(source));
            }
            // Protocol step 3: not reversible
            return Err(PyError::named(
                "TypeError",
                format!("'{}' object is not reversible", class.borrow().name),
            ));
        }
        // dict / dict views are reversible by insertion order (CPython 3.8+,
        // issue #2093).  The backing IndexMap preserves insertion order, so we
        // build a forward-ordered list of keys / values / (key, value) pairs,
        // reverse it, and wrap it in a `NativeIterFrame` carrying a size-mutation
        // guard keyed to the live backing dict (#2448).  Like CPython's forward
        // view iterators, mutating the dict's size during a `reversed()` walk
        // raises `RuntimeError` on the next `next()` call.
        if let ValueKind::Dict(map) = seq.0.kind() {
            let mut items: Vec<Value> = map.keys().map(|k| key_to_value(k.clone())).collect();
            items.reverse();
            let frame = make_reversed_dict_iter(items, seq.0.clone());
            return Ok(Value::generator(Box::new(frame)));
        }
        if let Some(kind) = pyrust_builtins::dict_views::view_kind(&seq.0)
            && let Some(rc) = pyrust_builtins::dict_views::as_dict_rc(&seq.0) {
                let mut items: Vec<Value> = {
                    let map = rc.borrow();
                    match kind {
                        // dict_keys
                        0 => map.keys().map(|k| key_to_value(k.clone())).collect(),
                        // dict_values
                        1 => map.values().cloned().collect(),
                        // dict_items
                        _ => map
                            .iter()
                            .map(|(k, v)| Value::tuple(vec![key_to_value(k.clone()), v.clone()]))
                            .collect(),
                    }
                };
                items.reverse();
                let frame = make_reversed_dict_iter(items, seq.0.clone());
                return Ok(Value::generator(Box::new(frame)));
            }
        // mappingproxy (`vars(C)` / `d.keys().mapping`): reverse like a dict, but
        // with a size-mutation guard keyed to the live proxy so a change mid-walk
        // raises `RuntimeError` (issue #2728).  Handled before the generic
        // `__reversed__` dispatch below because `mapping_proxy::call_method`
        // returns an unguarded list-reverse iterator with no interpreter access
        // to install the guard.
        if pyrust_builtins::mapping_proxy::as_class_rc(&seq.0).is_some()
            || pyrust_builtins::mapping_proxy::as_dict_rc(&seq.0).is_some()
        {
            let mut items = iter_values(&seq.0)?;
            items.reverse();
            let frame = make_reversed_dict_iter(items, seq.0.clone());
            return Ok(Value::generator(Box::new(frame)));
        }
        // BuiltinObject types that implement `__reversed__` (e.g. mappingproxy,
        // issue #2684) dispatch to it directly, matching CPython's protocol
        // step 1.  `call_method` already returns the reverse-order iterator.
        if let ValueKind::BuiltinObject { ops, state } = seq.0.kind()
            && ops.has_method("__reversed__")
        {
            return ops.call_method(state, "__reversed__", Vec::new(), &indexmap::IndexMap::new());
        }
        // Non-PyInstance: only sequence types and Range are reversible.
        // Generators (including list_iterator, set_iterator, filter, map, …)
        // and all BuiltinObject iterator types are not sequences and must
        // raise TypeError, matching CPython 3.12's check for __reversed__ /
        // (__len__ + __getitem__).
        let is_reversible = match seq.0.kind() {
            ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Str(_)
            | ValueKind::Bytes(_)
            | ValueKind::Range { .. }
            | ValueKind::BigRange { .. } => true,
            // `bytearray` is a mutable sequence (len + getitem) and is
            // reversible in CPython, yielding its bytes as ints (#2005).
            ValueKind::BuiltinObject { ops, .. } => {
                ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME
            }
            _ => false,
        };
        if !is_reversible {
            let type_name = full_type_name_str(&seq.0);
            return Err(PyError::named(
                "TypeError",
                format!("'{}' object is not reversible", type_name),
            ));
        }
        Ok(pyrust_builtins::iter_helpers::reversed(seq.0))
    }

    /// CPython: map(func, *iterables) — apply func to corresponding elements
    /// from all iterables in lockstep; stops at the shortest iterable.
    /// Returns a lazy `map` iterator object, not a list.
    /// <https://docs.python.org/3/library/functions.html#map>
    fn map(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() must have at least two arguments."),
            ));
        }
        let func = args[0].value.clone();
        // Convert each iterable argument to an iterator object without
        // consuming any elements.  Elements are pulled lazily by
        // step_map_iter via call_next.
        let sources: Result<IterSrcBuf> = args[1..]
            .iter()
            .map(|a| make_iterator(_interp, &a.value))
            .collect();
        let sources = sources?;
        Ok(Value::generator(Box::new(MapIter {
            func,
            sources,
            done: false,
        })))
    }

    /// CPython: filter(func, iterable) — keep elements where func is truthy.
    /// Returns a lazy `filter` iterator object, not a list.
    /// `func` may be `None` for identity truthiness testing.
    /// <https://docs.python.org/3/library/functions.html#filter>
    fn filter(
        #[positional_only] func: PyValue,
        #[positional_only] iterable: PyValue,
    ) -> Result<Value> {
        // Convert the iterable to an iterator without consuming any elements.
        // Elements are pulled lazily by step_filter_iter via call_next.
        let source = make_iterator(_interp, &iterable.0)?;
        let func_opt = if func.0.is_none() { None } else { Some(func.0) };
        Ok(Value::generator(Box::new(FilterIter {
            func: func_opt,
            source,
            done: false,
        })))
    }

    /// CPython: iter(obj) / iter(callable, sentinel) — return an iterator.
    /// <https://docs.python.org/3/library/functions.html#iter>
    ///
    /// The one-argument form returns an iterator over an iterable object.
    /// The two-argument form returns a callable-iterator that calls
    /// `callable()` on each `next()` and stops when the result equals
    /// `sentinel`.  `#[pure]` is absent because the two-argument form calls
    /// user code on every iteration.
    fn iter(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        match args.len() {
            2 => {
                // Two-argument form: iter(callable, sentinel).
                let callable = args[0].value.clone();
                let sentinel = args[1].value.clone();
                // Validate that arg 0 is callable — matches CPython's TypeError.
                let is_callable = match callable.kind() {
                    ValueKind::UserFunction(_)
                    | ValueKind::BuiltinFunction(_)
                    | ValueKind::BoundMethod { .. }
                    | ValueKind::ClassBoundMethod { .. }
                    | ValueKind::PyClass(_) => true,
                    ValueKind::BuiltinObject { .. } => {
                        pyrust_builtins::bound_method::is_bound_method(&callable)
                            || pyrust_builtins::super_bound_builtin::as_super_bound_builtin(
                                &callable,
                            )
                            .is_some()
                            || pyrust_builtins::property::property_partial_slot(&callable)
                                .is_some_and(|slot| slot.is_some())
                            || pyrust_builtins::type_call_wrapper::as_type_call_wrapper(&callable)
                                .is_some()
                    }
                    ValueKind::PyInstance(inst) => {
                        let class = Rc::clone(&inst.borrow().class);
                        lookup_class_attr(&class, "__call__").is_some()
                    }
                    _ => false,
                };
                if !is_callable {
                    return Err(PyError::named(
                        "TypeError",
                        "iter(object, sentinel): object must be callable".to_string(),
                    ));
                }
                Ok(Value::generator(Box::new(CallableIter {
                    callable,
                    sentinel,
                    done: false,
                })))
            }
            1 => {
                let val = args[0].value.clone();
                // Detect kind tag in a scoped block so the kind() borrow drops
                // before we may need to move `val` (#450).
                enum IterKind {
                    Generator,
                    PyInstance(Rc<RefCell<crate::value::PyInstance>>),
                    BigRange,
                    // A `BuiltinObject` that is itself an iterator (e.g.
                    // `reversed()`'s `list_reverseiterator`).  An iterator's
                    // `__iter__` returns `self`, so `iter(x) is x` (#2117).
                    SelfIterator,
                    Other,
                }
                // A coroutine (`async def`, issue #1039) — and an async
                // generator (#2280) — is not iterable.
                if is_coroutine_value(&val) {
                    let tn = full_type_name_str(&val);
                    return Err(pyrust_core::type_err!(
                        "'{tn}' object is not iterable"
                    ));
                }
                let kind = match val.kind() {
                    ValueKind::Generator(_) => IterKind::Generator,
                    ValueKind::PyInstance(inst) => IterKind::PyInstance(Rc::clone(inst)),
                    // Big range: lazy iterator (#2118) — never materialize.
                    ValueKind::BigRange { .. } => IterKind::BigRange,
                    ValueKind::BuiltinObject { ops, .. } if ops.is_iterator() => {
                        IterKind::SelfIterator
                    }
                    _ => IterKind::Other,
                };
                match kind {
                    IterKind::Generator | IterKind::SelfIterator => Ok(val),
                    IterKind::BigRange => make_iterator(_interp, &val),
                    IterKind::PyInstance(inst_rc) => {
                        let class = Rc::clone(&inst_rc.borrow().class);
                        // Mirror the `for`-loop `GetIter` path (#2400): a
                        // *user-defined* `__iter__` (UserFunction) still wins, but an
                        // inherited built-in `__iter__` slot on a backed subclass
                        // (`list`/`dict`/`OrderedDict`/`bytes`/`bytearray` subclass
                        // with no Python-level `__iter__`) is skipped so we iterate the
                        // backing primitive directly and pick the container-specific
                        // mutation message — rather than dispatching `dict.__iter__`,
                        // which always reports plain dict's wording.  Covers the
                        // bytes/bytearray subclass case from #2324.
                        let user_iter =
                            crate::interpreter::effective_user_iter(&class, &inst_rc);
                        if let Some(method_val) = user_iter {
                            let iter_obj = invoke_class_method(
                                _interp,
                                method_val,
                                Value::py_instance(inst_rc),
                                &[],
                            )?;
                            let is_valid_iter = match iter_obj.kind() {
                                ValueKind::Generator(_) => true,
                                ValueKind::PyInstance(it) => {
                                    let it_class = Rc::clone(&it.borrow().class);
                                    lookup_class_attr(&it_class, "__next__").is_some()
                                }
                                ValueKind::BuiltinObject { ops, .. } => ops.is_iterable(),
                                _ => false,
                            };
                            if !is_valid_iter {
                                return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "iter() returned non-iterator of type '{}'",
                                        value_type_name_str(&iter_obj),
                                    ),
                                ));
                            }
                            Ok(iter_obj)
                        } else if let Some(backing) = instance_builtin_data(&inst_rc) {
                            // list/tuple/dict subclass with no user-defined __iter__:
                            // iterate the backing primitive directly, matching the
                            // `for`-loop path (vm.rs `GetIter`) and CPython's
                            // inherited tp_iter slot behaviour.  A dict subclass also
                            // gets a size-mutation guard, OrderedDict-aware (#2400).
                            // CPython 3.12 tags `iter(OrderedDict(...))` as
                            // "odict_iterator", not "dict_keyiterator" (#2748);
                            // the backing primitive is a plain dict, so the
                            // OrderedDict-ness must come from the class chain.
                            let type_name = if backing.as_dict().is_some()
                                && crate::interpreter::class_is_named_ordered_dict(&class)
                            {
                                "odict_iterator"
                            } else {
                                builtin_iter_type_name(&backing)
                            };
                            // Pass the carrier (not the unwrapped backing) so a
                            // non-iterable base subclass (`class C(int): pass`)
                            // reports its own class name, not the base's (#2557).
                            let items = iter_values(&val)?;
                            let mut frame = NativeIterFrame::new(items, type_name);
                            if backing.as_dict().is_some()
                                && let Some(recorded_len) =
                                    crate::interpreter::live_collection_len(&backing)
                            {
                                let (msg, exhaust_first) =
                                    crate::interpreter::dict_subclass_iter_semantics(&class);
                                // issue #2465: OrderedDict iterators snapshot the
                                // clear tick to distinguish `clear()` wording.
                                let od_seq = if crate::interpreter::class_is_named_ordered_dict(
                                    &class,
                                ) {
                                    crate::interpreter::od_clear_seq_now()
                                } else {
                                    0
                                };
                                frame.guard = Some(Box::new(NativeIterGuard {
                                    container: backing.clone(),
                                    version: recorded_len as i64,
                                    kind: GuardVersion::Size,
                                    msg,
                                    exhaust_first,
                                    od_seq,
                                }));
                            }
                            Ok(Value::generator(Box::new(frame)))
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
                        // Determine the iterator type name before consuming val.
                        let iter_type_name = builtin_iter_type_name(&val);
                        let items = iter_values(&val).map_err(|_| {
                            PyError::named(
                                "TypeError",
                                format!("'{}' object is not iterable", value_type_name_str(&val)),
                            )
                        })?;
                        let mut frame = NativeIterFrame::new(items, iter_type_name);
                        // dict / set / dict-views: guard the manual `iter()`
                        // iterator against size mutation (#1988), mirroring the
                        // `for`-loop guard.
                        if let Some(recorded_len) = crate::interpreter::live_collection_len(&val) {
                            let ordered_view =
                                pyrust_builtins::dict_views::is_ordered_view(&val);
                            let msg = if val.set_len().is_some() {
                                "Set changed size during iteration"
                            } else if ordered_view {
                                // OrderedDict-backed view (issue #2436): match
                                // CPython's odict-view wording on size mutation.
                                "OrderedDict mutated during iteration"
                            } else {
                                "dictionary changed size during iteration"
                            };
                            // issue #2465: ordered views snapshot the clear tick.
                            let od_seq = if ordered_view {
                                crate::interpreter::od_clear_seq_now()
                            } else {
                                0
                            };
                            frame.guard = Some(Box::new(NativeIterGuard {
                                container: val.clone(),
                                version: recorded_len as i64,
                                kind: GuardVersion::Size,
                                msg,
                                exhaust_first: false,
                                od_seq,
                            }));
                        }
                        Ok(Value::generator(Box::new(frame)))
                    }
                }
            }
            0 => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at least 1 argument, got 0"),
            )),
            n => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 2 arguments, got {n}"),
            )),
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
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at least 1 argument, got 0"),
            ));
        }
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 2 arguments, got {}", args.len()),
            ));
        }
        let gen_val = args[0].value.clone();
        let default_val = if args.len() == 2 {
            Some(args[1].value.clone())
        } else {
            None
        };
        _interp.call_next(&gen_val, default_val)
    }

    /// CPython: issubclass(cls, classinfo) — true if `cls` is a subclass.
    /// <https://docs.python.org/3/library/functions.html#issubclass>
    fn issubclass(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 2 arguments, got {}", args.len()),
            ));
        }
        // The `arg 1 must be a class` validation lives inside
        // `issubclass_check`, *after* the `__subclasscheck__` hook is
        // resolved on `type(classinfo)`: CPython only rejects a non-class
        // `cls` when no custom `__subclasscheck__` handles it (and validates
        // lazily per tuple/union leaf), so `issubclass(5, M())` where
        // `type(M())` defines the hook must return the hook's result rather
        // than raising.  See issue #2525.
        let result = issubclass_check(FN_NAME, &args[0].value, &args[1].value, _interp)?;
        Ok(Value::bool_(result))
    }

    /// CPython: delattr(obj, name) — delete an attribute.
    /// <https://docs.python.org/3/library/functions.html#delattr>
    ///
    /// Kept in the `(args)` dialect (#2350): a non-`str` name must raise
    /// CPython's `attribute name must be string, not '<type>'` (no
    /// function prefix, names the offending type, accepts `str`
    /// subclasses) — a `PyStr` typed binding instead emits the generic
    /// `delattr() argument 'name' must be str, not int` and rejects str
    /// subclasses.  The shared `attr_name_arg` validator matches
    /// `getattr`/`hasattr`/`setattr` byte-for-byte.
    fn delattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 2 arguments, got {}", args.len()),
            ));
        }
        let name = attr_name_arg(&args[1].value)?;
        // Delegate to the canonical delete_attr path so that every value
        // kind (BuiltinFunction, UserFunction, BoundMethod, PyClass, …)
        // raises the correct error type and message instead of the old
        // catch-all "delattr() object has no writable attributes".
        _interp.delete_attr(args[0].value.clone(), &name)?;
        Ok(Value::none())
    }

    /// CPython: isinstance(obj, classinfo) — type check.
    /// <https://docs.python.org/3/library/functions.html#isinstance>
    fn isinstance(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 2 arguments, got {}", args.len()),
            ));
        }
        let result = isinstance_check(FN_NAME, &args[0].value, &args[1].value, _interp)?;
        Ok(Value::bool_(result))
    }

    /// CPython: type(object) → type / type(name, bases, namespace) → new class.
    /// <https://docs.python.org/3/library/functions.html#type>
    ///
    /// Not `#[pure]`: the 3-arg form runs class-creation hooks (`__set_name__`,
    /// `__init_subclass__`) which may execute arbitrary user code (#2129/#2130).
    fn r#type(args) -> Result<Value> {
        // The 3-arg form `type(name, bases, ns, **kwds)` forwards keyword args
        // to `__init_subclass__` (so a bad kwarg surfaces as
        // `X.__init_subclass__() takes no keyword arguments`, matching CPython);
        // the 1-arg form `type(obj)` takes none.  Split positional / keyword
        // accordingly instead of rejecting all kwargs up front.
        let positional: Vec<&ExpandedCallArg> =
            args.iter().filter(|a| a.name.is_none()).collect();
        let init_subclass_kwargs: Vec<ExpandedCallArg> = args
            .iter()
            .filter(|a| a.name.is_some())
            .cloned()
            .collect();
        if positional.len() == 3 {
            let name = match positional[0].value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "type.__new__() argument 1 must be str, not {}",
                        value_type_name_str(&positional[0].value),
                    ),
                )),
            };
            // Extract all bases from the bases sequence.  Collect into a Vec
            // first (inside a scoped block so the kind() Ref guard drops before
            // we work with the Values — see #450).
            let base_values: Vec<Value> = match positional[1].value.kind() {
                ValueKind::Tuple(items) => items.to_vec(),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "type.__new__() argument 2 must be tuple, not {}",
                        value_type_name_str(&positional[1].value),
                    ),
                )),
            };
            // Validate each entry and split into primary base + extra bases.
            // Issue #1453: reject non-subclassable singletons here too, so
            // `type("Foo", (type(None),), {})` raises TypeError just like the
            // `class Foo(type(None)): pass` syntax path does.
            let mut base: Option<Rc<RefCell<PyClass>>> = None;
            let mut extra_bases: Vec<Rc<RefCell<PyClass>>> = Vec::new();
            for (i, entry) in base_values.iter().enumerate() {
                match entry.kind() {
                    ValueKind::PyClass(c) => {
                        let cls = Rc::clone(c);
                        if let Some(tname) =
                            crate::interpreter::non_subclassable_builtin_name(&cls)
                        {
                            return Err(PyError::named(
                                "TypeError",
                                format!("type '{tname}' is not an acceptable base type"),
                            ));
                        }
                        if i == 0 {
                            base = Some(cls);
                        } else {
                            extra_bases.push(cls);
                        }
                    }
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() argument 2 entries must be classes"),
                    )),
                }
            }
            // Issue #1677: reject bases tuples that contain two or more
            // "solid" primitive types (int, str, float, bytes, tuple, list,
            // dict, set, frozenset) or two bases with non-empty `__slots__`
            // (issue #2109).  These have incompatible instance layouts; CPython
            // raises the same error via its `best_base`/`solid_base` check.
            {
                let all_bases: Vec<_> = base.iter().chain(extra_bases.iter()).cloned().collect();
                if crate::interpreter::bases_have_layout_conflict(&all_bases) {
                    return Err(PyError::named(
                        "TypeError",
                        "multiple bases have instance lay-out conflict".to_string(),
                    ));
                }
            }
            let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
            match positional[2].value.kind() {
                ValueKind::Dict(map) => {
                    for (k, v) in map.iter() {
                        if let PyKey::Str(key) = k {
                            attrs.insert(key.as_str().unwrap_or("").to_owned(), v.clone());
                        }
                    }
                }
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::mapping_proxy::TYPE_NAME =>
                {
                    if let Some(class_rc) =
                        pyrust_builtins::mapping_proxy::as_class_rc(&positional[2].value)
                    {
                        let class = class_rc.borrow();
                        for (k, v) in class.attrs.iter() {
                            attrs.insert(k.clone(), v.clone());
                        }
                    }
                }
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "type.__new__() argument 3 must be dict, not {}",
                        value_type_name_str(&positional[2].value),
                    ),
                )),
            }
            // Issues #2129 / #2130: route the 3-arg constructor through the
            // same finalization the `class` statement runs (set __module__,
            // process __slots__, call __set_name__ on descriptors and
            // __init_subclass__ on the base) so a `type()`-built class is not
            // missing hooks a `class`-built one has.  Keyword args are forwarded
            // to __init_subclass__ (CPython routes type()'s kwds there).
            return _interp.build_class_via_type(
                name, base, extra_bases, attrs, None, &init_subclass_kwargs,
            );
        }
        // The 1-arg form `type(obj)` accepts no keyword arguments.
        reject_keyword_args_expanded(FN_NAME, args)?;
        if positional.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes 1 or 3 arguments"),
            ));
        }
        let obj = &positional[0].value;
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
        Ok(value_class(obj))
    }

    /// CPython: hasattr(obj, name) — true if `getattr(obj, name)` would succeed.
    /// <https://docs.python.org/3/library/functions.html#hasattr>
    ///
    /// Kept in the `(args)` dialect (#400/#2331): hasattr is a warm path,
    /// and migrating to a typed `expected_got` signature regressed a tight
    /// `hasattr(o, 'x')` hit/miss loop ~6–8% (the per-arg `PyValue` binding
    /// clone) for zero wording benefit — its arity/kwarg messages already
    /// match CPython.  Bench captured in the #400 batch-1 PR.
    fn hasattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 2 arguments, got {}", args.len()),
            ));
        }
        let name = attr_name_arg(&args[1].value)?;
        let result = match _interp.get_attr(&args[0].value, &name) {
            Ok(_) => true,
            Err(ref e) if e.class_name_is("AttributeError") => false,
            Err(e) => return Err(e),
        };
        Ok(Value::bool_(result))
    }

    /// CPython: getattr(obj, name[, default]) — attribute access by name.
    /// <https://docs.python.org/3/library/functions.html#getattr>
    ///
    /// Must stay in the `(args)` dialect (#400/#2331): `getattr(o, n, None)`
    /// must return Python `None` as the default rather than re-raising the
    /// `AttributeError`, but an `Option<PyValue>` + `#[default(None)]`
    /// trailing param collapses an explicit `None` default and an absent
    /// default into the same Rust `None`, breaking the `default=None` case
    /// (the exact blocker documented on `fn next`).  Its arity/kwarg
    /// wording already matches CPython, so there is no wording gain from a
    /// typed migration either.
    fn getattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() < 2 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at least 2 arguments, got {}", args.len()),
            ));
        }
        if args.len() > 3 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 3 arguments, got {}", args.len()),
            ));
        }
        let name = attr_name_arg(&args[1].value)?;
        match _interp.get_attr(&args[0].value, &name) {
            Ok(v) => Ok(v),
            Err(ref e) if e.class_name_is("AttributeError") && args.len() == 3 => {
                Ok(args[2].value.clone())
            }
            Err(e) => Err(e),
        }
    }

    /// CPython: setattr(obj, name, value) — attribute assignment by name.
    /// <https://docs.python.org/3/library/functions.html#setattr>
    ///
    /// Kept in the `(args)` dialect (#2350): a non-`str` name must raise
    /// CPython's `attribute name must be string, not '<type>'` (no
    /// function prefix, names the offending type, accepts `str`
    /// subclasses) — a `PyStr` typed binding instead emits the generic
    /// `setattr() argument 'name' must be str, not int` and rejects str
    /// subclasses.  The shared `attr_name_arg` validator matches
    /// `getattr`/`hasattr`/`delattr` byte-for-byte.
    fn setattr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 3 arguments, got {}", args.len()),
            ));
        }
        let name = attr_name_arg(&args[1].value)?;
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
            // vars() with no args == locals(): return a snapshot of the current
            // frame's local namespace.  At module scope that is module globals
            // (CPython parity: vars() is locals() is globals() at top level).
            let is_module_scope = _interp
                .vm_frame_views
                .last()
                .map(|v| v.kind == crate::interpreter::FrameKind::Script)
                .unwrap_or(true);
            if is_module_scope {
                sync_module_env_to_globals_dict(_interp);
                return Ok(_interp.module_globals_dict.clone());
            }
            return Ok(Value::dict(snapshot_current_locals(_interp)));
        }
        match args[0].value.kind() {
            ValueKind::PyInstance(instance) => {
                // Issue #2076: a `__slots__` instance with no `__dict__` has no
                // mapping, so `vars()` raises TypeError (CPython parity).
                if class_suppresses_instance_dict(&instance.borrow().class) {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() argument must have __dict__ attribute"),
                    ));
                }
                // Issue #1981: when `__dict__` was replaced wholesale, the live
                // backing dict is `vars(obj)` (so `vars(obj) is obj.__dict__`).
                if let Some(d) = instance.borrow().attrs.dict_ref() {
                    return Ok(d.clone());
                }
                let is_exc = class_chain_contains_name(
                    &instance.borrow().class,
                    "BaseException",
                );
                Ok(pyrust_builtins::instance_dict::instance_dict(
                    Rc::clone(instance),
                    is_exc,
                ))
            }
            ValueKind::PyModule(module) => {
                let mut dict: PyDict = PyDict::default();
                for (k, v) in module.borrow().attrs.iter() {
                    // Skip Value::unset() tombstones (deletion markers for
                    // synthetic dunders written by delete_attr).
                    if !v.is_unset() {
                        dict.insert(PyKey::str_from(k), v.clone());
                    }
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

    /// CPython: exec(source[, globals[, locals]]) — execute Python source code.
    /// <https://docs.python.org/3/library/functions.html#exec>
    ///
    /// `source` may be a string or a code object returned by `compile()`.
    /// When `globals` and `locals` are omitted the code runs in the current
    /// interpreter's module namespace (assignments become module globals).
    /// When an explicit `globals` dict is supplied the code runs in that
    /// namespace.  Returns `None`.
    fn exec(args) -> Result<Value> {
        let (source_val, globals_opt, locals_opt) = parse_exec_eval_args(FN_NAME, args)?;
        // Inject `__builtins__` into a caller-supplied globals dict, matching
        // CPython's PyEval_EvalCode behaviour.
        if let Some(g) = &globals_opt {
            inject_builtins_into_globals(g);
        }
        // Code object path (from compile()).
        if let Some(result) = crate::interpreter::with_code_state(&source_val, |cs| {
            use crate::interpreter::CodeMode;
            match cs.mode {
                CodeMode::Exec => {
                    _interp.run_exec_code(
                        Rc::clone(&cs.code),
                        Rc::clone(&cs.local_index),
                        globals_opt.clone(),
                        locals_opt.clone(),
                    )
                }
                CodeMode::Eval => {
                    // exec() can take an eval-mode code object too (CPython allows it).
                    _interp.run_eval_code_dispatch(
                        Rc::clone(&cs.code),
                        Rc::clone(&cs.local_index),
                        globals_opt.clone(),
                        locals_opt.clone(),
                    ).map(|_| ())
                }
            }
        }) {
            result?;
            return Ok(Value::none());
        }
        // String path.
        let source = source_val.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() arg 1 must be a string or code object, not '{}'",
                    value_type_name_str(&source_val),
                ),
            )
        })?;
        _interp.exec_source(source, globals_opt, locals_opt)?;
        Ok(Value::none())
    }

    /// CPython: eval(expression[, globals[, locals]]) — evaluate a Python
    /// expression string and return its value.
    /// <https://docs.python.org/3/library/functions.html#eval>
    fn eval(args) -> Result<Value> {
        let (source_val, globals_opt, locals_opt) = parse_exec_eval_args(FN_NAME, args)?;
        // Inject `__builtins__` into a caller-supplied globals dict, matching
        // CPython's PyEval_EvalCode behaviour.
        if let Some(g) = &globals_opt {
            inject_builtins_into_globals(g);
        }
        // Code object path (from compile()).
        if let Some(result) = crate::interpreter::with_code_state(&source_val, |cs| {
            _interp.run_eval_code_dispatch(
                Rc::clone(&cs.code),
                Rc::clone(&cs.local_index),
                globals_opt.clone(),
                locals_opt.clone(),
            )
        }) {
            return result;
        }
        // String path.
        let source = source_val.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() arg 1 must be a string or code object, not '{}'",
                    value_type_name_str(&source_val),
                ),
            )
        })?;
        _interp.eval_source(source, globals_opt, locals_opt)
    }

    /// CPython: compile(source, filename, mode, ...) — compile source to a code
    /// object.
    /// <https://docs.python.org/3/library/functions.html#compile>
    ///
    /// pyrust stores the compiled `FnCode` wrapped in a `Value`; the returned
    /// value can be passed to `exec()` or `eval()`.  Only the `"exec"` and
    /// `"eval"` modes are supported; `"single"` raises `NotImplementedError`.
    fn compile(args) -> Result<Value> {
        // Reject keyword arguments — CPython accepts them but we keep it simple.
        if args.iter().any(|a| a.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() does not accept keyword arguments"),
            ));
        }
        if args.len() < 3 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() requires at least 3 arguments ({} given)",
                    args.len()
                ),
            ));
        }
        let source_val = &args[0].value;
        let source = source_val.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() arg 1 must be a string, not '{}'",
                    value_type_name_str(source_val),
                ),
            )
        })?;
        // filename (arg 2) — CPython tags the resulting code object's
        // `co_filename` with it, so an exception raised inside the compiled code
        // reports this name in its traceback (#2438).  Non-string filenames
        // (CPython also accepts bytes / path-like) fall back to `<unknown>`.
        let compile_filename = args[1]
            .value
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let mode_val = &args[2].value;
        let mode = mode_val.as_str().ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() arg 3 must be a string, not '{}'",
                    value_type_name_str(mode_val),
                ),
            )
        })?;
        match mode {
            "exec" => {
                // Thread the lexer line table through so errors inside the
                // compiled code report correct internal line numbers (#2245).
                let (program, linenos) =
                    crate::interpreter::Interpreter::parse_source_to_stmts_with_linenos(source)?;
                let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
                let local_names =
                    crate::interpreter::collect_local_names(&[], &program, &empty, &empty);
                const MAX_SCRIPT_LOCALS: usize = 200;
                let local_index: Rc<std::collections::HashMap<String, crate::bytecode::Reg>> =
                    if local_names.len() <= MAX_SCRIPT_LOCALS {
                        Rc::new(
                            (0u32..)
                                .zip(local_names.iter())
                                .map(|(i, n)| (n.clone(), i))
                                .collect(),
                        )
                    } else {
                        Rc::new(std::collections::HashMap::new())
                    };
                let code = crate::compiler::compile_script_with_linenos(
                    &program,
                    Rc::clone(&local_index),
                    false,
                    &linenos,
                    &compile_filename,
                )
                .map(|c| Rc::new(crate::optimizer::optimize(c)))?;
                Ok(crate::interpreter::value_code_object(
                    code,
                    crate::interpreter::CodeMode::Exec,
                    local_index,
                ))
            }
            "eval" => {
                let trimmed = source.trim();
                let (program, linenos) =
                    crate::interpreter::Interpreter::parse_source_to_stmts_with_linenos(trimmed)?;
                let local_index: Rc<std::collections::HashMap<String, crate::bytecode::Reg>> =
                    Rc::new(std::collections::HashMap::new());
                let code = crate::compiler::compile_eval_expr_with_linenos(
                    &program,
                    Rc::clone(&local_index),
                    &linenos,
                    &compile_filename,
                )
                .map(|c| Rc::new(crate::optimizer::optimize(c)))?;
                Ok(crate::interpreter::value_code_object(
                    code,
                    crate::interpreter::CodeMode::Eval,
                    local_index,
                ))
            }
            "single" => Err(PyError::named(
                "NotImplementedError",
                "compile() mode 'single' is not yet implemented".to_string(),
            )),
            other => Err(PyError::named(
                "ValueError",
                format!("compile() mode must be 'exec', 'eval' or 'single', not {other:?}"),
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
        if args.is_empty() {
            let mut names: Vec<String> =
                _interp.env.borrow().values.keys().map(String::from).collect();
            names.sort();
            names.dedup();
            return Ok(Value::list(names.into_iter().map(Value::string).collect()));
        }
        // Honour a user-defined `__dir__` override.  CPython's `dir(obj)` is
        // `sorted(type(obj).__dir__(obj))`: it accepts any iterable result,
        // sorts it via the elements' own comparison (so a non-str element only
        // errors if the `<` comparison fails), and does NOT dedup the custom
        // result.  Only `PyInstance` values can carry an overridden `__dir__`;
        // primitives use the default `dir_names` path (issue #1941).
        if let ValueKind::PyInstance(inst) = args[0].value.kind() {
            let class = Rc::clone(&inst.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__dir__") {
                let result =
                    invoke_class_method(_interp, method_val, args[0].value.clone(), &[])?;
                let mut items = _interp.collect_iterable(&result)?;
                // Sort the collected values exactly as `sorted()` would,
                // surfacing the element comparison error verbatim.
                let mut sort_err: Option<PyError> = None;
                let has_instance = items
                    .iter()
                    .any(|v| matches!(v.kind(), ValueKind::PyInstance(_)));
                if has_instance {
                    items.sort_by(|a, b| {
                        if sort_err.is_some() {
                            return std::cmp::Ordering::Equal;
                        }
                        match _interp.richcmp_order(a, b) {
                            Ok(ord) => ord,
                            Err(e) => {
                                sort_err = Some(e);
                                std::cmp::Ordering::Equal
                            }
                        }
                    });
                } else {
                    items.sort_by(|a, b| {
                        if sort_err.is_some() {
                            return std::cmp::Ordering::Equal;
                        }
                        match compare_values(a, b) {
                            Ok(ord) => ord,
                            Err(e) => {
                                sort_err = Some(e);
                                std::cmp::Ordering::Equal
                            }
                        }
                    });
                }
                if let Some(e) = sort_err {
                    return Err(e);
                }
                return Ok(Value::list(items));
            }
        }
        let mut names: Vec<String> = dir_names(&args[0].value);
        names.sort();
        names.dedup();
        Ok(Value::list(names.into_iter().map(Value::string).collect()))
    }

    /// CPython: len(s) — number of items in a container.
    /// <https://docs.python.org/3/library/functions.html#len>
    /// Not marked `#[pure]`: the `PyInstance` arm dispatches user `__len__` via
    /// `invoke_class_method`, which can run arbitrary user code (issue #1526).
    fn len(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes exactly one argument ({} given)", args.len()),
            ));
        }
        let value = &args[0].value;
        let size = match value.kind() {
            ValueKind::Str(text) => text.chars().count() as i64,
            ValueKind::List(items) => items.len() as i64,
            ValueKind::Tuple(items) => items.len() as i64,
            ValueKind::Set(items) => items.len() as i64,
            ValueKind::Bytes(rc) => rc.len() as i64,
            ValueKind::Dict(items) => items.len() as i64,
            ValueKind::Range { start, stop, step } => range_len(start, stop, step),
            // Arbitrary-precision range (#2118): the *length* must still fit in a
            // Py_ssize_t (i64), matching CPython's `range.__len__` which raises
            // `OverflowError: cannot fit 'int' into an index-sized integer` when
            // it does not — even though the bounds themselves may be big.
            ValueKind::BigRange { start, stop, step } => {
                match pyrust_core::bigrange_len(start, stop, step).to_i64() {
                    Some(n) => n,
                    None => {
                        return Err(PyError::named(
                            "OverflowError",
                            "Python int too large to convert to C ssize_t".to_string(),
                        ));
                    }
                }
            }
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
                // Always check for a user-defined __len__ override first.
                // Only fall back to backing primitive data when the class
                // does not define __len__. This matches CPython's dunder
                // dispatch semantics (issue #1448).
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
                        ValueKind::Int(_) => {
                            return Err(PyError::named(
                                "ValueError",
                                "__len__() should return >= 0".to_string(),
                            ))
                        }
                        ValueKind::Bool(b) => if b { 1 } else { 0 },
                        ValueKind::BigInt(b) => {
                            use num_bigint::Sign;
                            if b.sign() == Sign::Minus {
                                return Err(PyError::named(
                                    "ValueError",
                                    "__len__() should return >= 0".to_string(),
                                ));
                            }
                            return Err(PyError::named(
                                "OverflowError",
                                "cannot fit 'int' into an index-sized integer".to_string(),
                            ));
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "'{}' object cannot be interpreted as an integer",
                                    pyrust_core::builtin_type_name(&result),
                                ),
                            ))
                        }
                    }
                } else if let Some(backing) = instance_builtin_data(&inst_rc) {
                    // No user __len__: delegate to backing primitive for
                    // dict/list/set/frozenset/tuple subclasses constructed by
                    // call_class_expanded (issue #976/#994).
                    match backing.kind() {
                        ValueKind::Str(text) => text.chars().count() as i64,
                        ValueKind::Bytes(rc) => rc.len() as i64,
                        ValueKind::List(items) => items.len() as i64,
                        ValueKind::Dict(items) => items.len() as i64,
                        ValueKind::Set(items) => items.len() as i64,
                        ValueKind::Tuple(items) => items.len() as i64,
                        ValueKind::BuiltinObject { ops, state }
                            if ops.type_name() == "frozenset" =>
                        {
                            match ops.len(state) {
                                Some(n) => n as i64,
                                None => {
                                    return Err(PyError::named(
                                        "TypeError",
                                        format!(
                                            "object of type '{}' has no len()",
                                            inst_rc.borrow().class.borrow().name,
                                        ),
                                    ));
                                }
                            }
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "object of type '{}' has no len()",
                                    inst_rc.borrow().class.borrow().name,
                                ),
                            ));
                        }
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
            ValueKind::PyClass(cls)
                if crate::interpreter::metaclass_dunder(cls, "__len__").is_some() =>
            {
                // A class whose metaclass defines `__len__` (e.g. an `Enum`
                // subclass under `EnumMeta`): `len(Color)` dispatches the
                // metaclass slot with the class as the receiver (#2611).
                let method_val = crate::interpreter::metaclass_dunder(cls, "__len__").unwrap();
                let result =
                    invoke_class_method(_interp, method_val, value.clone(), &[])?;
                match result.kind() {
                    ValueKind::Int(n) if n >= 0 => n,
                    ValueKind::Int(_) => {
                        return Err(PyError::named(
                            "ValueError",
                            "__len__() should return >= 0".to_string(),
                        ))
                    }
                    ValueKind::Bool(b) => if b { 1 } else { 0 },
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "'{}' object cannot be interpreted as an integer",
                                pyrust_core::builtin_type_name(&result),
                            ),
                        ))
                    }
                }
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "object of type '{}' has no len()",
                        pyrust_core::builtin_type_name(value),
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
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 1 argument, got 0"),
            ));
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
            .map(|a| a.value.clone())
            .filter(|v| !v.is_none());
        let positional: Vec<&ExpandedCallArg> = args.iter()
            .filter(|a| a.name.is_none())
            .collect();
        if positional.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 1 argument, got {}", positional.len()),
            ));
        }
        let mut items = _interp.collect_iterable(&positional[0].value)?;
        if let Some(kfn) = key_fn {
            let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(items.len());
            for v in std::mem::take(&mut items) {
                let k = _interp.call_function_expanded(
                    kfn.clone(),
                    &[ExpandedCallArg { name: None, value: v.clone() }],
                )?;
                keyed.push((k, v));
            }
            // Pre-scan: if no key is a PyInstance, all comparisons are
            // primitive — skip the interpreter-dispatch overhead entirely.
            let has_instance =
                keyed.iter().any(|(k, _)| matches!(k.kind(), ValueKind::PyInstance(_)));
            let mut sort_err: Option<PyError> = None;
            if has_instance {
                keyed.sort_by(|(a, _), (b, _)| {
                    if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    match _interp.richcmp_order(lhs, rhs) {
                        Ok(ord) => ord,
                        Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                    }
                });
            } else {
                keyed.sort_by(|(a, _), (b, _)| {
                    if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    match compare_values(lhs, rhs) {
                        Ok(ord) => ord,
                        Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                    }
                });
            }
            if let Some(e) = sort_err { return Err(e); }
            // Reuse the `items` buffer (now empty after `take`) to avoid
            // a fresh allocation when extracting values from the keyed pairs.
            items.extend(keyed.into_iter().map(|(_, v)| v));
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
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    match _interp.richcmp_order(lhs, rhs) {
                        Ok(ord) => ord,
                        Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                    }
                });
            } else {
                items.sort_by(|a, b| {
                    if sort_err.is_some() { return std::cmp::Ordering::Equal; }
                    let (lhs, rhs) = if reverse { (b, a) } else { (a, b) };
                    match compare_values(lhs, rhs) {
                        Ok(ord) => ord,
                        Err(e) => { sort_err = Some(e); std::cmp::Ordering::Equal }
                    }
                });
            }
            if let Some(e) = sort_err { return Err(e); }
        }
        // `reverse=True` is applied by inverting the comparison inside the
        // stable `sort_by` (operand swap above), matching `list.sort`'s
        // `sort_by_cmp`.  A trailing `items.reverse()` would flip equal
        // runs too and break stability (see #1904).
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

    /// CPython: round(number, ndigits=None) — banker's rounding.
    /// <https://docs.python.org/3/library/functions.html#round>
    ///
    /// Both `number` and `ndigits` are keyword-or-positional in CPython 3.12
    /// (#2180), so this uses the raw-`args` form + `bind_constructor_kwargs`
    /// rather than the typed-signature dialect (whose unknown-keyword wording
    /// differs from the C-clinic "invalid keyword argument" message round
    /// emits).  `x`/`ndigits` are resolved to `PyValue` so the body can
    /// dispatch on `ValueKind` for full CPython parity: `bool ⊆ int` (both
    /// round unchanged), `float` uses half-even rounding, and everything else
    /// raises `TypeError`.
    fn round(args) -> Result<Value> {
        // CPython 3.12: round(number, ndigits=None) — both `number` and
        // `ndigits` are keyword-or-positional.  Bind kwargs (with the
        // C-clinic "invalid keyword argument" wording) before the existing
        // positional dispatch below.
        //
        // Unlike the int/complex/str constructors, `round` is an argument-clinic
        // function whose *missing required positional* check precedes the
        // unknown-keyword check: `round(x=3.5)` / `round(foo=1)` report
        // "missing required argument 'number'", not "'x'/'foo' is an invalid
        // keyword argument".  The arity-overflow check still comes first, so
        // mirror CPython's exact order: arity, then missing-number, then the
        // generic keyword binding (which surfaces the remaining errors).
        if args.len() > 2 {
            let noun = if args.iter().all(|a| a.name.is_some()) {
                "keyword arguments"
            } else {
                "arguments"
            };
            return Err(PyError::named(
                "TypeError",
                format!("round() takes at most 2 {noun} ({} given)", args.len()),
            ));
        }
        let number_bound = args
            .iter()
            .any(|a| a.name.is_none() || a.name.as_deref() == Some("number"));
        if !number_bound {
            return Err(PyError::named(
                "TypeError",
                "round() missing required argument 'number' (pos 1)".to_string(),
            ));
        }
        let bound =
            bind_constructor_kwargs(FN_NAME, args, &["number", "ndigits"], &[true, true], 2)?;
        // `number` was confirmed bound above; the slot is always populated here.
        let x: PyValue = PyValue(bound[0].clone().expect("number slot bound"));
        let ndigits: Option<PyValue> = bound[1].clone().map(PyValue);
        // Classify x first so we can decide whether to validate ndigits.
        // CPython forwards any ndigits type to user-defined __round__ without
        // pre-validating it (round(obj, 3.5) passes 3.5 to __round__), but
        // raises TypeError for non-int ndigits on all primitive types.
        enum NumKind { Int(i64), Bool(bool), BigInt, Float(f64), Other }
        let num = match x.0.kind() {
            ValueKind::Int(v) => NumKind::Int(v),
            ValueKind::Bool(b) => NumKind::Bool(b),
            ValueKind::BigInt(_) => NumKind::BigInt,
            ValueKind::Float(v) => NumKind::Float(v),
            _ => NumKind::Other,
        };
        // Validate ndigits type for primitive dispatches only.
        let ndigits_i32: Option<i32> = if matches!(num, NumKind::Other) {
            // Deferred: for user objects ndigits is passed as-is to __round__.
            None
        } else {
            match ndigits {
                None => None,
                Some(ref v) => match v.0.kind() {
                    // Clamp to -(i32::MAX)..=i32::MAX to avoid both the as-i32 wrap
                    // for large positive values and the -n overflow for i32::MIN.
                    // Out-of-range values land in the is_infinite() overflow guards
                    // in the float branch.
                    ValueKind::Int(n) => Some(n.clamp(-(i32::MAX as i64), i32::MAX as i64) as i32),
                    ValueKind::Bool(b) => Some(b as i32),
                    ValueKind::None => None,
                    // BigInt ndigits: a BigInt by definition doesn't fit in i64, so
                    // its magnitude always exceeds i32::MAX.  Clamp to ±i32::MAX
                    // directly based on sign.  CPython accepts any integer type for
                    // ndigits; the float branch then returns the value unchanged for
                    // very large positive ndigits, and 0.0 for very large negative ones.
                    ValueKind::BigInt(b) => {
                        use num_bigint::Sign;
                        Some(if b.sign() == Sign::Minus { -(i32::MAX) } else { i32::MAX })
                    }
                    // CPython calls operator.index() on any non-int ndigits.
                    // For int subclasses, coerce_numeric extracts the backing
                    // primitive (no __index__ call needed — the subclass IS an
                    // int).  For other objects with __index__, call it.
                    ValueKind::PyInstance(_) => {
                        let coerced = coerce_numeric(&v.0);
                        let result = match coerced.kind() {
                            ValueKind::Int(_) | ValueKind::BigInt(_) => coerced.clone(),
                            _ => {
                                // Not an int subclass; try user-defined __index__.
                                let inst_rc = match v.0.as_py_instance_rc() {
                                    Some(rc) => Rc::clone(rc),
                                    None => return Err(PyError::named(
                                        "TypeError",
                                        format!(
                                            "'{}' object cannot be interpreted as an integer",
                                            value_type_name_str(&v.0),
                                        ),
                                    )),
                                };
                                let class = Rc::clone(&inst_rc.borrow().class);
                                let Some(method_val) = lookup_class_attr(&class, "__index__") else {
                                    return Err(PyError::named(
                                        "TypeError",
                                        format!(
                                            "'{}' object cannot be interpreted as an integer",
                                            value_type_name_str(&v.0),
                                        ),
                                    ));
                                };
                                invoke_class_method(
                                    _interp,
                                    method_val,
                                    Value::py_instance(inst_rc),
                                    &[],
                                )?
                            }
                        };
                        // CPython's operator.index() accepts __index__ returning
                        // an int subclass (with a DeprecationWarning in 3.12).
                        // Coerce PyInstance results so int subclasses are unwrapped.
                        let result = if matches!(result.kind(), ValueKind::PyInstance(_)) {
                            coerce_numeric(&result)
                        } else {
                            result
                        };
                        match result.kind() {
                            ValueKind::Int(n) => Some(n.clamp(-(i32::MAX as i64), i32::MAX as i64) as i32),
                            ValueKind::Bool(b) => Some(b as i32),
                            ValueKind::BigInt(b) => {
                                use num_bigint::Sign;
                                Some(if b.sign() == Sign::Minus { -(i32::MAX) } else { i32::MAX })
                            }
                            _ => return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__index__ returned non-int (type {})",
                                    value_type_name_str(&result),
                                ),
                            )),
                        }
                    }
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object cannot be interpreted as an integer",
                            value_type_name_str(&v.0),
                        ),
                    )),
                },
            }
        };
        match num {
            NumKind::Int(v) => match ndigits_i32 {
                Some(n) if n < 0 => Ok(round_bigint_neg_ndigits(PyBigInt::from(v), (-n) as u32)),
                _ => Ok(Value::int(v)),
            },
            NumKind::Bool(b) => {
                let v: i64 = if b { 1 } else { 0 };
                match ndigits_i32 {
                    Some(n) if n < 0 => Ok(round_bigint_neg_ndigits(PyBigInt::from(v), (-n) as u32)),
                    _ => Ok(Value::int(v)),
                }
            }
            NumKind::BigInt => {
                if let ValueKind::BigInt(b) = x.0.kind() {
                    match ndigits_i32 {
                        Some(n) if n < 0 => {
                            Ok(round_bigint_neg_ndigits(b.clone(), (-n) as u32))
                        }
                        _ => Ok(Value::bigint(b.clone())),
                    }
                } else {
                    unreachable!()
                }
            }
            NumKind::Float(v) => match ndigits_i32 {
                None => Ok(Value::int(py_round_half_even_checked(v)?)),
                Some(n) => round_float_ndigits(v, n),
            },
            NumKind::Other => {
                // Check for user-defined __round__ before raising TypeError.
                // CPython does not validate the ndigits type for user objects;
                // ndigits is forwarded as-is to __round__.
                if let ValueKind::PyInstance(inst) = x.0.kind() {
                    let inst_rc = Rc::clone(inst);
                    let class = Rc::clone(&inst_rc.borrow().class);
                    if let Some(method_val) = lookup_class_attr(&class, "__round__") {
                        // CPython: __round__() is called with no args when ndigits
                        // is absent or None; otherwise the original ndigits value
                        // (which may be a bool) is forwarded as-is.
                        let call_args: Vec<ExpandedCallArg> = match ndigits {
                            None => vec![],
                            Some(ref v) if matches!(v.0.kind(), ValueKind::None) => vec![],
                            Some(ref v) => vec![ExpandedCallArg {
                                name: None,
                                value: v.0.clone(),
                            }],
                        };
                        return invoke_class_method(
                            _interp,
                            method_val,
                            Value::py_instance(inst_rc),
                            &call_args,
                        );
                    }
                    // No user-defined __round__ found: if this is an int/float
                    // subclass, coerce to the backing primitive and re-dispatch
                    // using the same rounding logic as the primitive arms above.
                    // This matches CPython's inherited int.__round__ / float.__round__
                    // behaviour for subclasses that don't override __round__.
                    let coerced = coerce_numeric(&x.0);
                    let ndigits_i32_coerced: Option<i32> = match ndigits {
                        None => None,
                        Some(ref v) => match v.0.kind() {
                            // Clamp to -(i32::MAX)..=i32::MAX for the same reason as the
                            // primary ndigits_i32 path above.
                            ValueKind::Int(n) => Some(n.clamp(-(i32::MAX as i64), i32::MAX as i64) as i32),
                            ValueKind::Bool(b) => Some(b as i32),
                            ValueKind::None => None,
                            // BigInt ndigits: same logic as the primary path above.
                            ValueKind::BigInt(b) => {
                                use num_bigint::Sign;
                                Some(if b.sign() == Sign::Minus { -(i32::MAX) } else { i32::MAX })
                            }
                            // int subclass / __index__ object: same coerce_numeric +
                            // __index__ protocol as the primary ndigits_i32 path above.
                            ValueKind::PyInstance(_) => {
                                let coerced_nd = coerce_numeric(&v.0);
                                let result = match coerced_nd.kind() {
                                    ValueKind::Int(_) | ValueKind::BigInt(_) => coerced_nd.clone(),
                                    _ => {
                                        let inst_rc = match v.0.as_py_instance_rc() {
                                            Some(rc) => Rc::clone(rc),
                                            None => return Err(PyError::named(
                                                "TypeError",
                                                format!(
                                                    "'{}' object cannot be interpreted as an integer",
                                                    value_type_name_str(&v.0),
                                                ),
                                            )),
                                        };
                                        let class = Rc::clone(&inst_rc.borrow().class);
                                        let Some(method_val) = lookup_class_attr(&class, "__index__") else {
                                            return Err(PyError::named(
                                                "TypeError",
                                                format!(
                                                    "'{}' object cannot be interpreted as an integer",
                                                    value_type_name_str(&v.0),
                                                ),
                                            ));
                                        };
                                        invoke_class_method(
                                            _interp,
                                            method_val,
                                            Value::py_instance(inst_rc),
                                            &[],
                                        )?
                                    }
                                };
                                // CPython's operator.index() accepts __index__
                                // returning an int subclass (DeprecationWarning in 3.12).
                                let result = if matches!(result.kind(), ValueKind::PyInstance(_)) {
                                    coerce_numeric(&result)
                                } else {
                                    result
                                };
                                match result.kind() {
                                    ValueKind::Int(n) => Some(n.clamp(-(i32::MAX as i64), i32::MAX as i64) as i32),
                                    ValueKind::Bool(b) => Some(b as i32),
                                    ValueKind::BigInt(b) => {
                                        use num_bigint::Sign;
                                        Some(if b.sign() == Sign::Minus { -(i32::MAX) } else { i32::MAX })
                                    }
                                    _ => return Err(PyError::named(
                                        "TypeError",
                                        format!(
                                            "__index__ returned non-int (type {})",
                                            value_type_name_str(&result),
                                        ),
                                    )),
                                }
                            }
                            _ => return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "'{}' object cannot be interpreted as an integer",
                                    value_type_name_str(&v.0),
                                ),
                            )),
                        },
                    };
                    match coerced.kind() {
                        ValueKind::Int(v) => return match ndigits_i32_coerced {
                            Some(n) if n < 0 => Ok(round_bigint_neg_ndigits(PyBigInt::from(v), (-n) as u32)),
                            _ => Ok(Value::int(v)),
                        },
                        ValueKind::BigInt(b) => return match ndigits_i32_coerced {
                            Some(n) if n < 0 => Ok(round_bigint_neg_ndigits(b.clone(), (-n) as u32)),
                            _ => Ok(Value::bigint(b.clone())),
                        },
                        ValueKind::Float(v) => return match ndigits_i32_coerced {
                            None => Ok(Value::int(py_round_half_even_checked(v)?)),
                            Some(n) => round_float_ndigits(v, n),
                        },
                        _ => {}
                    }
                }
                Err(PyError::named(
                    "TypeError",
                    format!("type {} doesn't define __round__ method", value_type_name_str(&x.0)),
                ))
            }
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
            1 => Ok(Value::list(_interp.collect_iterable(&args[0].value)?)),
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 1 argument, got {}", args.len()),
            )),
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
            1 => Ok(Value::tuple(_interp.collect_iterable(&args[0].value)?)),
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 1 argument, got {}", args.len()),
            )),
        }
    }

    /// CPython: bytes() — bytes constructor.
    /// <https://docs.python.org/3/library/functions.html#func-bytes>
    /// Not marked `#[pure]` because the iterable fallback dispatches user
    /// `__iter__` and `__next__` when consuming a general iterable (e.g. range,
    /// generator expressions, user-defined iterables).
    fn bytes(args) -> Result<Value> {
        // CPython 3.12: bytes(source, encoding, errors) — source/encoding/
        // errors are keyword-or-positional.
        let bound = bind_bytes_like_args(FN_NAME, args)?;
        let args = &bound[..];
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
                    // Warm path: every element is a plain int/bool — no clone,
                    // no per-element dispatch.  Only on hitting a PyInstance do
                    // we clone the remaining elements (releasing the cell borrow
                    // so __index__ can't alias the list) and resolve them.
                    let fast = try_fast_bytes_elems(&items)?;
                    match fast {
                        Ok(out) => Ok(Value::bytes(out)),
                        Err((mut out, from)) => {
                            let rest: Vec<Value> = items[from..].to_vec();
                            drop(items);
                            for v in &rest {
                                out.push(bytes_element_to_u8(_interp, v)?);
                            }
                            Ok(Value::bytes(out))
                        }
                    }
                }
                ValueKind::Tuple(items) => {
                    let fast = try_fast_bytes_elems(items)?;
                    match fast {
                        Ok(out) => Ok(Value::bytes(out)),
                        Err((mut out, from)) => {
                            let rest: Vec<Value> = items[from..].to_vec();
                            for v in &rest {
                                out.push(bytes_element_to_u8(_interp, v)?);
                            }
                            Ok(Value::bytes(out))
                        }
                    }
                }
                ValueKind::BigInt(_) => Err(PyError::named(
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer".to_string(),
                )),
                ValueKind::PyInstance(inst) => {
                    // CPython 3.12: check __bytes__ before falling through to the
                    // iterable path. __bytes__ takes priority over __iter__.
                    let inst_rc = Rc::clone(inst);
                    let class = Rc::clone(&inst_rc.borrow().class);
                    // Issue #1204: if the instance is a bytes subclass extract the
                    // backing value first. `bytes(MyBytes(b"x"))` must return b'x'.
                    if let Some(backing) = instance_builtin_data(&inst_rc)
                        && matches!(backing.kind(), ValueKind::Bytes(_)) {
                            return Ok(backing);
                        }
                    let self_val = Value::py_instance(Rc::clone(&inst_rc));
                    if let Some(method) = lookup_class_attr(&class, "__bytes__") {
                        let result = invoke_class_method(_interp, method, self_val, &[])?;
                        return if matches!(result.kind(), ValueKind::Bytes(_)) {
                            Ok(result)
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__bytes__ returned non-bytes (type {})",
                                    value_type_name_str(&result)
                                ),
                            ))
                        };
                    }
                    // No __bytes__: CPython next honors __index__ as the count
                    // form (`bytes(obj)` -> N zero bytes) before __iter__ (#1908).
                    if let Some(count) = bytes_count_via_index(_interp, &args[0].value)? {
                        return Ok(Value::bytes(vec![0u8; count]));
                    }
                    // Otherwise fall through to the iterable path.
                    let type_name = value_type_name_str(&args[0].value).to_string();
                    let items =
                        _interp.collect_iterable(&args[0].value).map_err(|e| {
                            if e.class_name_is("TypeError") {
                                PyError::named(
                                    "TypeError",
                                    format!("cannot convert '{type_name}' object to bytes"),
                                )
                            } else {
                                e
                            }
                        })?;
                    Ok(Value::bytes(bytes_from_items(_interp, items)?))
                }
                _ => {
                    // General iterable fallback: any object supporting __iter__ /
                    // __next__ (range, generators, user-defined iterables, etc.).
                    // Non-iterable arguments produce CPython-compatible
                    // "cannot convert 'X' object to bytes".
                    let type_name = pyrust_core::builtin_type_name(&args[0].value).into_owned();
                    let items =
                        _interp.collect_iterable(&args[0].value).map_err(|e| {
                            if e.class_name_is("TypeError") {
                                PyError::named(
                                    "TypeError",
                                    format!("cannot convert '{type_name}' object to bytes"),
                                )
                            } else {
                                e
                            }
                        })?;
                    Ok(Value::bytes(bytes_from_items(_interp, items)?))
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

    /// CPython: bytearray(...) — mutable bytes constructor.
    /// <https://docs.python.org/3/library/functions.html#func-bytearray>
    /// Mirrors `bytes()` but returns a mutable `bytearray` value.
    fn bytearray(args) -> Result<Value> {
        // CPython 3.12: bytearray(source, encoding, errors) — all three are
        // keyword-or-positional.
        let bound = bind_bytes_like_args(FN_NAME, args)?;
        let args = &bound[..];
        match args.len() {
            0 => Ok(pyrust_builtins::bytearray::bytearray(Vec::new())),
            1 => match args[0].value.kind() {
                ValueKind::Int(n) => {
                    if n < 0 {
                        return Err(PyError::named("ValueError", "negative count".to_string()));
                    }
                    Ok(pyrust_builtins::bytearray::bytearray(vec![0u8; n as usize]))
                }
                ValueKind::Bool(b) => Ok(pyrust_builtins::bytearray::bytearray(vec![0u8; b as usize])),
                ValueKind::Bytes(rc) => Ok(pyrust_builtins::bytearray::bytearray((**rc).clone())),
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME =>
                {
                    // bytearray(bytearray) — copy
                    let snap = pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value)
                        .unwrap_or_default();
                    Ok(pyrust_builtins::bytearray::bytearray(snap))
                }
                ValueKind::Str(_) => Err(PyError::named(
                    "TypeError",
                    "string argument without an encoding".to_string(),
                )),
                ValueKind::BigInt(_) => Err(PyError::named(
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer".to_string(),
                )),
                _ => {
                    // General iterable or PyInstance path. CPython honors the
                    // __index__ count form (`bytearray(obj)` -> N zero bytes)
                    // before falling back to __iter__ (#1908).
                    if let Some(count) = bytes_count_via_index(_interp, &args[0].value)? {
                        return Ok(pyrust_builtins::bytearray::bytearray(vec![0u8; count]));
                    }
                    let type_name = pyrust_core::builtin_type_name(&args[0].value).into_owned();
                    let items = _interp.collect_iterable(&args[0].value).map_err(|e| {
                        if e.class_name_is("TypeError") {
                            PyError::named(
                                "TypeError",
                                format!("cannot convert '{type_name}' object to bytearray"),
                            )
                        } else {
                            e
                        }
                    })?;
                    Ok(pyrust_builtins::bytearray::bytearray(bytes_from_items(
                        _interp, items,
                    )?))
                }
            },
            2 | 3 => {
                // bytearray(source, encoding[, errors])
                let encoding: String = match args[1].value.kind() {
                    ValueKind::Str(s) => s.to_string(),
                    ValueKind::None => return Err(PyError::named(
                        "TypeError",
                        "bytearray() argument 'encoding' must be str, not None".to_string(),
                    )),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!(
                            "bytearray() argument 'encoding' must be str, not {}",
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
                        ValueKind::None => return Err(PyError::named(
                            "TypeError",
                            "bytearray() argument 'errors' must be str, not None".to_string(),
                        )),
                        _ => return Err(PyError::named(
                            "TypeError",
                            format!(
                                "bytearray() argument 'errors' must be str, not {}",
                                value_type_name_str(&args[2].value),
                            ),
                        )),
                    }
                } else {
                    "strict".to_string()
                };
                // Reuse the string encoding logic from bytes, then wrap as bytearray.
                let bytes_val = encode_str_to_bytes(&source, &encoding, &errors)?;
                let data = match bytes_val.kind() {
                    ValueKind::Bytes(rc) => (**rc).clone(),
                    _ => unreachable!("encode_str_to_bytes returns bytes"),
                };
                Ok(pyrust_builtins::bytearray::bytearray(data))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("bytearray() takes at most 3 arguments ({} given)", args.len()),
            )),
        }
    }

    /// CPython: complex(real=0, imag=0) — complex constructor.
    /// <https://docs.python.org/3/library/functions.html#complex>
    /// Not marked `#[pure]` because it dispatches user `__complex__`,
    /// `__float__`, and `__index__` on user-defined objects.
    fn complex(args) -> Result<Value> {
        // CPython 3.12: complex(real=0, imag=0) — both keyword-or-positional.
        let bound = bind_constructor_kwargs(FN_NAME, args, &["real", "imag"], &[true, true], 2)?;
        // Flatten to positional args.  If `imag` is supplied but `real` is
        // omitted (`complex(imag=5)`), CPython treats `real` as its default
        // `0` — fill the interior gap so the two-arg path runs.
        let mut bound_pos: Vec<ExpandedCallArg> = Vec::with_capacity(2);
        if bound[1].is_some() && bound[0].is_none() {
            bound_pos.push(ExpandedCallArg { name: None, value: Value::int(0) });
            bound_pos.push(ExpandedCallArg { name: None, value: bound[1].clone().unwrap() });
        } else {
            for slot in bound.into_iter() {
                match slot {
                    Some(v) => bound_pos.push(ExpandedCallArg { name: None, value: v }),
                    None => break,
                }
            }
        }
        let args = &bound_pos[..];

        // Convert a primitive (non-PyInstance) Value to f64.
        // `type_err_msg` is the full TypeError message to emit for unrecognised kinds.
        let prim_to_f64 = |v: &Value, type_err_msg: &str| -> Result<f64> {
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
                    format!("{type_err_msg}, not '{}'", value_type_name_str(v)),
                )),
            }
        };

        match args.len() {
            0 => Ok(Value::complex(0.0, 0.0)),
            1 => match args[0].value.kind() {
                ValueKind::Complex(re, im) => Ok(Value::complex(re, im)),
                ValueKind::Str(s) => {
                    let (re, im) = parse_complex_str(s).ok_or_else(|| {
                        PyError::named("ValueError", "complex() arg is a malformed string")
                    })?;
                    Ok(Value::complex(re, im))
                }
                ValueKind::PyInstance(inst) => {
                    let inst_rc = Rc::clone(inst);
                    let class = Rc::clone(&inst_rc.borrow().class);
                    let self_val = Value::py_instance(Rc::clone(&inst_rc));
                    // CPython 3.12 dispatch: __complex__ → __float__ → __index__
                    if let Some(method) = lookup_class_attr(&class, "__complex__") {
                        let result = invoke_class_method(_interp, method, self_val, &[])?;
                        return if matches!(result.kind(), ValueKind::Complex(_, _)) {
                            Ok(result)
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__complex__ returned non-complex (type {})",
                                    value_type_name_str(&result)
                                ),
                            ))
                        };
                    }
                    if let Some(method) = lookup_class_attr(&class, "__float__") {
                        let result = invoke_class_method(_interp, method, self_val, &[])?;
                        return if let ValueKind::Float(f) = result.kind() {
                            Ok(Value::complex(f, 0.0))
                        } else {
                            Err(PyError::named(
                                "TypeError",
                                format!(
                                    "{}.__float__ returned non-float (type {})",
                                    class.borrow().name,
                                    value_type_name_str(&result),
                                ),
                            ))
                        };
                    }
                    if let Some(method) = lookup_class_attr(&class, "__index__") {
                        let result = invoke_class_method(_interp, method, self_val, &[])?;
                        return match result.kind() {
                            ValueKind::Int(n) => Ok(Value::complex(n as f64, 0.0)),
                            ValueKind::Bool(b) => Ok(Value::complex(if b { 1.0 } else { 0.0 }, 0.0)),
                            ValueKind::BigInt(b) => {
                                let f = b.to_f64().unwrap_or(f64::INFINITY);
                                if f.is_finite() {
                                    Ok(Value::complex(f, 0.0))
                                } else {
                                    Err(PyError::named(
                                        "OverflowError",
                                        "int too large to convert to float",
                                    ))
                                }
                            }
                            _ => Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__index__ returned non-int (type {})",
                                    value_type_name_str(&result)
                                ),
                            )),
                        };
                    }
                    Err(PyError::named(
                        "TypeError",
                        format!(
                            "complex() first argument must be a string or a number, not '{}'",
                            class.borrow().name
                        ),
                    ))
                }
                _ => Ok(Value::complex(
                    prim_to_f64(&args[0].value, "complex() first argument must be a string or a number")?,
                    0.0,
                )),
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
                // Resolve each arg to a (re, im) pair.
                // First arg: __complex__ yields (re, im); __float__/__index__ → scalar.
                // Second arg: __float__/__index__ only (CPython ignores __complex__ there).
                let first_val = args[0].value.clone();
                let second_val = args[1].value.clone();

                // Helper: call __float__ then __index__ on a PyInstance and return f64.
                // $no_conv_msg is the prefix of the TypeError when no suitable dunder is found;
                // the class name is appended as ", not '<name>'".
                macro_rules! inst_to_f64 {
                    ($inst_rc:expr, $class:expr, $self_val:expr, $no_conv_msg:literal) => {{
                        if let Some(method) = lookup_class_attr(&$class, "__float__") {
                            let result = invoke_class_method(_interp, method, $self_val, &[])?;
                            if let ValueKind::Float(f) = result.kind() {
                                f
                            } else {
                                return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "{}.__float__ returned non-float (type {})",
                                        $class.borrow().name,
                                        value_type_name_str(&result),
                                    ),
                                ));
                            }
                        } else if let Some(method) = lookup_class_attr(&$class, "__index__") {
                            let result = invoke_class_method(_interp, method, $self_val, &[])?;
                            match result.kind() {
                                ValueKind::Int(n) => n as f64,
                                ValueKind::Bool(b) => if b { 1.0 } else { 0.0 },
                                ValueKind::BigInt(b) => {
                                    let f = b.to_f64().unwrap_or(f64::INFINITY);
                                    if f.is_finite() {
                                        f
                                    } else {
                                        return Err(PyError::named(
                                            "OverflowError",
                                            "int too large to convert to float",
                                        ));
                                    }
                                }
                                _ => return Err(PyError::named(
                                    "TypeError",
                                    format!(
                                        "__index__ returned non-int (type {})",
                                        value_type_name_str(&result)
                                    ),
                                )),
                            }
                        } else {
                            return Err(PyError::named(
                                "TypeError",
                                format!("{}, not '{}'", $no_conv_msg, $class.borrow().name),
                            ));
                        }
                    }};
                }

                // Resolve first arg to (re, im) pair.
                let (cr, ci, first_is_complex) = if let ValueKind::Complex(re, im) = first_val.kind() {
                    (re, im, true)
                } else if let ValueKind::PyInstance(inst) = first_val.kind() {
                    let inst_rc = Rc::clone(inst);
                    let class = Rc::clone(&inst_rc.borrow().class);
                    let self_val = Value::py_instance(Rc::clone(&inst_rc));
                    if let Some(method) = lookup_class_attr(&class, "__complex__") {
                        let result = invoke_class_method(_interp, method, self_val, &[])?;
                        if let ValueKind::Complex(re, im) = result.kind() {
                            (re, im, true)
                        } else {
                            return Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__complex__ returned non-complex (type {})",
                                    value_type_name_str(&result)
                                ),
                            ));
                        }
                    } else {
                        let f = inst_to_f64!(
                            inst_rc,
                            class,
                            self_val,
                            "complex() first argument must be a string or a number"
                        );
                        (f, 0.0, false)
                    }
                } else {
                    let f = prim_to_f64(
                        &first_val,
                        "complex() first argument must be a string or a number",
                    )?;
                    (f, 0.0, false)
                };

                // Resolve second arg to (re, im) pair (no __complex__ for second arg).
                let (dr, di, second_is_complex) = if let ValueKind::Complex(re, im) = second_val.kind() {
                    (re, im, true)
                } else if let ValueKind::PyInstance(inst) = second_val.kind() {
                    let inst_rc = Rc::clone(inst);
                    let class = Rc::clone(&inst_rc.borrow().class);
                    let self_val = Value::py_instance(Rc::clone(&inst_rc));
                    let f = inst_to_f64!(
                        inst_rc,
                        class,
                        self_val,
                        "complex() second argument must be a number"
                    );
                    (f, 0.0, false)
                } else {
                    let f = prim_to_f64(&second_val, "complex() second argument must be a number")?;
                    (f, 0.0, false)
                };

                // CPython decomposition formula (Objects/complexobject.c):
                // When at least one arg is complex, apply:
                //   result.real = cr - di
                //   result.imag = ci + dr
                // When neither is complex, assign directly (preserving -0.0 sign).
                if first_is_complex || second_is_complex {
                    Ok(Value::complex(cr - di, ci + dr))
                } else {
                    Ok(Value::complex(cr, dr))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 2 arguments ({} given)", args.len()),
            )),
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
            0 => Ok(Value::set(PySet::default())),
            1 => {
                let items = _interp.collect_iterable(&args[0].value)?;
                let mut set: PySet = PySet::default();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                Ok(Value::set(set))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 1 argument, got {}", args.len()),
            )),
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
            0 => Ok(pyrust_builtins::frozenset::frozenset(PySet::default())),
            1 => {
                // frozenset(frozenset_instance) returns the same object (per CPython).
                if let Some(rc) = pyrust_builtins::frozenset::as_items(&args[0].value) {
                    return Ok(pyrust_builtins::frozenset::frozenset_rc(rc));
                }
                let items = _interp.collect_iterable(&args[0].value)?;
                let mut set: PySet = PySet::default();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                Ok(pyrust_builtins::frozenset::frozenset(set))
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected at most 1 argument, got {}", args.len()),
            )),
        }
    }

    /// CPython: str(object='') — string constructor.
    /// <https://docs.python.org/3/library/functions.html#func-str>
    /// Not marked `#[pure]` because it dispatches user `__str__` and
    /// (as fallback) `__repr__` on user-defined objects.
    fn str(args) -> Result<Value> {
        // CPython 3.12: str(object='', encoding='utf-8', errors='strict') —
        // all three parameters are keyword-or-positional.
        let bound = bind_constructor_kwargs(
            FN_NAME,
            args,
            &["object", "encoding", "errors"],
            &[true, true, true],
            3,
        )?;
        let object = &bound[0];
        let encoding = &bound[1];
        let errors = &bound[2];

        // No object → empty string, regardless of encoding/errors (CPython:
        // `str(encoding='utf-8') == ''`).
        let Some(object) = object else {
            return Ok(Value::string(String::new()));
        };

        // The decoding form is selected when *either* encoding or errors is
        // supplied; otherwise this is the plain `str(object)` form.
        if encoding.is_none() && errors.is_none() {
            // Scalar fast path (#alloc): `str(int)` formats the digits straight
            // into the string Value via a stack buffer — one allocation instead
            // of the intermediate heap `String` that `render_instance_str`
            // returns before `Value::string` copies it.
            if let ValueKind::Int(n) = object.kind() {
                return Ok(Value::int_string(n));
            }
            return Ok(Value::string(render_instance_str(_interp, object)?));
        }

        // str(object, encoding[, errors]) — bytes-to-string decoding form.
        let bytes = match object.kind() {
            ValueKind::Bytes(rc) => rc.as_slice().to_vec(),
            ValueKind::Str(_) => {
                return Err(PyError::named(
                    "TypeError",
                    "decoding str is not supported".to_string(),
                ));
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decoding to str: need a bytes-like object, {} found",
                        pyrust_core::builtin_type_name(object)
                    ),
                ));
            }
        };
        let encoding = match encoding {
            Some(e) => match e.kind() {
                ValueKind::Str(s) => s.to_owned(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() argument 2 (encoding) must be a str"),
                    ));
                }
            },
            None => "utf-8".to_owned(),
        };
        let errors = match errors {
            Some(e) => match e.kind() {
                ValueKind::Str(s) => s.to_owned(),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() argument 3 (errors) must be a str"),
                    ));
                }
            },
            None => "strict".to_owned(),
        };
        pyrust_builtins::bytes::decode_bytes(&bytes, &encoding, &errors)
    }

    /// CPython: int(x=0, base=10) — integer constructor.
    /// <https://docs.python.org/3/library/functions.html#int>
    /// Not marked `#[pure]` because it dispatches user `__int__`, `__index__`,
    /// and `__trunc__` on user-defined objects.
    fn int(args) -> Result<Value> {
        // CPython 3.12: int(x, /, base=10) — `x` positional-only, `base`
        // keyword-or-positional.  `int(x='5')` → invalid-keyword error;
        // `int('10', base=2)` is accepted.
        let bound = bind_constructor_kwargs(FN_NAME, args, &["x", "base"], &[false, true], 2)?;
        // `int(base=2)` (base supplied, value omitted): CPython raises
        // `int() missing string argument`, not the default-0 path.
        if bound[0].is_none() && bound[1].is_some() {
            return Err(PyError::named(
                "TypeError",
                "int() missing string argument".to_string(),
            ));
        }
        // Flatten to positional args (stop at the first unfilled slot — `int`
        // has no interior optional gaps once the missing-value case above is
        // handled).
        let mut bound_pos: Vec<ExpandedCallArg> = Vec::with_capacity(2);
        for slot in bound.into_iter() {
            match slot {
                Some(v) => bound_pos.push(ExpandedCallArg { name: None, value: v }),
                None => break,
            }
        }
        let args = &bound_pos[..];
        match args.len() {
            0 => Ok(Value::int(0)),
            1 => match args[0].value.kind() {
                ValueKind::Int(v) => Ok(Value::int(v)),
                ValueKind::BigInt(b) => Ok(Value::bigint((*b).clone())),
                ValueKind::Float(v) => {
                    if v.is_nan() {
                        return Err(PyError::named(
                            "ValueError",
                            "cannot convert float NaN to integer".to_string(),
                        ));
                    }
                    if v.is_infinite() {
                        return Err(PyError::named(
                            "OverflowError",
                            "cannot convert float infinity to integer".to_string(),
                        ));
                    }
                    let t = v.trunc();
                    if t >= i64::MAX as f64 || t < i64::MIN as f64 {
                        float_to_bigint(t)
                    } else {
                        Ok(Value::int(t as i64))
                    }
                }
                ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                ValueKind::Str(s) => {
                    let trimmed = s.trim();
                    let cleaned = int_strip_explicit_base(trimmed, 10).ok_or_else(|| {
                        PyError::named(
                            "ValueError",
                            format!("invalid literal for int() with base 10: '{s}'"),
                        )
                    })?;
                    pyrust_core::check_int_parse_digits(&cleaned, 10)?;
                    match cleaned.parse::<i64>() {
                        Ok(v) => Ok(Value::int(v)),
                        Err(_) => {
                            // Overflow: try BigInt before giving up.
                            use num_traits::Num as _;
                            crate::value::PyBigInt::from_str_radix(&cleaned, 10)
                                .map(Value::bigint)
                                .map_err(|_| PyError::named(
                                    "ValueError",
                                    format!("invalid literal for int() with base 10: '{s}'"),
                                ))
                        }
                    }
                }
                ValueKind::Bytes(rc) => {
                    int_parse_bytes_like(rc.as_slice(), &args[0].value.repr_raw(), 10)
                }
                ValueKind::PyInstance(inst) => {
                    let inst_rc = Rc::clone(inst);
                    let class = Rc::clone(&inst_rc.borrow().class);
                    // Issue #1204: if the instance is a scalar-primitive subclass
                    // (MyInt, MyFloat, …) extract the backing value first.
                    // `int(MyInt(42))` must return 42, not raise TypeError.
                    if let Some(backing) = instance_builtin_data(&inst_rc) {
                        let result: Option<Result<Value>> = match backing.kind() {
                            ValueKind::Int(v) => Some(Ok(Value::int(v))),
                            ValueKind::BigInt(_) => Some(Ok(backing.clone())),
                            ValueKind::Bool(b) => Some(Ok(Value::int(if b { 1 } else { 0 }))),
                            ValueKind::Float(v) => {
                                if v.is_nan() {
                                    Some(Err(PyError::named(
                                        "ValueError",
                                        "cannot convert float NaN to integer".to_string(),
                                    )))
                                } else if v.is_infinite() {
                                    Some(Err(PyError::named(
                                        "OverflowError",
                                        "cannot convert float infinity to integer".to_string(),
                                    )))
                                } else {
                                    let t = v.trunc();
                                    if t >= i64::MAX as f64 || t < i64::MIN as f64 {
                                        Some(float_to_bigint(t))
                                    } else {
                                        Some(Ok(Value::int(t as i64)))
                                    }
                                }
                            }
                            _ => None,
                        };
                        if let Some(v) = result {
                            return v;
                        }
                    }
                    let self_val = Value::py_instance(Rc::clone(&inst_rc));
                    // CPython 3.12 dispatch: __int__ → __index__ → __trunc__
                    if let Some(method) = lookup_class_attr(&class, "__int__") {
                        let result = invoke_class_method(_interp, method, self_val, &[])?;
                        // CPython normalises bool → int for __int__ return (bool is a
                        // subclass of int, but int() must always return a plain int).
                        if let ValueKind::Bool(b) = result.kind() {
                            return Ok(Value::int(if b { 1 } else { 0 }));
                        }
                        let ok = matches!(result.kind(), ValueKind::Int(_) | ValueKind::BigInt(_));
                        if ok {
                            return Ok(result);
                        }
                        return Err(PyError::named(
                            "TypeError",
                            format!("__int__ returned non-int (type {})", value_type_name_str(&result)),
                        ));
                    }
                    if let Some(method) = lookup_class_attr(&class, "__index__") {
                        let result = invoke_class_method(_interp, method, self_val, &[])?;
                        // Same normalisation as __int__: bool is a valid __index__ return
                        // type (it's an int subclass) but int() must return a plain int.
                        if let ValueKind::Bool(b) = result.kind() {
                            return Ok(Value::int(if b { 1 } else { 0 }));
                        }
                        let ok = matches!(result.kind(), ValueKind::Int(_) | ValueKind::BigInt(_));
                        if ok {
                            return Ok(result);
                        }
                        return Err(PyError::named(
                            "TypeError",
                            format!("__index__ returned non-int (type {})", value_type_name_str(&result)),
                        ));
                    }
                    if let Some(method) = lookup_class_attr(&class, "__trunc__") {
                        // Deprecated since 3.11 but still works in 3.12; call int() on the result.
                        let trunc_result = invoke_class_method(_interp, method, self_val, &[])?;
                        return match trunc_result.kind() {
                            ValueKind::Int(v) => Ok(Value::int(v)),
                            ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
                            ValueKind::BigInt(_) => Ok(trunc_result.clone()),
                            // CPython 3.12: float is not an Integral type — any float returned
                            // from __trunc__ (including inf/nan) raises TypeError, not
                            // OverflowError/ValueError.  The inf/nan guards belong only in the
                            // direct float-to-int conversion paths, not here.
                            _ => Err(PyError::named(
                                "TypeError",
                                format!(
                                    "__trunc__ returned non-Integral (type {})",
                                    value_type_name_str(&trunc_result)
                                ),
                            )),
                        };
                    }
                    Err(PyError::named(
                        "TypeError",
                        format!(
                            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                            class.borrow().name
                        ),
                    ))
                }
                // bytearray (a BuiltinObject) is bytes-like: decode + parse as
                // base-10 ASCII, same as the `bytes` arm above (#2077).  Note
                // CPython's `int()` error uses the *bytes* repr (`b'…'`) even
                // for a bytearray operand, so render from the byte data.
                _ if pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value).is_some() => {
                    let data =
                        pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value).unwrap();
                    let repr = Value::bytes(data.clone()).repr_raw();
                    int_parse_bytes_like(&data, &repr, 10)
                }
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                        value_type_name_str(&args[0].value),
                    ),
                )),
            },
            2 => {
                let base_arg = match args[1].value.kind() {
                    ValueKind::Int(b) if b == 0 || (2..=36).contains(&b) => b,
                    ValueKind::Int(b) => return Err(PyError::named(
                        "ValueError",
                        format!("int() base must be >= 2 and <= 36, or 0, not {b}"),
                    )),
                    _ => return Err(PyError::named(
                        "TypeError",
                        format!("'{}' object cannot be interpreted as an integer", value_type_name_str(&args[1].value)),
                    )),
                };
                match args[0].value.kind() {
                    ValueKind::Str(s) => {
                        let trimmed = s.trim();
                        if base_arg == 0 {
                            let (base, digits) = int_parse_base_zero(trimmed).ok_or_else(|| {
                                PyError::named(
                                    "ValueError",
                                    format!("invalid literal for int() with base 0: '{s}'"),
                                )
                            })?;
                            pyrust_core::check_int_parse_digits(&digits, base)?;
                            match i64::from_str_radix(&digits, base) {
                                Ok(v) => Ok(Value::int(v)),
                                Err(_) => {
                                    // Overflow: try BigInt before giving up.
                                    use num_traits::Num as _;
                                    crate::value::PyBigInt::from_str_radix(&digits, base)
                                        .map(Value::bigint)
                                        .map_err(|_| PyError::named(
                                            "ValueError",
                                            format!("invalid literal for int() with base 0: '{s}'"),
                                        ))
                                }
                            }
                        } else {
                            let base = base_arg as u32;
                            let stripped = int_strip_explicit_base(trimmed, base).ok_or_else(|| {
                                PyError::named(
                                    "ValueError",
                                    format!("invalid literal for int() with base {base_arg}: '{s}'"),
                                )
                            })?;
                            pyrust_core::check_int_parse_digits(&stripped, base)?;
                            match i64::from_str_radix(&stripped, base) {
                                Ok(v) => Ok(Value::int(v)),
                                Err(_) => {
                                    // Overflow: try BigInt before giving up.
                                    use num_traits::Num as _;
                                    crate::value::PyBigInt::from_str_radix(&stripped, base)
                                        .map(Value::bigint)
                                        .map_err(|_| PyError::named(
                                            "ValueError",
                                            format!("invalid literal for int() with base {base_arg}: '{s}'"),
                                        ))
                                }
                            }
                        }
                    }
                    ValueKind::Bytes(rc) => {
                        int_parse_bytes_like(rc.as_slice(), &args[0].value.repr_raw(), base_arg)
                    }
                    // bytearray with explicit base — bytes-like, parse as ASCII
                    // (#2077).  As above, CPython's `int()` error uses the
                    // bytes repr for a bytearray operand.
                    _ if pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value)
                        .is_some() =>
                    {
                        let data = pyrust_builtins::bytearray::as_bytearray_snapshot(
                            &args[0].value,
                        )
                        .unwrap();
                        let repr = Value::bytes(data.clone()).repr_raw();
                        int_parse_bytes_like(&data, &repr, base_arg)
                    }
                    _ => Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() can't convert non-string with explicit base"),
                    )),
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("int() takes at most 2 arguments ({} given)", args.len()),
            )),
        }
    }

    /// CPython: float(x=0.0) — float constructor.
    /// <https://docs.python.org/3/library/functions.html#float>
    /// Not marked `#[pure]` because it dispatches user `__float__` and
    /// `__index__` on user-defined objects.
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
                ValueKind::Str(s) => {
                    let err = || {
                        PyError::named(
                            "ValueError",
                            format!("could not convert string to float: '{s}'"),
                        )
                    };
                    // PEP 515: strip valid underscores, reject invalid placement.
                    let cleaned = pep515_strip_float(s.trim()).ok_or_else(err)?;
                    cleaned.parse::<f64>().map(Value::float).map_err(|_| err())
                }
                // bytes-like: decode as ASCII and parse identically to `str`
                // (#2077).  `bytearray` is a BuiltinObject handled by the
                // `_` guard below.
                ValueKind::Bytes(rc) => {
                    float_parse_bytes_like(rc.as_slice(), &args[0].value.repr_raw())
                }
                ValueKind::PyInstance(inst) => {
                    let inst_rc = Rc::clone(inst);
                    let class = Rc::clone(&inst_rc.borrow().class);
                    // Issue #1204: if the instance is a scalar-primitive subclass
                    // (MyFloat, MyInt, …) extract the backing value first.
                    // `float(MyFloat(3.14))` must return 3.14, not raise TypeError.
                    if let Some(backing) = instance_builtin_data(&inst_rc) {
                        match backing.kind() {
                            ValueKind::Float(v) => return Ok(Value::float(v)),
                            ValueKind::Int(v) => return Ok(Value::float(v as f64)),
                            ValueKind::Bool(b) => {
                                return Ok(Value::float(if b { 1.0 } else { 0.0 }));
                            }
                            ValueKind::BigInt(b) => {
                                return b
                                    .to_f64()
                                    .filter(|f| f.is_finite())
                                    .map(Value::float)
                                    .ok_or_else(|| {
                                        PyError::named(
                                            "OverflowError",
                                            "int too large to convert to float".to_string(),
                                        )
                                    });
                            }
                            _ => {}
                        }
                    }
                    let self_val = Value::py_instance(Rc::clone(&inst_rc));
                    // CPython 3.12 dispatch: __float__ → __index__
                    if let Some(method) = lookup_class_attr(&class, "__float__") {
                        let result = invoke_class_method(_interp, method, self_val, &[])?;
                        let ok = matches!(result.kind(), ValueKind::Float(_));
                        if ok {
                            return Ok(result);
                        }
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "{}.__float__ returned non-float (type {})",
                                class.borrow().name,
                                value_type_name_str(&result),
                            ),
                        ));
                    }
                    if let Some(method) = lookup_class_attr(&class, "__index__") {
                        let result = invoke_class_method(_interp, method, self_val, &[])?;
                        return match result.kind() {
                            ValueKind::Int(v) => Ok(Value::float(v as f64)),
                            ValueKind::Bool(b) => Ok(Value::float(if b { 1.0 } else { 0.0 })),
                            ValueKind::BigInt(b) => {
                                let f = b.to_f64().unwrap_or(f64::INFINITY);
                                if f.is_finite() {
                                    Ok(Value::float(f))
                                } else {
                                    Err(PyError::named("OverflowError", "int too large to convert to float"))
                                }
                            }
                            _ => Err(PyError::named(
                                "TypeError",
                                format!("__index__ returned non-int (type {})", value_type_name_str(&result)),
                            )),
                        };
                    }
                    Err(PyError::named(
                        "TypeError",
                        format!(
                            "float() argument must be a string or a real number, not '{}'",
                            class.borrow().name
                        ),
                    ))
                }
                // bytearray (a BuiltinObject) is bytes-like: decode + parse as
                // ASCII, same as the `bytes` arm above (#2077).
                _ if pyrust_builtins::bytearray::as_bytearray_snapshot(&args[0].value).is_some() => {
                    let (data, repr) = float_bytes_like(&args[0].value).unwrap();
                    float_parse_bytes_like(&data, &repr)
                }
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "float() argument must be a string or a real number, not '{}'",
                        value_type_name_str(&args[0].value),
                    ),
                )),
            },
            _ => Err(PyError::named(
                "TypeError",
                format!("float expected at most 1 argument, got {}", args.len()),
            )),
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
        // Separate positional and keyword args.
        // CPython: dict([mapping_or_iterable], **kwargs)
        let mut pos_args: Vec<&ExpandedCallArg> = Vec::with_capacity(1);
        let mut kw_pairs: Vec<(String, Value)> = Vec::with_capacity(args.len());
        for a in args {
            match &a.name {
                None => pos_args.push(a),
                Some(n) => kw_pairs.push((n.clone(), a.value.clone())),
            }
        }
        if pos_args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME} takes at most 1 positional argument ({} given)",
                    pos_args.len()
                ),
            ));
        }

        let mut result: PyDict =
            PyDict::with_capacity_and_hasher(kw_pairs.len(), Default::default());

        // Process the optional positional argument.
        if let Some(arg) = pos_args.first() {
            match arg.value.kind() {
                ValueKind::Dict(map) => {
                    result.extend(map.clone());
                }
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::mapping_proxy::TYPE_NAME =>
                {
                    if let Some(class_rc) =
                        pyrust_builtins::mapping_proxy::as_class_rc(&arg.value)
                    {
                        let class = class_rc.borrow();
                        for (k, v) in class.attrs.iter() {
                            result.insert(PyKey::str_from(k), v.clone());
                        }
                    } else if let Some(dict_rc) =
                        pyrust_builtins::mapping_proxy::as_dict_rc(&arg.value)
                    {
                        // Dict-backed mappingproxy (`d.keys().mapping`, #2679):
                        // copy the parent dict's key/value pairs verbatim.
                        result.extend(dict_rc.borrow().clone());
                    }
                }
                // PyInstance with a backing dict (dict subclass).
                ValueKind::PyInstance(inst) => {
                    let inst_rc = Rc::clone(inst);
                    let dict_backing = instance_builtin_data(&inst_rc)
                        .and_then(|backing| backing.as_dict().cloned());
                    if let Some(map) = dict_backing {
                        // PyInstance with a backing dict (dict subclass).
                        result.extend(map);
                    } else if is_dict_subclass_instance(&inst_rc) {
                        // Dict subclasses that keep their mapping in a custom
                        // backing attr rather than `__builtin_data__` — e.g.
                        // `collections.Counter` / `defaultdict` (issue #2010).
                        // CPython's `dict(mapping)` reads via `keys()` +
                        // `__getitem__`; we iterate the keys and subscript via
                        // the class's `__getitem__`.
                        let class = Rc::clone(&inst_rc.borrow().class);
                        let getitem = lookup_class_attr(&class, "__getitem__");
                        let keys = _interp.collect_iterable(&arg.value)?;
                        for k in keys {
                            let v = match getitem.clone() {
                                Some(m) => invoke_class_method(
                                    _interp,
                                    m,
                                    Value::py_instance(Rc::clone(&inst_rc)),
                                    &[ExpandedCallArg { name: None, value: k.clone() }],
                                )?,
                                None => {
                                    return Err(PyError::named(
                                        "TypeError",
                                        format!(
                                            "{FN_NAME}() argument must be a mapping or iterable"
                                        ),
                                    ));
                                }
                            };
                            let key = _interp.value_to_pykey(&k)?;
                            _interp.dict_insert(&mut result, key, v)?;
                        }
                    } else if let Some(pairs) =
                        mapping_pairs_via_protocol(_interp, &arg.value)?
                    {
                        // Any non-dict mapping that follows the duck-typed
                        // protocol (`keys()` + `__getitem__`): `ChainMap`,
                        // `UserDict`, custom mappings (issue #2190).
                        for (key, v) in pairs {
                            _interp.dict_insert(&mut result, key, v)?;
                        }
                    } else {
                        // Treat as iterable of (key, value) pairs.
                        let pairs = _interp.collect_iterable(&arg.value)?;
                        for (idx, pair) in pairs.into_iter().enumerate() {
                            let items = _interp.collect_iterable(&pair).map_err(|e| {
                                // A non-iterable element maps to CPython's
                                // "cannot convert ... to a sequence" TypeError;
                                // an error raised *inside* the element's own
                                // iteration (e.g. a user `__iter__` raising)
                                // propagates unchanged.
                                if is_not_iterable_error(&e) {
                                    PyError::named(
                                        "TypeError",
                                        format!(
                                            "cannot convert dictionary update sequence element #{idx} to a sequence"
                                        ),
                                    )
                                } else {
                                    e
                                }
                            })?;
                            if items.len() != 2 {
                                return Err(PyError::named(
                                    "ValueError",
                                    format!(
                                        "dictionary update sequence element #{idx} has length {}; 2 is required",
                                        items.len()
                                    ),
                                ));
                            }
                            let key = _interp.value_to_pykey(&items[0])?;
                            // #1914: dedup `PyKey::Object` keys via user `__eq__`.
                            _interp.dict_insert(&mut result, key, items[1].clone())?;
                        }
                    }
                }
                _ => {
                    // Treat as iterable of (key, value) pairs.
                    let pairs = _interp.collect_iterable(&arg.value)?;
                    for (idx, pair) in pairs.into_iter().enumerate() {
                        let items = _interp.collect_iterable(&pair).map_err(|e| {
                            // A non-iterable element maps to CPython's
                            // "cannot convert ... to a sequence" TypeError;
                            // an error raised *inside* the element's own
                            // iteration (e.g. a user `__iter__` raising)
                            // propagates unchanged.
                            if is_not_iterable_error(&e) {
                                PyError::named(
                                    "TypeError",
                                    format!(
                                        "cannot convert dictionary update sequence element #{idx} to a sequence"
                                    ),
                                )
                            } else {
                                e
                            }
                        })?;
                        if items.len() != 2 {
                            return Err(PyError::named(
                                "ValueError",
                                format!(
                                    "dictionary update sequence element #{idx} has length {}; 2 is required",
                                    items.len()
                                ),
                            ));
                        }
                        let key = _interp.value_to_pykey(&items[0])?;
                        // #1914: dedup `PyKey::Object` keys via user `__eq__`.
                        _interp.dict_insert(&mut result, key, items[1].clone())?;
                    }
                }
            }
        }

        // Apply keyword arguments.
        for (name, value) in kw_pairs {
            result.insert(PyKey::str_from(&name), value);
        }

        Ok(Value::dict(result))
    }

    /// CPython: input([prompt]) — read a line from stdin, stripping the trailing newline.
    /// <https://docs.python.org/3/library/functions.html#input>
    ///
    /// Accepts 0 or 1 positional argument (the prompt); no keyword arguments.
    /// The prompt (any type — converted to `str`) is printed to stdout without
    /// a trailing newline, with stdout flushed before reading.  Raises
    /// `EOFError` when stdin is at EOF.
    fn input(args) -> Result<Value> {
        // Reject keyword arguments with CPython's exact message.
        if args.iter().any(|a| a.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                "input() takes no keyword arguments".to_string(),
            ));
        }
        // Reject more than 1 positional argument.
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("input expected at most 1 argument, got {}", args.len()),
            ));
        }
        // Print the prompt (if any) to stdout without a trailing newline, then flush.
        if let Some(prompt_arg) = args.first() {
            let prompt_str = render_instance_str(_interp, &prompt_arg.value)?;
            print!("{}", prompt_str);
            use std::io::Write as _;
            std::io::stdout().flush().ok();
        }
        // Read one line from stdin.
        // CPython raises OSError for real I/O errors and EOFError only for EOF.
        let mut line = String::new();
        let n = std::io::stdin()
            .read_line(&mut line)
            .map_err(|e| PyError::named("OSError", e.to_string()))?;
        if n == 0 {
            return Err(PyError::named(
                "EOFError",
                "EOF when reading a line".to_string(),
            ));
        }
        // CPython strips only the trailing '\n'; it does NOT strip '\r'.
        // On Linux, a \r\n line from stdin should return "hello\r", not "hello".
        if line.ends_with('\n') {
            line.pop();
        }
        Ok(Value::string(line))
    }

    /// CPython: print(*objects, sep=' ', end='\n', file=sys.stdout, flush=False).
    /// <https://docs.python.org/3/library/functions.html#print>
    fn print(args) -> Result<Value> {
        let print_options = _interp.parse_print_options_expanded(args)?;
        let mut rendered = Vec::with_capacity(print_options.values.len());
        for value in &print_options.values {
            rendered.push(render_instance_str(_interp, value)?);
        }
        // No explicit `file=` → CPython prints to the *current* `sys.stdout`.
        // When that has been redirected (e.g. `contextlib.redirect_stdout`),
        // route through the replacement's `write()`; otherwise fall through to
        // the native console fast path below.
        let file = print_options
            .file
            .or_else(|| _interp.redirected_std_stream("stdout"));
        if let Some(file_val) = file {
            // CPython calls file.write() once per item separated by sep,
            // then calls file.write(end), then file.flush() if flush=True.
            let write_fn = _interp.get_attr(&file_val, "write")?;
            let sep = print_options.sep;
            let end = print_options.end;
            for (i, text) in rendered.into_iter().enumerate() {
                if i > 0 {
                    _interp.call_function_expanded(
                        write_fn.clone(),
                        &[ExpandedCallArg { name: None, value: Value::string(sep.clone()) }],
                    )?;
                }
                _interp.call_function_expanded(
                    write_fn.clone(),
                    &[ExpandedCallArg { name: None, value: Value::string(text) }],
                )?;
            }
            _interp.call_function_expanded(
                write_fn,
                &[ExpandedCallArg { name: None, value: Value::string(end) }],
            )?;
            if print_options.flush {
                let flush_fn = _interp.get_attr(&file_val, "flush")?;
                _interp.call_function_expanded(flush_fn, &[])?;
            }
        } else {
            print!("{}{}", rendered.join(&print_options.sep), print_options.end);
            if print_options.flush {
                use std::io::Write as _;
                std::io::stdout().flush().ok();
            }
        }
        Ok(Value::none())
    }

    /// CPython: range(stop) / range(start, stop[, step]).
    /// <https://docs.python.org/3/library/functions.html#func-range>
    #[pure]
    fn range(args) -> Result<Value> {
        _interp.call_range_expanded(args)
    }

    /// CPython: open(file, mode='r', buffering=-1, encoding=None, errors=None,
    /// newline=None, closefd=True, opener=None).
    /// <https://docs.python.org/3/library/functions.html#open>
    ///
    /// First builtin migrated to the typed-signature dialect (#395) — the
    /// macro-emitted prelude rejects unknown kwargs, validates the positional
    /// count, and binds typed Rust locals.  The `encoding`, `buffering`,
    /// `errors`, `newline`, and `closefd` parameters added here to fix #1360.
    fn open(
        path: PyStr,
        #[default("r".into())]
        mode: PyStr,
        #[default(None)]
        buffering: Option<PyValue>,
        #[default(None)]
        encoding: Option<PyStr>,
        #[default(None)]
        errors: Option<PyStr>,
        #[default(None)]
        newline: Option<PyStr>,
        #[default(None)]
        closefd: Option<PyValue>,
    ) -> Result<Value> {
        let _ = buffering; // accepted, not yet implemented (buffering is complex)
        let _ = errors;    // accepted, not yet implemented
        let _ = newline;   // accepted, not yet implemented
        // `closefd` defaults to True when not supplied (None means absent).
        let closefd_bool = closefd.is_none_or(|v| v.0.truthy_raw());
        pyrust_builtins::file::open(
            &path,
            &mode,
            encoding.as_deref(),
            closefd_bool,
        )
    }

    /// Internal: variadic-call helper used by call sites that unpack
    /// `*args` / `**kwargs`.  Not a Python-level public function, but
    /// shipped under this name so the generated bytecode can reach it.
    fn __vcall__(args) -> Result<Value> {
        if args.len() != 3 {
            return Err(PyError::Runtime(format!("{FN_NAME} requires 3 arguments")));
        }
        let func = args[0].value.clone();
        let pos_items = _interp.collect_iterable(&args[1].value)?;
        let mut expanded: Vec<ExpandedCallArg> = pos_items
            .into_iter()
            .map(|v| ExpandedCallArg { name: None, value: v })
            .collect();
        if let ValueKind::Dict(kw_map) = args[2].value.kind() {
            for (k, v) in kw_map.iter() {
                match k {
                    PyKey::Str(name) => expanded.push(ExpandedCallArg {
                        name: Some(name.as_str().unwrap_or("").to_owned()),
                        value: v.clone(),
                    }),
                    // CPython: a non-string `**` key is a TypeError, not a
                    // silently dropped entry.
                    _ => return Err(pyrust_core::type_err!("keywords must be strings")),
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
        // Delegate to the shared `__format__` dispatcher that f-strings and
        // `str.format` already use (#1370).  It only invokes a *user-defined*
        // `__format__` (skipping the inherited builtin/`object.__format__`),
        // and otherwise extracts the primitive backing and applies the spec.
        // Issue #1935: the previous inline copy here invoked *any* inherited
        // `__format__` — including the builtin one a primitive subclass picks
        // up from its MRO — which rejected a non-empty spec before the
        // backing-extraction branch could run, so `format(MyInt(42), "d")`
        // raised TypeError.  Routing through `dispatch_dunder_format` (which has
        // the `!BuiltinFunction` guard) fixes that and keeps the three format
        // paths in lock-step.
        _interp.dispatch_dunder_format(value, spec)
    }

    /// CPython: classmethod(function) — class-method descriptor.
    /// <https://docs.python.org/3/library/functions.html#classmethod>
    fn classmethod(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 1 argument, got {}", args.len()),
            ));
        }
        // CPython 3.12 accepts any object as a classmethod descriptor.
        // When the argument is a UserFunction we use the existing tagged-kind
        // path; for any other value we wrap it in a BuiltinObject descriptor.
        match args[0].value.kind() {
            ValueKind::UserFunction(f) => Ok(Value::class_method(Rc::clone(f))),
            _ => Ok(pyrust_builtins::classmethod::class_method_any(
                args[0].value.clone(),
            )),
        }
    }

    /// CPython: staticmethod(function) — static-method descriptor.
    /// <https://docs.python.org/3/library/functions.html#staticmethod>
    fn staticmethod(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME} expected 1 argument, got {}", args.len()),
            ));
        }
        // CPython 3.12 accepts any object as a staticmethod descriptor.
        // When the argument is a UserFunction we use the existing tagged-kind
        // path; for any other value we wrap it in a BuiltinObject descriptor.
        match args[0].value.kind() {
            ValueKind::UserFunction(f) => Ok(Value::static_method(Rc::clone(f))),
            _ => Ok(pyrust_builtins::classmethod::static_method_any(
                args[0].value.clone(),
            )),
        }
    }

    /// CPython: property(fget=None, fset=None, fdel=None, doc=None).
    /// <https://docs.python.org/3/library/functions.html#property>
    fn property(args) -> Result<Value> {
        // Accept up to 4 positional args (fget, fset, fdel, doc) or keyword args.
        if args.len() > 4 {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() takes at most 4 arguments ({} given)", args.len()),
            ));
        }
        let mut fget = Value::none();
        let mut fset = Value::none();
        let mut fdel = Value::none();
        let mut doc: Option<Value> = None;
        for (i, arg) in args.iter().enumerate() {
            let name_ref = arg.name.as_deref();
            let idx = match name_ref {
                None => i,
                Some("fget") => 0,
                Some("fset") => 1,
                Some("fdel") => 2,
                Some("doc") => 3,
                Some(k) => return Err(PyError::named(
                    "TypeError",
                    format!("'{k}' is an invalid keyword argument for {FN_NAME}()"),
                )),
            };
            match idx {
                0 => fget = arg.value.clone(),
                1 => fset = arg.value.clone(),
                2 => fdel = arg.value.clone(),
                // doc: store an explicit doc unless it is None (CPython treats
                // `doc=None` as "no explicit doc", falling back to fget's
                // docstring). Issue #1961.
                _ => {
                    if !arg.value.is_none() {
                        doc = Some(arg.value.clone());
                    }
                }
            }
        }
        Ok(pyrust_builtins::property::property_with_doc(fget, fset, fdel, doc))
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
        } else if args.len() == 1 {
            // One-argument super(cls) — an *unbound* super object that acts as a
            // descriptor (#2704).  `__get__(obj, owner)` binds it to a concrete
            // super(cls, obj).
            let cls_val = args[0].value.clone();
            let class = match cls_val.kind() {
                ValueKind::PyClass(c) => Rc::clone(c),
                _ => return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{FN_NAME}() argument 1 must be a type, not {}",
                        value_type_name_str(&cls_val),
                    ),
                )),
            };
            return Ok(Value::super_proxy_unbound(class));
        } else if args.len() == 2 {
            (args[0].value.clone(), args[1].value.clone())
        } else {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expected at most 2 arguments, got {}", args.len()),
            ));
        };
        let class = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => return Err(PyError::named(
                "TypeError",
                format!(
                    "{FN_NAME}() argument 1 must be a type, not {}",
                    value_type_name_str(&cls_val),
                ),
            )),
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
                if class_is_subclass_of(&obj_class, &class) {
                    // Standard case: obj_class is a subclass of class.
                    // e.g. super(Base, Derived) in a classmethod.
                    return Ok(Value::super_proxy_class(class, obj_class));
                }
                // Issue #1385 / #1956: metaclass case — super(Meta, cls) where
                // Meta is a subclass of `type` and `cls` is any class (an
                // "instance" of the metaclass).  In CPython, `type(cls).__mro__`
                // is walked starting after Meta.  We keep `cls` as the proxy's
                // `obj_class` (so e.g. `super().__call__(*a)` binds `cls` as the
                // construction target); env.rs detects the metaclass-method case
                // — Meta is in `type(cls)`'s MRO, not `cls`'s own MRO — and walks
                // the metaclass MRO ([Meta, type, object]) accordingly.
                let type_cls = type_class_singleton();
                if class_is_subclass_of(&class, &type_cls) {
                    return Ok(Value::super_proxy_class(Rc::clone(&class), obj_class));
                }
                Err(PyError::named(
                    "TypeError",
                    "super(type, obj): obj must be an instance or subtype of type".to_string(),
                ))
            }
            _ => Err(PyError::named(
                "TypeError",
                "super(type, obj): obj must be an instance or subtype of type".to_string(),
            )),
        }
    }

    /// CPython: callable(object) — true if the object is callable.
    /// <https://docs.python.org/3/library/functions.html#callable>
    ///
    /// Migrated to the typed-signature dialect (#400).  Mirrors `ascii`
    /// / `id`: a single-body `PyValue` catch-all, since `callable`
    /// accepts every Python object and never raises `TypeError`.
    ///
    /// `#[arity_style(takes_exactly_one)]` (#400/#2331) reproduces the
    /// METH_O wording `callable() takes exactly one argument (N given)`.
    #[pure]
    #[arity_style(takes_exactly_one)]
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
            // Issue #1772: super_bound_builtin (returned by e.g.
            // `object.__subclasshook__` or `object.__init_subclass__`) are
            // also callable — CPython exposes these as
            // `builtin_function_or_method` which is always callable.
            ValueKind::BuiltinObject { .. } => {
                pyrust_builtins::bound_method::is_bound_method(value)
                    || pyrust_builtins::super_bound_builtin::as_super_bound_builtin(value)
                        .is_some()
                    || pyrust_builtins::property::property_partial_slot(value)
                        .is_some_and(|slot| slot.is_some())
                    // Issue #2096: the `type.__call__` method-wrapper surfaced
                    // by `C.__call__` is callable (calling it constructs an
                    // instance), so `callable(C.__call__)` must agree with
                    // CPython's `True`.
                    || pyrust_builtins::type_call_wrapper::as_type_call_wrapper(value).is_some()
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

    /// Issue #988: `list.__init__(self[, iterable])` — resets the backing
    /// store of a list subclass instance.  With no iterable arg the backing
    /// is reset to an empty list (matching CPython where `list.__init__(x)`
    /// clears `x`); with an iterable arg the backing is rebuilt from it.
    ///
    /// CPython signature: `list.__init__(self, iterable=())`
    #[py_name = "list.__init__"]
    fn list_init(args) -> Result<Value> {
        let Some(first) = args.first() else {
            return Ok(Value::none());
        };
        let inst_rc = match first.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => return Ok(Value::none()),
        };
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("list expected at most 1 argument, got {}", args.len() - 1),
            ));
        }
        if instance_builtin_data(&inst_rc).is_some() {
            let list_dispatch = crate::builtin_registry::lookup("list").ok_or_else(|| {
                PyError::Runtime("internal: list constructor not in registry".to_string())
            })?;
            // Pass the iterable arg if present, or empty args to get an empty list.
            let new_backing = list_dispatch(_interp, args.get(1).map_or(&[], std::slice::from_ref))?;
            inst_rc.borrow_mut().attrs.insert("__builtin_data__", new_backing);
        }
        Ok(Value::none())
    }

    /// Issue #988: `dict.__init__(self[, mapping_or_iterable][, **kwargs])` —
    /// resets the backing store of a dict subclass instance.  With no args
    /// beyond self the backing is reset to an empty dict; with args it is
    /// rebuilt (matching CPython's `dict.__init__` clearing and re-populating
    /// behaviour).
    ///
    /// CPython signature: `dict.__init__(self, *args, **kwargs)`
    #[py_name = "dict.__init__"]
    fn dict_init(args) -> Result<Value> {
        let Some(first) = args.first() else {
            return Ok(Value::none());
        };
        let inst_rc = match first.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => return Ok(Value::none()),
        };
        let pos_count = args[1..].iter().filter(|a| a.name.is_none()).count();
        if pos_count > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("dict expected at most 1 argument, got {}", pos_count),
            ));
        }
        if instance_builtin_data(&inst_rc).is_some() {
            let dict_dispatch = crate::builtin_registry::lookup("dict").ok_or_else(|| {
                PyError::Runtime("internal: dict constructor not in registry".to_string())
            })?;
            // Pass remaining args (positional + kwargs) or nothing for the empty case.
            let new_backing = dict_dispatch(_interp, &args[1..])?;
            inst_rc.borrow_mut().attrs.insert("__builtin_data__", new_backing);
        }
        Ok(Value::none())
    }

    /// Issue #1134: `dict.__getitem__(self, key)` — native dict subscript for
    /// dict subclasses.  Called via `super().__getitem__(key)` when the
    /// subclass routes through the SuperProxy mechanism.  Performs the raw
    /// backing-dict lookup and honours `__missing__` when the key is absent.
    ///
    /// CPython signature: `dict.__getitem__(self, key)`
    #[py_name = "dict.__getitem__"]
    fn dict_getitem(args) -> Result<Value> {
        // CPython exposes dict.__getitem__ as a *method_descriptor* (#2266):
        // missing receiver -> "unbound method ...", wrong receiver type ->
        // "descriptor ... doesn't apply to a '<X>' object".  The receiver check
        // happens before the arity check, matching CPython's slot ordering.
        let self_arg = args.first().ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getitem__", "dict", method)
        })?;
        let inst_rc = match self_arg.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => {
                let actual = pyrust_core::builtin_type_name(&self_arg.value);
                return Err(pyrust_core::descriptor_requires!(
                    "__getitem__", "dict", actual, method
                ));
            }
        };
        let key = match args.get(1) {
            Some(key_arg) => key_arg.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "dict.__getitem__ expected 2 arguments".to_string(),
                ));
            }
        };
        let backing = instance_builtin_data(&inst_rc).ok_or_else(|| {
            pyrust_core::descriptor_requires!("__getitem__", "dict")
        })?;
        // #2657: a PyInstance receiver whose base is not `dict` (e.g. a list
        // subclass) must be rejected with CPython's method_descriptor wording
        // instead of reaching the dict-lookup helper and tripping its
        // "internal: expected dict" assertion.
        if !matches!(backing.kind(), ValueKind::Dict(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "dict", actual, method
            ));
        }
        let lookup = if let Some(s) = key.as_str() {
            _interp.dict_str_lookup(&backing, s)?
        } else {
            let py_key = _interp.value_to_pykey(&key)?;
            _interp.dict_lookup(&backing, &py_key)?
        };
        match lookup {
            Some((_, v)) => Ok(v),
            None => {
                let class = Rc::clone(&inst_rc.borrow().class);
                if let Some(missing_fn) = lookup_class_attr(&class, "__missing__") {
                    invoke_class_method(
                        _interp,
                        missing_fn,
                        Value::py_instance(inst_rc),
                        &[ExpandedCallArg { name: None, value: key }],
                    )
                } else {
                    Err(PyError::key_error(key))
                }
            }
        }
    }

    /// Issue #1134 (review): `list.__getitem__(self, key)` — native list subscript
    /// for list subclasses.  Called via `super().__getitem__(key)` from a list
    /// subclass override.  Delegates to `eval_index` on the backing primitive.
    ///
    /// CPython signature: `list.__getitem__(self, key)`
    #[py_name = "list.__getitem__"]
    fn list_getitem(args) -> Result<Value> {
        // CPython exposes list.__getitem__ as a *method_descriptor* (#2266):
        // missing receiver -> "unbound method ...", wrong receiver type ->
        // "descriptor ... doesn't apply to a '<X>' object".  The receiver check
        // happens before the arity check, matching CPython's slot ordering.
        let self_arg = args.first().ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getitem__", "list", method)
        })?;
        if !matches!(self_arg.value.kind(), ValueKind::PyInstance(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "list", actual, method
            ));
        }
        let key = match args.get(1) {
            Some(key_arg) => key_arg.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "list.__getitem__ expected 2 arguments".to_string(),
                ));
            }
        };
        let backing = builtin_data_backing(&self_arg.value).ok_or_else(|| {
            pyrust_core::descriptor_requires!("__getitem__", "list")
        })?;
        // #2657: a PyInstance receiver whose base is not `list` (e.g. a tuple
        // subclass) must be rejected with CPython's method_descriptor wording.
        if !matches!(backing.kind(), ValueKind::List(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "list", actual, method
            ));
        }
        _interp.eval_index(&backing, key)
    }

    /// Issue #1134 (review): `tuple.__getitem__(self, key)` — native tuple subscript
    /// for tuple subclasses.  Called via `super().__getitem__(key)` from a tuple
    /// subclass override.  Delegates to `eval_index` on the backing primitive.
    ///
    /// CPython signature: `tuple.__getitem__(self, key)`
    #[py_name = "tuple.__getitem__"]
    fn tuple_getitem(args) -> Result<Value> {
        // CPython exposes tuple.__getitem__ as a *slot wrapper* (#2266/#2276):
        // missing receiver -> "descriptor '__getitem__' of 'tuple' object needs
        // an argument", wrong receiver type -> "descriptor '__getitem__'
        // requires a 'tuple' object but received a '<X>'".  The receiver check
        // precedes the arity check, matching CPython's slot ordering.
        let self_arg = args.first().ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getitem__", "tuple")
        })?;
        if !matches!(self_arg.value.kind(), ValueKind::PyInstance(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "tuple", actual
            ));
        }
        let key = match args.get(1) {
            Some(key_arg) => key_arg.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "tuple.__getitem__ expected 2 arguments".to_string(),
                ));
            }
        };
        let backing = builtin_data_backing(&self_arg.value).ok_or_else(|| {
            pyrust_core::descriptor_requires!("__getitem__", "tuple")
        })?;
        // #2657: a PyInstance receiver whose base is not `tuple` (e.g. a list
        // subclass) must be rejected with CPython's slot-wrapper wording.
        if !matches!(backing.kind(), ValueKind::Tuple(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "tuple", actual
            ));
        }
        _interp.eval_index(&backing, key)
    }

    /// Issue #1134 (review): `bytes.__getitem__(self, key)` — native bytes subscript
    /// for bytes subclasses.  Called via `super().__getitem__(key)` from a bytes
    /// subclass override.  Delegates to `eval_index` on the backing primitive.
    ///
    /// CPython signature: `bytes.__getitem__(self, key)`
    #[py_name = "bytes.__getitem__"]
    fn bytes_getitem(args) -> Result<Value> {
        // CPython exposes bytes.__getitem__ as a *slot wrapper* (#2266/#2276):
        // missing receiver -> "descriptor '__getitem__' of 'bytes' object needs
        // an argument", wrong receiver type -> "descriptor '__getitem__'
        // requires a 'bytes' object but received a '<X>'".  The receiver check
        // precedes the arity check, matching CPython's slot ordering.
        let self_arg = args.first().ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__getitem__", "bytes")
        })?;
        if !matches!(self_arg.value.kind(), ValueKind::PyInstance(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "bytes", actual
            ));
        }
        let key = match args.get(1) {
            Some(key_arg) => key_arg.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "bytes.__getitem__ expected 2 arguments".to_string(),
                ));
            }
        };
        let backing = builtin_data_backing(&self_arg.value).ok_or_else(|| {
            pyrust_core::descriptor_requires!("__getitem__", "bytes")
        })?;
        // #2657: a PyInstance receiver whose base is not `bytes` (e.g. a list
        // subclass) must be rejected with CPython's slot-wrapper wording.
        if !matches!(backing.kind(), ValueKind::Bytes(_)) {
            let actual = pyrust_core::builtin_type_name(&self_arg.value);
            return Err(pyrust_core::descriptor_requires!(
                "__getitem__", "bytes", actual
            ));
        }
        _interp.eval_index(&backing, key)
    }

    /// Issue #988: `set.__init__(self[, iterable])` — resets the backing
    /// store of a set subclass instance.  With no iterable arg the backing
    /// is reset to an empty set; with an iterable arg the backing is rebuilt
    /// from it (matching CPython's clearing + re-populating behaviour).
    ///
    /// CPython signature: `set.__init__(self, iterable=())`
    #[py_name = "set.__init__"]
    fn set_init(args) -> Result<Value> {
        let Some(first) = args.first() else {
            return Ok(Value::none());
        };
        let inst_rc = match first.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => return Ok(Value::none()),
        };
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!("set expected at most 1 argument, got {}", args.len() - 1),
            ));
        }
        if instance_builtin_data(&inst_rc).is_some() {
            let set_dispatch = crate::builtin_registry::lookup("set").ok_or_else(|| {
                PyError::Runtime("internal: set constructor not in registry".to_string())
            })?;
            // Pass the iterable arg if present, or empty args to get an empty set.
            let new_backing = set_dispatch(_interp, args.get(1).map_or(&[], std::slice::from_ref))?;
            inst_rc.borrow_mut().attrs.insert("__builtin_data__", new_backing);
        }
        Ok(Value::none())
    }

    /// Issue #1004: `frozenset.__init__` — no-op.  frozenset is immutable; the
    /// backing data is fixed at `__new__` time.  Registering this sentinel
    /// allows `super().__init__()` in a frozenset subclass to resolve without
    /// AttributeError (matching CPython 3.12 where frozenset inherits
    /// object.__init__ which ignores all args when __new__ is overridden).
    ///
    /// CPython signature: `frozenset.__init__(self, *args, **kwargs)`
    #[py_name = "frozenset.__init__"]
    fn frozenset_init(_args) -> Result<Value> {
        Ok(Value::none())
    }

    /// Issue #1004: `tuple.__init__` — no-op.  tuple is immutable; the
    /// backing data is fixed at `__new__` time.  Registering this sentinel
    /// allows `super().__init__()` in a tuple subclass to resolve without
    /// AttributeError.
    ///
    /// CPython signature: `tuple.__init__(self, *args, **kwargs)`
    #[py_name = "tuple.__init__"]
    fn tuple_init(_args) -> Result<Value> {
        Ok(Value::none())
    }

    /// Issue #1047: `object.__init_subclass__` — the default no-op hook.
    /// CPython (Objects/typeobject.c) registers this on `object` so that
    /// `super().__init_subclass__(**kwargs)` inside a user-defined
    /// `__init_subclass__` terminates the MRO walk without error.
    ///
    /// CPython raises TypeError if any keyword arguments reach this point:
    /// the expectation is that each level of the MRO consumed its own kwargs
    /// before forwarding the rest upward with `super().__init_subclass__(**kwargs)`.
    ///
    /// CPython signature: `object.__init_subclass__(cls, /)`
    #[py_name = "object.__init_subclass__"]
    fn object_init_subclass(args) -> Result<Value> {
        // Raise TypeError if any keyword arguments reach this point.  Each
        // level of the MRO should have consumed its own kwargs before calling
        // super().__init_subclass__(**remaining_kwargs).
        //
        // CPython's error message uses the new class's name (the `cls` arg),
        // not the literal string "object". E.g. for `class B(A, foo=1)` the
        // message is "B.__init_subclass__() takes no keyword arguments".
        //
        // Check keyword args first (CPython raises keyword error even when
        // positional excess is also present).
        if args.iter().any(|a| a.name.is_some()) {
            let cls_name = args
                .first()
                .and_then(|a| match a.value.kind() {
                    ValueKind::PyClass(c) => Some(c.borrow().name.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "object".to_string());
            return Err(PyError::named(
                "TypeError",
                format!("{cls_name}.__init_subclass__() takes no keyword arguments"),
            ));
        }
        // args[0] is the implicit `cls` prepended by the classmethod dispatch.
        // Any additional positional arguments are excess.  Use the same cls_name
        // lookup as the keyword-error path: CPython uses the subclass name in
        // the positional error too (e.g. "B.__init_subclass__() takes no
        // arguments (1 given)" when called as `B.__init_subclass__(42)`).
        let n_positional = args.iter().filter(|a| a.name.is_none()).count();
        if n_positional > 1 {
            let excess = n_positional - 1;
            let cls_name = args
                .first()
                .and_then(|a| match a.value.kind() {
                    ValueKind::PyClass(c) => Some(c.borrow().name.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "object".to_string());
            return Err(PyError::named(
                "TypeError",
                format!("{cls_name}.__init_subclass__() takes no arguments ({excess} given)"),
            ));
        }
        Ok(Value::none())
    }

    /// Issue #1738: `object.__subclasshook__` — the default classmethod hook
    /// used by `ABCMeta.__subclasscheck__` to allow custom `issubclass()`
    /// behaviour.  The default implementation on `object` always returns
    /// `NotImplemented`, signalling that the normal MRO-based subclass check
    /// should proceed.
    ///
    /// CPython signature: `object.__subclasshook__(cls, subclass, /)`
    ///
    /// CPython rejects keyword arguments with the message
    /// `__subclasshook__() takes no keyword arguments` (note: no `object.`
    /// prefix, unlike `__init_subclass__`).  Any number of positional args
    /// is accepted — the implementation ignores them all.
    #[py_name = "object.__subclasshook__"]
    fn object_subclasshook(args) -> Result<Value> {
        reject_keyword_args_expanded("__subclasshook__", args)?;
        Ok(Value::not_implemented())
    }

    /// Issue #1256: `object.__str__(self)` — the default __str__ exposed on
    /// the `object` class so that `super().__str__()` and `hasattr(object,
    /// '__str__')` work correctly.
    ///
    /// CPython's `object.__str__` is implemented by calling `tp_repr` on the
    /// object's type (typeobject.c:object_str).  We route through
    /// `render_value_repr` which dispatches `type(self).__repr__(self)` for
    /// user instances, and falls back to `value.repr_raw()` for primitives.
    ///
    /// CPython signature: `object.__str__(self, /)`
    #[py_name = "object.__str__"]
    fn object_str_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__str__", "object")
        })?;
        let s = render_value_repr(_interp, &self_val)?;
        Ok(Value::string(s))
    }

    /// Issue #1256: `object.__repr__(self)` — the default __repr__ on `object`.
    ///
    /// Returns the canonical `<ClassName object at 0xADDR>` format for plain
    /// instances.  For `PyInstance` values that carry a primitive backing store
    /// (e.g. a `MyList(list)` subclass instance), delegates to the backing data
    /// so that `list.__repr__(MyList([1,2,3]))` returns `[1, 2, 3]` rather than
    /// the generic `<__main__.MyList object at 0x...>` form.
    ///
    /// Issue #1600: regression from PR #1595 (primitive types got
    /// `base: Some(OBJECT_CLASS)`), which caused `list.__repr__` to resolve via
    /// MRO to this sentinel and fall through to `self_val.repr_raw()`.
    ///
    /// Note: we call `render_value_repr(interp, &backing)` (on the raw backing
    /// value, NOT on the instance) so that nested `PyInstance` elements inside a
    /// list/dict/tuple get their own `__repr__` dispatched correctly, but we do
    /// NOT re-run the MRO lookup on the outer instance — this matches CPython's
    /// behaviour where `list.__repr__(MyList_with_custom_repr)` still renders the
    /// list contents, not the custom `__repr__`.
    ///
    /// CPython signature: `object.__repr__(self, /)`
    #[py_name = "object.__repr__"]
    fn object_repr_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "object")
        })?;
        // Issue #1600: for a PyInstance with backing primitive data, render the
        // backing value directly (bypassing MRO lookup) so that
        // `list.__repr__(MyList([1,2,3]))` returns `[1, 2, 3]`.
        if let ValueKind::PyInstance(inst_rc) = self_val.kind() {
            let inst_rc = Rc::clone(inst_rc);
            if let Some(backing) = instance_builtin_data(&inst_rc) {
                let class = Rc::clone(&inst_rc.borrow().class);
                let s = match backing.kind() {
                    ValueKind::Str(_)
                    | ValueKind::Int(_)
                    | ValueKind::BigInt(_)
                    | ValueKind::Bool(_)
                    | ValueKind::Float(_)
                    | ValueKind::Complex(_, _)
                    | ValueKind::Bytes(_) => backing.repr_raw(),
                    ValueKind::List(_) | ValueKind::Dict(_) | ValueKind::Tuple(_) => {
                        render_value_repr(_interp, &backing)?
                    }
                    ValueKind::Set(items) => {
                        let class_name = class.borrow().name.clone();
                        if items.is_empty() {
                            format!("{class_name}()")
                        } else {
                            let inner = render_value_repr(_interp, &backing)?;
                            format!("{class_name}({inner})")
                        }
                    }
                    ValueKind::BuiltinObject { ops, .. }
                        if ops.type_name() == pyrust_builtins::frozenset::TYPE_NAME =>
                    {
                        let class_name = class.borrow().name.clone();
                        let items = pyrust_builtins::frozenset::as_items(&backing);
                        let is_empty = items.as_ref().is_none_or(|rc| rc.is_empty());
                        if is_empty {
                            format!("{class_name}()")
                        } else {
                            let snapshot: Vec<_> =
                                items.unwrap().iter().cloned().collect();
                            let mut parts = Vec::with_capacity(snapshot.len());
                            for k in &snapshot {
                                parts.push(render_key_repr(_interp, k)?);
                            }
                            format!("{class_name}({{{}}})", parts.join(", "))
                        }
                    }
                    _ => self_val.repr_raw(),
                };
                return Ok(Value::string(s));
            }
        }
        Ok(Value::string(self_val.repr_raw()))
    }

    /// Issue #1256: `object.__eq__(self, other)` — default identity equality.
    ///
    /// Returns `True` if `self is other`, `NotImplemented` otherwise (so the
    /// reflected `other.__eq__(self)` gets a chance).  This matches CPython's
    /// `object_richcompare` for `Py_EQ`.
    ///
    /// CPython signature: `object.__eq__(self, value, /)`
    #[py_name = "object.__eq__"]
    fn object_eq_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__eq__", "object")),
        };
        // Identity comparison: two PyInstance values are equal iff they are
        // the same object (Rc::ptr_eq), matching CPython's default __eq__.
        // For non-instance primitives we fall back to structural equality so
        // that `int.__eq__(1, 1)` still returns True.
        let same = match (a.kind(), b.kind()) {
            (ValueKind::PyInstance(ra), ValueKind::PyInstance(rb)) => {
                Rc::ptr_eq(ra, rb)
            }
            _ => a == b,
        };
        Ok(if same { Value::bool_(true) } else { Value::not_implemented() })
    }

    /// Issue #1256: `object.__ne__(self, other)` — default identity inequality.
    ///
    /// Returns `False` if `self is other`, `NotImplemented` otherwise.
    ///
    /// CPython signature: `object.__ne__(self, value, /)`
    #[py_name = "object.__ne__"]
    fn object_ne_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ne__", "object")),
        };
        let same = match (a.kind(), b.kind()) {
            (ValueKind::PyInstance(ra), ValueKind::PyInstance(rb)) => {
                Rc::ptr_eq(ra, rb)
            }
            _ => a == b,
        };
        Ok(if same { Value::bool_(false) } else { Value::not_implemented() })
    }

    /// Issue #1256: `object.__hash__(self)` — default identity-based hash.
    ///
    /// For user instances, CPython hashes by `id(self) // 16`.  For primitives
    /// routed here via an explicit `object.__hash__(x)` call, delegate to the
    /// standard `hash_value_with_interp` helper which already contains the
    /// correct Mersenne-prime hash for each primitive type.
    ///
    /// CPython signature: `object.__hash__(self, /)`
    #[py_name = "object.__hash__"]
    fn object_hash_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__hash__", "object")
        })?;
        // For user instances use the Rc pointer as the identity hash, matching
        // CPython's default `id(x) >> 4`.  Map -1 → -2 as CPython requires.
        if let ValueKind::PyInstance(inst) = self_val.kind() {
            let ptr = Rc::as_ptr(inst) as i64;
            let h = if ptr == -1 { -2 } else { ptr };
            return Ok(Value::int(h));
        }
        // For primitives, use the shared hash helper.
        let h = hash_value_with_interp(_interp, &self_val)?;
        Ok(Value::int(h))
    }

    /// Issue #2151: `object.__sizeof__(self)` — the size of the object in
    /// bytes.  CPython returns an implementation-specific value; pyrust's
    /// NaN-boxed representation has no comparable layout, so we report the
    /// in-memory `Value` size as a plausible, deterministic-per-build int.
    /// Tests assert only the return type (int), not the exact value.
    #[py_name = "object.__sizeof__"]
    fn object_sizeof_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__sizeof__", "object", method));
        }
        Ok(Value::int(std::mem::size_of::<Value>() as i64))
    }

    /// Issue #2151: `object.__dir__(self)` — the default attribute listing,
    /// equivalent to `dir(self)` before `dir()` sorts.  Returns a `list`.
    #[py_name = "object.__dir__"]
    fn object_dir_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__dir__", "object", method)
        })?;
        let mut names = dir_names(&self_val);
        names.sort();
        names.dedup();
        Ok(Value::list(names.into_iter().map(Value::string).collect()))
    }

    /// Issue #2151: `object.__reduce_ex__(self, protocol)` — the pickle-protocol
    /// reduction.  CPython returns the `copyreg.__newobj__` tuple; pyrust does
    /// not model copyreg, so we return a tuple of the correct *shape*
    /// (`(class, ())`).  Tests assert only the return type (tuple).
    #[py_name = "object.__reduce_ex__"]
    fn object_reduce_ex_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__reduce_ex__", "object", method)
        })?;
        Ok(Value::tuple(vec![
            value_class(&self_val),
            Value::tuple(Vec::new()),
        ]))
    }

    /// Issue #2151: `object.__reduce__(self)` — the default reduction, which
    /// CPython implements as `self.__reduce_ex__(2)`.  Returns a tuple.
    #[py_name = "object.__reduce__"]
    fn object_reduce_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__reduce__", "object", method)
        })?;
        Ok(Value::tuple(vec![
            value_class(&self_val),
            Value::tuple(Vec::new()),
        ]))
    }

    /// Issue #2151: `None.__bool__()` returns `False`.  `__bool__` is
    /// NoneType-specific (not inherited from `object`).
    #[py_name = "NoneType.__bool__"]
    fn none_bool_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__bool__", "NoneType"));
        }
        Ok(Value::bool_(false))
    }

    /// Issue #1256: `object.__init__(self, *args, **kwargs)` — the default no-op
    /// `__init__` exposed on `object` so that `super().__init__()` in user
    /// classes terminates the MRO walk without error.
    ///
    /// Issue #1016: CPython 3.12 arg-leniency rule — extra positional/keyword
    /// arguments are accepted (and ignored) only when BOTH:
    ///   (a) `type(self)` defines a custom `__new__` (not `object.__new__`), AND
    ///   (b) `type(self)` does NOT define a custom `__init__` (i.e. inherits
    ///       `object.__init__`).
    /// This is the symmetric counterpart of the rule in `object_new_dunder`.
    /// In all other cases extra args raise `TypeError: object.__init__() takes
    /// exactly one argument (the instance to initialize)`.
    ///
    /// CPython signature: `object.__init__(self, /)`
    #[py_name = "object.__init__"]
    fn object_init_dunder(args) -> Result<Value> {
        // CPython 3.12 descriptor protocol: object.__init__() called with no
        // arguments (no self) raises TypeError.
        // Reproduced from CPython's slot_tp_init / descriptor wrappers:
        //   TypeError: descriptor '__init__' of 'object' object needs an argument
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__init__", "object"));
        }
        // Only args beyond the mandatory first (self) are "extra".
        let has_extra_args = args.len() > 1 || args.iter().skip(1).any(|a| a.name.is_some());
        if has_extra_args {
            // Determine whether the leniency rule applies.  From CPython's
            // object_init in Objects/typeobject.c:
            //
            //   if (excess_args(args, kwds)) {
            //       if (type->tp_new == object_new) {
            //           /* no custom __new__ → error */
            //       } else if (type->tp_init != object_init) {
            //           /* custom __new__ AND custom __init__ → error */
            //       }
            //       /* else: custom __new__, no custom __init__ → lenient */
            //   }
            //
            // Lenient iff: has_custom_new AND NOT has_custom_init.
            let self_val = args.first().map(|a| a.value.clone());
            let class_rc_opt = self_val.as_ref().and_then(|v| match v.kind() {
                ValueKind::PyInstance(inst) => Some(Rc::clone(&inst.borrow().class)),
                _ => None,
            });
            // CPython error message prefix:
            //   - when type(self) has a custom __init__: "object.__init__()"
            //   - when type(self) has no custom __init__: "<typename>.__init__()"
            // (Objects/typeobject.c, object_init:
            //    PyErr_Format(..., "%.100s.__init__()", Py_TYPE(self)->tp_name)
            //    when tp_init == object_init; "object.__init__()" otherwise)
            let (is_lenient, err_prefix) = if let Some(ref class_rc) = class_rc_opt {
                // "Custom __new__" = any __new__ that is not object.__new__.
                // This includes:
                //   (a) user-defined __new__ (UserFunction), or
                //   (b) a registered builtin __new__ (str.__new__, int.__new__,
                //       etc.) — BuiltinFunction with name != "object.__new__",
                //   (c) a primitive subclass that uses the type's builtin
                //       constructor as its allocator (e.g. complex/list/dict/set
                //       which lack an explicit __new__ registration but are
                //       handled by find_scalar/mutable/immutable_primitive_base
                //       in call_class_expanded).  In CPython these types have
                //       tp_new != object_new at the C level.
                let new_val = lookup_class_attr(class_rc, "__new__");
                let has_custom_new = match new_val.as_ref().map(|v| v.kind()) {
                    Some(ValueKind::UserFunction(_)) => true,
                    Some(ValueKind::BuiltinFunction("object.__new__")) => {
                        // __new__ resolved to object.__new__ via MRO.  This can
                        // happen for primitive types like complex/list/dict/set
                        // that have no explicit __new__ registration in pyrust.
                        // In CPython these types have tp_new != object_new at the
                        // C level, so treat their subclasses as having a custom
                        // __new__ by checking for primitive ancestry.
                        find_scalar_primitive_base(class_rc).is_some()
                            || find_mutable_primitive_base(class_rc).is_some()
                            || find_immutable_primitive_base(class_rc).is_some()
                    }
                    Some(ValueKind::BuiltinFunction(_)) => true,
                    _ => false,
                };
                // "Custom __init__" = any __init__ other than object.__init__:
                // both user-defined (UserFunction) and builtin subtype inits
                // (list.__init__, dict.__init__, etc.) count.  object.__init__
                // is the sentinel; None means the MRO has no __init__ at all
                // (only possible for pathological class structures).
                let init_val = lookup_class_attr(class_rc, "__init__");
                let has_custom_init = !matches!(
                    init_val.as_ref().map(|v| v.kind()),
                    None | Some(ValueKind::BuiltinFunction("object.__init__"))
                );
                // CPython error prefix: type name when no custom __init__,
                // "object" when a custom __init__ is present.
                let prefix = if has_custom_init {
                    "object".to_string()
                } else {
                    class_rc.borrow().name.clone()
                };
                (has_custom_new && !has_custom_init, prefix)
            } else {
                (false, "object".to_string())
            };
            if !is_lenient {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{err_prefix}.__init__() takes exactly one argument (the instance to initialize)"
                    ),
                ));
            }
        }
        Ok(Value::none())
    }

    /// Issue #1143: `object.__new__(cls)` — the default allocator that creates
    /// a bare `PyInstance` of `cls`.  Registered so that `super().__new__(cls)`
    /// in user-defined `__new__` methods can resolve it via the MRO walk and
    /// `call_class_expanded` can distinguish it from user-defined `__new__`.
    ///
    /// Issue #1421: CPython 3.12 arg-leniency rule — extra positional/keyword
    /// arguments are accepted (and ignored) only when BOTH:
    ///   (a) `cls` does NOT define a custom `__new__` (i.e. cls.__new__ IS
    ///       object.__new__), AND
    ///   (b) `cls` defines a custom `__init__` (something other than
    ///       object.__init__).
    /// When `cls` defines a custom `__new__`, extra args raise
    /// `TypeError: object.__new__() takes exactly one argument (the type to
    /// instantiate)`.  When neither custom override is present, extra args
    /// raise `TypeError: <cls>() takes no arguments`.
    ///
    /// CPython signature: `object.__new__(cls, /)`
    #[py_name = "object.__new__"]
    fn object_new_dunder(args) -> Result<Value> {
        let cls_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            PyError::named(
                "TypeError",
                "object.__new__(): not enough arguments".to_string(),
            )
        })?;
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "object.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        // Issue #1421: reject extra args unless the full CPython 3.12 leniency
        // rule is satisfied.  From Objects/typeobject.c (object_new):
        //
        //   lenient iff:
        //     (a) cls.__new__  IS  object.__new__  (no custom __new__), AND
        //     (b) cls.__init__ is NOT object.__init__ (has a custom __init__)
        //
        // When both (a) and (b) hold, the extra args are intended for the
        // custom __init__ and object.__new__ should silently ignore them.
        // In all other cases extra args are a programmer error:
        //
        //   - cls has a custom __new__ → the custom __new__ is responsible for
        //     any extra args; object.__new__ rejects them with the "takes
        //     exactly one argument" wording.
        //   - cls has no custom __init__ → no-one will consume the args →
        //     "<cls>() takes no arguments".
        // Only args beyond the mandatory first (cls) are "extra".  Do not
        // include the cls arg itself — it may arrive as a keyword arg via the
        // raw expanded-arg slice, and that must not trigger the leniency check.
        let has_extra_args = args.len() > 1 || args.iter().skip(1).any(|a| a.name.is_some());
        if has_extra_args {
            let new_val = lookup_class_attr(&class_rc, "__new__");
            let has_custom_new = matches!(
                new_val.as_ref().map(|v| v.kind()),
                Some(ValueKind::UserFunction(_))
            );
            // Exception subclasses are special: CPython's BaseException.__new__
            // (BaseException_new in Objects/exceptions.c) accepts extra args and
            // stores them as .args.  In pyrust there is no separate
            // BaseException.__new__ registration — the MRO walk falls through to
            // object_new_dunder.  When cls is a BaseException subclass, mirror
            // CPython by accepting the extra args silently (they will be processed
            // by BaseException.__init__).
            let is_exception_subclass =
                class_chain_contains_name(&class_rc, "BaseException");
            if is_exception_subclass {
                // Accept extra args for exception subclasses unconditionally.
                // BaseException.__new__ is responsible for them in CPython.
            } else {
                if has_custom_new {
                    return Err(PyError::named(
                        "TypeError",
                        "object.__new__() takes exactly one argument (the type to instantiate)"
                            .to_string(),
                    ));
                }
                let init_val = lookup_class_attr(&class_rc, "__init__");
                let has_custom_init = matches!(
                    init_val.as_ref().map(|v| v.kind()),
                    Some(ValueKind::UserFunction(_))
                );
                if !has_custom_init {
                    let cls_name = class_rc.borrow().name.clone();
                    return Err(PyError::named(
                        "TypeError",
                        format!("{cls_name}() takes no arguments"),
                    ));
                }
            }
        }
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs: InstanceAttrs::new(),
            },
        ))))
    }

    /// Issue #1385: `type.__new__(mcs, name, bases, namespace)` — the metaclass
    /// allocator.  Creates a new `PyClass` from the given arguments.  Called when
    /// `super().__new__(mcs, name, bases, namespace)` is used inside a custom
    /// metaclass `__new__` method.
    ///
    /// CPython signature: `type.__new__(cls, name, bases, dict, /)`
    #[py_name = "type.__new__"]
    fn type_new_dunder(args) -> Result<Value> {
        // type.__new__ has two call signatures:
        //   type.__new__(mcs, name, bases, namespace, **kwds)  — metaclass alloc
        //   type(obj)                                          — returns type(obj)
        // The one-arg form is handled by the "type" registry entry, not here.
        // The 4 positional args are mcs + name + bases + namespace; any extra
        // keyword args are the PEP 487 class kwargs that get forwarded to
        // `__init_subclass__` (CPython's type.__new__ accepts **kwds).
        let positional: Vec<&ExpandedCallArg> =
            args.iter().filter(|a| a.name.is_none()).collect();
        let init_subclass_kwargs: Vec<ExpandedCallArg> = args
            .iter()
            .filter(|a| a.name.is_some())
            .cloned()
            .collect();
        if positional.len() != 4 {
            // CPython counts positional args excluding the implicit cls arg, so
            // the error says "3 arguments" (name, bases, dict) not "4".
            // With 0 args (no cls at all), CPython uses a different message.
            if positional.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "type.__new__(): not enough arguments".to_string(),
                ));
            }
            return Err(PyError::named(
                "TypeError",
                format!(
                    "type.__new__() takes exactly 3 arguments ({} given)",
                    positional.len() - 1,
                ),
            ));
        }
        let mcs_val = positional[0].value.clone();
        let name_val = positional[1].value.clone();
        let bases_val = positional[2].value.clone();
        let namespace_val = positional[3].value.clone();

        let mcs_rc = match mcs_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "type.__new__(): first argument must be a type".to_string(),
                ));
            }
        };
        let name = match name_val.as_str() {
            Some(s) => s.to_string(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "type.__new__(): second argument must be a str".to_string(),
                ));
            }
        };
        // Parse bases tuple/list into Vec<Rc<RefCell<PyClass>>>.
        let bases_slice: &[Value] = bases_val
            .as_tuple()
            .or_else(|| bases_val.as_list())
            .unwrap_or(&[]);
        let mut base: Option<Rc<RefCell<PyClass>>> = None;
        let mut extra_bases: Vec<Rc<RefCell<PyClass>>> = Vec::new();
        for (i, b) in bases_slice.iter().enumerate() {
            match b.kind() {
                ValueKind::PyClass(c) => {
                    if i == 0 {
                        base = Some(Rc::clone(c));
                    } else {
                        extra_bases.push(Rc::clone(c));
                    }
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "type.__new__(): bases must be types".to_string(),
                    ));
                }
            }
        }
        // Issue #1677 / #2109: reject bases with incompatible instance layouts
        // (two C-level primitive bases, or two non-empty `__slots__` bases).
        {
            let all_bases: Vec<_> = base.iter().chain(extra_bases.iter()).cloned().collect();
            if crate::interpreter::bases_have_layout_conflict(&all_bases) {
                return Err(PyError::named(
                    "TypeError",
                    "multiple bases have instance lay-out conflict".to_string(),
                ));
            }
        }
        // Build attrs from the namespace dict.
        let mut attrs: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
        if let Some(map) = namespace_val.as_dict() {
            for (k, v) in map.iter() {
                if let PyKey::Str(s) = k
                    && let Some(key_str) = s.as_str() {
                        attrs.insert(key_str.to_string(), v.clone());
                    }
            }
        }
        // Issue #1626: record the actual metatype on the class so that
        // `type(Foo)` returns the metaclass and `isinstance(Foo, Meta)` works.
        // `type` itself is the default metatype and is represented as `None`
        // to avoid a circular Rc; only a custom metaclass is stored explicitly.
        let metatype = {
            let type_class = type_class_singleton();
            if Rc::ptr_eq(&mcs_rc, &type_class) {
                None
            } else {
                Some(mcs_rc)
            }
        };
        // Issues #2129 / #2130: run the full class-creation finalization
        // (__module__, __slots__, __set_name__, __init_subclass__) so a class
        // built via a metaclass / `type.__new__` matches the `class` statement.
        // The `class`-statement metaclass path now also routes here exactly
        // once (via `exec_make_class_meta`), so hooks fire once, not twice.
        _interp.build_class_via_type(name, base, extra_bases, attrs, metatype, &init_subclass_kwargs)
    }

    /// Issue #1385: `type.__init__(cls, name, bases, namespace)` — the
    /// metaclass initialiser.  In CPython `type.__init__` is effectively a
    /// no-op (the real work happens in `type.__new__`).  Registering it here
    /// lets `super().__init__(name, bases, namespace)` in a custom metaclass
    /// `__init__` resolve and terminate cleanly instead of raising
    /// `AttributeError: super(): parent class has no attribute '__init__'`.
    ///
    /// CPython signature: `type.__init__(cls, name, bases, dict, /)`
    #[py_name = "type.__init__"]
    fn type_init_dunder(_args) -> Result<Value> {
        Ok(Value::none())
    }

    /// Issue #2128: `type.__prepare__(mcs, name, bases, /, **kwds)` — the
    /// default metaclass namespace factory.  CPython exposes this as a
    /// classmethod returning a fresh plain `dict`; a `class` statement (and the
    /// `exec_make_class_meta` path) calls `metaclass.__prepare__(...)` before
    /// running the class body.  Registering it makes `hasattr(type, '__prepare__')`
    /// true and lets `super().__prepare__(...)` resolve in a custom metaclass.
    #[py_name = "type.__prepare__"]
    fn type_prepare_dunder(_args) -> Result<Value> {
        Ok(Value::dict(PyDict::default()))
    }

    /// Issue #1956: `type.__call__(cls, *args, **kwargs)` — the default
    /// instance-construction protocol.  Runs `cls.__new__` + `cls.__init__`.
    /// Reached when `super().__call__(*args)` inside a metaclass `__call__`
    /// override chains (via the metaclass MRO) to the default `type.__call__`
    /// bound to the class being constructed.  This is the same default-construct
    /// path as a plain `Cls()` (both go through `Interpreter::default_construct`).
    ///
    /// CPython signature: `type.__call__(self, /, *args, **kwargs)` where
    /// `self` is the class being instantiated.
    #[py_name = "type.__call__"]
    fn type_call_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                "type.__call__(): not enough arguments".to_string(),
            ));
        }
        let cls_val = args[0].value.clone();
        let class = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    "type.__call__(): first argument must be a type".to_string(),
                ));
            }
        };
        _interp.default_construct(class, &args[1..])
    }

    /// Issue #1143: `tuple.__new__(cls, iterable=())` — allocator for tuple
    /// subclasses.  Creates a `PyInstance` of `cls` with the tuple backing
    /// store (`__builtin_data__`) populated from `iterable`.  Called when
    /// a `tuple` subclass's `__new__` calls `super().__new__(cls, it)`.
    ///
    /// CPython signature: `tuple.__new__(cls, iterable=(), /)`
    #[py_name = "tuple.__new__"]
    fn tuple_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "tuple.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "tuple.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "tuple")?;
        let backing = match rest {
            [] => Value::tuple(vec![]),
            [single] => Value::tuple(_interp.collect_iterable(&single.value)?),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "tuple expected at most 1 argument, got {}",
                        rest.len()
                    ),
                ));
            }
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #1143: `frozenset.__new__(cls, iterable=())` — allocator for
    /// frozenset subclasses.  Creates a `PyInstance` of `cls` with the
    /// frozenset backing store (`__builtin_data__`) populated from `iterable`.
    ///
    /// CPython signature: `frozenset.__new__(cls, iterable=(), /)`
    #[py_name = "frozenset.__new__"]
    fn frozenset_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "frozenset.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "frozenset.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "frozenset")?;
        let backing = match rest {
            [] => pyrust_builtins::frozenset::frozenset(PySet::default()),
            [single] => {
                let items = _interp.collect_iterable(&single.value)?;
                let mut set: PySet = PySet::default();
                for item in items {
                    let key = _interp.value_to_pykey(&item)?;
                    _interp.set_insert(&mut set, key)?;
                }
                pyrust_builtins::frozenset::frozenset(set)
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "frozenset expected at most 1 argument, got {}",
                        rest.len()
                    ),
                ));
            }
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #2619: `bool.__new__(cls, x=False)` — applies truthiness
    /// conversion and returns a canonical bool.  `bool` is final in CPython,
    /// so `cls` is always `bool` and the result is `True if x else False`.
    /// Without this dedicated handler `bool.__new__` would inherit
    /// `int.__new__`, returning an int-backed value tagged as bool.
    ///
    /// CPython signature: `bool.__new__(cls, x=False, /)`
    #[py_name = "bool.__new__"]
    fn bool_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "bool.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "bool.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "bool")?;
        match rest {
            [] => Ok(Value::bool_(false)),
            [x] => Ok(Value::bool_(_interp.truthy_value(&x.value)?)),
            _ => Err(PyError::named(
                "TypeError",
                format!("bool expected at most 1 argument, got {}", rest.len()),
            )),
        }
    }

    /// Issue #1465: `int.__new__(cls, x=0)` — allocator for int subclasses.
    /// Creates a `PyInstance` of `cls` with the int backing store
    /// (`__builtin_data__`) populated from the constructor arguments.
    /// Called when an `int` subclass's `__new__` calls `super().__new__(cls, val)`.
    ///
    /// CPython signature: `int.__new__(cls, x=0, /)`
    #[py_name = "int.__new__"]
    fn int_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "int.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "int.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "int")?;
        let backing = if let Some(dispatch) = crate::builtin_registry::lookup("int") {
            dispatch(_interp, rest)?
        } else {
            return Err(PyError::Runtime("internal: int not in registry".to_string()));
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #1465: `str.__new__(cls, object='')` — allocator for str subclasses.
    /// Creates a `PyInstance` of `cls` with the str backing store
    /// (`__builtin_data__`) populated from the constructor arguments.
    /// Called when a `str` subclass's `__new__` calls `super().__new__(cls, val)`.
    ///
    /// CPython signature: `str.__new__(cls, object='', /)`
    #[py_name = "str.__new__"]
    fn str_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "str.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "str.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "str")?;
        let backing = if let Some(dispatch) = crate::builtin_registry::lookup("str") {
            dispatch(_interp, rest)?
        } else {
            return Err(PyError::Runtime("internal: str not in registry".to_string()));
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #1465: `float.__new__(cls, x=0.0)` — allocator for float subclasses.
    /// Creates a `PyInstance` of `cls` with the float backing store
    /// (`__builtin_data__`) populated from the constructor arguments.
    /// Called when a `float` subclass's `__new__` calls `super().__new__(cls, val)`.
    ///
    /// CPython signature: `float.__new__(cls, x=0.0, /)`
    #[py_name = "float.__new__"]
    fn float_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "float.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "float.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "float")?;
        let backing = if let Some(dispatch) = crate::builtin_registry::lookup("float") {
            dispatch(_interp, rest)?
        } else {
            return Err(PyError::Runtime(
                "internal: float not in registry".to_string(),
            ));
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #1465: `bytes.__new__(cls, source=b'')` — allocator for bytes subclasses.
    /// Creates a `PyInstance` of `cls` with the bytes backing store
    /// (`__builtin_data__`) populated from the constructor arguments.
    /// Called when a `bytes` subclass's `__new__` calls `super().__new__(cls, val)`.
    ///
    /// CPython signature: `bytes.__new__(cls, source=b'', /)`
    #[py_name = "bytes.__new__"]
    fn bytes_new_dunder(args) -> Result<Value> {
        let (cls_val, rest) = match args {
            [] => {
                return Err(PyError::named(
                    "TypeError",
                    "bytes.__new__(): not enough arguments".to_string(),
                ));
            }
            [first, rest @ ..] => (first.value.clone(), rest),
        };
        let class_rc = match cls_val.kind() {
            ValueKind::PyClass(c) => Rc::clone(c),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "bytes.__new__(X): X is not a type object ({})",
                        value_type_name_str(&cls_val)
                    ),
                ));
            }
        };
        check_new_subtype(&class_rc, "bytes")?;
        let backing = if let Some(dispatch) = crate::builtin_registry::lookup("bytes") {
            dispatch(_interp, rest)?
        } else {
            return Err(PyError::Runtime(
                "internal: bytes not in registry".to_string(),
            ));
        };
        let mut attrs = InstanceAttrs::new();
        attrs.insert(
            crate::interpreter::BUILTIN_DATA_ATTR,
            backing,
        );
        Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
            crate::value::PyInstance {
                class: class_rc,
                attrs,
            },
        ))))
    }

    /// Issue #1256: `object.__lt__`, `__le__`, `__gt__`, `__ge__` — ordering
    /// comparisons not defined on object; all return `NotImplemented`.
    ///
    /// CPython signature: `object.__lt__(self, value, /)`
    #[py_name = "object.__lt__"]
    fn object_lt_dunder(_args) -> Result<Value> {
        Ok(Value::not_implemented())
    }

    /// CPython signature: `object.__le__(self, value, /)`
    #[py_name = "object.__le__"]
    fn object_le_dunder(_args) -> Result<Value> {
        Ok(Value::not_implemented())
    }

    /// CPython signature: `object.__gt__(self, value, /)`
    #[py_name = "object.__gt__"]
    fn object_gt_dunder(_args) -> Result<Value> {
        Ok(Value::not_implemented())
    }

    /// CPython signature: `object.__ge__(self, value, /)`
    #[py_name = "object.__ge__"]
    fn object_ge_dunder(_args) -> Result<Value> {
        Ok(Value::not_implemented())
    }

    /// Issue #1256: `object.__format__(self, format_spec)`.
    ///
    /// CPython's default implementation calls `str(self)` then applies the
    /// format spec.
    ///
    /// CPython signature: `object.__format__(self, format_spec, /)`
    #[py_name = "object.__format__"]
    fn object_format_dunder(args) -> Result<Value> {
        // Issue #2299: `object.__format__` / the inherited `bytes.__format__`
        // (both resolve to this slot) take no keyword arguments.  CPython names
        // the slot owner `object` regardless of the calling type, so
        // `bytes.__format__(b"", "", k=1)` reports `object.__format__()`.
        if args.iter().any(|a| a.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                "object.__format__() takes no keyword arguments".to_string(),
            ));
        }
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__format__", "object", method)
        })?;
        let spec_str = if args.len() >= 2 {
            match args[1].value.kind() {
                ValueKind::Str(s) => s.to_string(),
                _ => return Err(PyError::named(
                    "TypeError",
                    "format_spec must be a string".to_string(),
                )),
            }
        } else {
            String::new()
        };
        // A builtin subclass (`class I(int)`, `class S(str)`, …) that does not
        // override `__format__` resolves `super().__format__(spec)` and the
        // method-call form `inst.__format__(spec)` to *this* `object.__format__`
        // body, because the backing primitive's `__format__` is not exposed as a
        // distinct class attribute in pyrust's MRO.  CPython instead resolves
        // them to the backing type's `__format__` (`int.__format__`), which
        // formats the underlying value.  Emulate that by delegating to the
        // backing formatter when the receiver carries `__builtin_data__`, so
        // `super().__format__('x')` / `I(255).__format__('x')` → `'ff'`
        // (issues #2211, #2214).  The error names the actual subclass, not the
        // backing primitive (issue #2212).
        if let Some(backing) = builtin_data_backing(&self_val) {
            let owner = value_type_name_str(&self_val);
            return apply_format_spec_named(&backing, &spec_str, Some(&owner));
        }
        // CPython raises TypeError when a non-empty spec is passed to
        // object.__format__ on a value with no backing primitive (a pure user
        // class or `object()` itself).
        if !spec_str.is_empty() {
            let type_name = value_type_name_str(&self_val);
            return Err(PyError::named(
                "TypeError",
                format!("unsupported format string passed to {}.__format__", type_name),
            ));
        }
        let s = render_instance_str(_interp, &self_val)?;
        // apply_format_spec takes &Value; wrap the str result temporarily.
        apply_format_spec(&Value::string(s), &spec_str)
    }

    /// Issue #1256: `int.__add__(self, value)` — exposes `int.__add__` as a
    /// class-level attribute so that `int.__add__(1, 2)` and
    /// `hasattr(int, '__add__')` work.  Returns `NotImplemented` when the
    /// right-hand operand is not an integer type, matching CPython's C slot
    /// which only handles int/bool/BigInt and delegates float/str/other to the
    /// reflected operator on the right-hand side.
    ///
    /// CPython signature: `int.__add__(self, value, /)`
    #[py_name = "int.__add__"]
    fn int_add_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__add__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Add, b)
    }

    /// Issue #1256: `int.__sub__(self, value)`
    #[py_name = "int.__sub__"]
    fn int_sub_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__sub__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Sub, b)
    }

    /// Issue #1256: `int.__mul__(self, value)`
    #[py_name = "int.__mul__"]
    fn int_mul_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__mul__", "int")),
        };
        // CPython's int.__mul__ only accepts integer types; string repetition
        // (1 * "x" = "x") is dispatched via str.__rmul__, not here.
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Mul, b)
    }

    /// Issue #1256: `int.__truediv__(self, value)`
    #[py_name = "int.__truediv__"]
    fn int_truediv_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__truediv__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Div, b)
    }

    /// Issue #1256: `int.__floordiv__(self, value)`
    #[py_name = "int.__floordiv__"]
    fn int_floordiv_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__floordiv__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::FloorDiv, b)
    }

    /// Issue #1256: `int.__mod__(self, value)`
    #[py_name = "int.__mod__"]
    fn int_mod_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__mod__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Mod, b)
    }

    /// Issue #1256: `int.__pow__(self, value)`
    #[py_name = "int.__pow__"]
    fn int_pow_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__pow__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Pow, b)
    }

    /// Issue #1256: `int.__and__(self, value)`
    #[py_name = "int.__and__"]
    fn int_and_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__and__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::BitAnd, b)
    }

    /// Issue #1256: `int.__or__(self, value)`
    #[py_name = "int.__or__"]
    fn int_or_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__or__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::BitOr, b)
    }

    /// Issue #1256: `int.__xor__(self, value)`
    #[py_name = "int.__xor__"]
    fn int_xor_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__xor__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::BitXor, b)
    }

    /// Issue #1256: `int.__lshift__(self, value)`
    #[py_name = "int.__lshift__"]
    fn int_lshift_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__lshift__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::LShift, b)
    }

    /// Issue #1256: `int.__rshift__(self, value)`
    #[py_name = "int.__rshift__"]
    fn int_rshift_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__rshift__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::RShift, b)
    }

    /// Issue #1256: `int.__lt__(self, value)`
    #[py_name = "int.__lt__"]
    fn int_lt_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__lt__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Lt, b)
    }

    /// Issue #1256: `int.__le__(self, value)`
    #[py_name = "int.__le__"]
    fn int_le_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__le__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Le, b)
    }

    /// Issue #1256: `int.__gt__(self, value)`
    #[py_name = "int.__gt__"]
    fn int_gt_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__gt__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Gt, b)
    }

    /// Issue #1256: `int.__ge__(self, value)`
    #[py_name = "int.__ge__"]
    fn int_ge_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ge__", "int")),
        };
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Ge, b)
    }

    /// Issue #1256: `int.__eq__(self, value)`
    #[py_name = "int.__eq__"]
    fn int_eq_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__eq__", "int")),
        };
        // CPython's int.__eq__ returns NotImplemented for non-integer types;
        // pyrust's eval_binary(Eq) falls through to values_user_eq which
        // returns False for cross-type comparisons without raising TypeError.
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Eq, b)
    }

    /// Issue #1256: `int.__ne__(self, value)`
    #[py_name = "int.__ne__"]
    fn int_ne_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ne__", "int")),
        };
        // CPython's int.__ne__ returns NotImplemented for non-integer types.
        if !matches!(b.kind(), ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Ne, b)
    }

    /// Issue #1452: `float.__trunc__(self)` — registered so that float subclasses
    /// inherit it via MRO and `math.trunc(MyFloat(x))` dispatches correctly.
    ///
    /// CPython: `float.__trunc__` truncates toward zero and returns an `int`.
    /// Raises `OverflowError` for infinity, `ValueError` for NaN.
    #[py_name = "float.__trunc__"]
    fn float_trunc_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__trunc__", "float", method)
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Float(f) => {
                if f.is_nan() {
                    return Err(PyError::named(
                        "ValueError",
                        "cannot convert float NaN to integer".to_string(),
                    ));
                }
                if f.is_infinite() {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot convert float infinity to integer".to_string(),
                    ));
                }
                let t = f.trunc();
                // i64::MAX as f64 rounds up to 2^63, so ">=" is required: any float
                // >= 2^63 cannot fit in an i64 and must go through BigInt.
                if t >= i64::MAX as f64 || t < i64::MIN as f64 {
                    float_to_bigint(t)
                } else {
                    Ok(Value::int(t as i64))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor '__trunc__' for 'float' objects doesn't apply to a '{}' object",
                    value_type_name_str(&self_val)
                ),
            )),
        }
    }

    /// Issue #1452: `float.__floor__(self)` — registered so that float subclasses
    /// inherit it via MRO and `math.floor(MyFloat(x))` dispatches correctly.
    ///
    /// CPython: `float.__floor__` rounds toward negative infinity and returns an `int`.
    /// Raises `OverflowError` for infinity, `ValueError` for NaN.
    #[py_name = "float.__floor__"]
    fn float_floor_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__floor__", "float", method)
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Float(f) => {
                if f.is_nan() {
                    return Err(PyError::named(
                        "ValueError",
                        "cannot convert float NaN to integer".to_string(),
                    ));
                }
                if f.is_infinite() {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot convert float infinity to integer".to_string(),
                    ));
                }
                let floor = f.floor();
                // i64::MAX as f64 rounds up to 2^63, so ">=" is required: any float
                // >= 2^63 cannot fit in an i64 and must go through BigInt.
                if floor >= i64::MAX as f64 || floor < i64::MIN as f64 {
                    float_to_bigint(floor)
                } else {
                    Ok(Value::int(floor as i64))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor '__floor__' for 'float' objects doesn't apply to a '{}' object",
                    value_type_name_str(&self_val)
                ),
            )),
        }
    }

    /// Issue #1452: `float.__ceil__(self)` — registered so that float subclasses
    /// inherit it via MRO and `math.ceil(MyFloat(x))` dispatches correctly.
    ///
    /// CPython: `float.__ceil__` rounds toward positive infinity and returns an `int`.
    /// Raises `OverflowError` for infinity, `ValueError` for NaN.
    #[py_name = "float.__ceil__"]
    fn float_ceil_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__ceil__", "float", method)
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Float(f) => {
                if f.is_nan() {
                    return Err(PyError::named(
                        "ValueError",
                        "cannot convert float NaN to integer".to_string(),
                    ));
                }
                if f.is_infinite() {
                    return Err(PyError::named(
                        "OverflowError",
                        "cannot convert float infinity to integer".to_string(),
                    ));
                }
                let ceil = f.ceil();
                // i64::MAX as f64 rounds up to 2^63, so ">=" is required: any float
                // >= 2^63 cannot fit in an i64 and must go through BigInt.
                if ceil >= i64::MAX as f64 || ceil < i64::MIN as f64 {
                    float_to_bigint(ceil)
                } else {
                    Ok(Value::int(ceil as i64))
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor '__ceil__' for 'float' objects doesn't apply to a '{}' object",
                    value_type_name_str(&self_val)
                ),
            )),
        }
    }

    /// Issue #1256: `str.__len__(self)` — exposes `str.__len__` as a class-level
    /// attribute so that `str.__len__("hello")` and `hasattr(str, '__len__')`
    /// work.
    ///
    /// CPython signature: `str.__len__(self, /)`
    #[py_name = "str.__len__"]
    fn str_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "str")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Str(s) => Ok(Value::int(s.chars().count() as i64)),
            _ => Err(pyrust_core::descriptor_requires!("__len__", "str", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1256: `str.__add__(self, value)`
    #[py_name = "str.__add__"]
    fn str_add_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__add__", "str")),
        };
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Add, b)
    }

    /// Issue #1256: `str.__mul__(self, value)`
    #[py_name = "str.__mul__"]
    fn str_mul_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__mul__", "str")),
        };
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Mul, b)
    }

    /// Issue #1256: `str.__lt__(self, value)`
    #[py_name = "str.__lt__"]
    fn str_lt_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__lt__", "str")),
        };
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Lt, b)
    }

    /// Issue #1256: `str.__le__(self, value)`
    #[py_name = "str.__le__"]
    fn str_le_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__le__", "str")),
        };
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Le, b)
    }

    /// Issue #1256: `str.__gt__(self, value)`
    #[py_name = "str.__gt__"]
    fn str_gt_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__gt__", "str")),
        };
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Gt, b)
    }

    /// Issue #1256: `str.__ge__(self, value)`
    #[py_name = "str.__ge__"]
    fn str_ge_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ge__", "str")),
        };
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Ge, b)
    }

    /// Issue #1256: `str.__eq__(self, value)`
    #[py_name = "str.__eq__"]
    fn str_eq_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__eq__", "str")),
        };
        // CPython's str.__eq__ returns NotImplemented for non-str types;
        // eval_binary(Eq) falls through to values_user_eq which returns False.
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Eq, b)
    }

    /// Issue #1256: `str.__ne__(self, value)`
    #[py_name = "str.__ne__"]
    fn str_ne_dunder(args) -> Result<Value> {
        let (a, b) = match args {
            [a, b, ..] => (a.value.clone(), b.value.clone()),
            _ => return Err(pyrust_core::descriptor_needs_arg!("__ne__", "str")),
        };
        // CPython's str.__ne__ returns NotImplemented for non-str types.
        if !matches!(b.kind(), ValueKind::Str(_)) {
            return Ok(Value::not_implemented());
        }
        _interp.eval_binary(coerce_numeric(&a), BinaryOp::Ne, b)
    }

    /// Issue #1256: `list.__len__(self)` — exposes `list.__len__` as a
    /// class-level attribute.
    ///
    /// CPython signature: `list.__len__(self, /)`
    #[py_name = "list.__len__"]
    fn list_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "list")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::List(items) => Ok(Value::int(items.len() as i64)),
            // Issue #1434: list subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::List(items)) => Ok(Value::int(items.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "list", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "list", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1256: `tuple.__len__(self)`
    #[py_name = "tuple.__len__"]
    fn tuple_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "tuple")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Tuple(items) => Ok(Value::int(items.len() as i64)),
            // Issue #1434: tuple subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::Tuple(items)) => Ok(Value::int(items.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "tuple", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "tuple", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1256: `dict.__len__(self)`
    #[py_name = "dict.__len__"]
    fn dict_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "dict")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Dict(items) => Ok(Value::int(items.len() as i64)),
            // Issue #1434: dict subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::Dict(items)) => Ok(Value::int(items.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "dict", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "dict", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1390: `dict.fromkeys(iterable[, value])` — classmethod that creates a
    /// new dict with keys from `iterable` and all values set to `value` (default
    /// `None`).
    ///
    /// CPython rules (3.12):
    ///   - At most two positional arguments; no keyword arguments.
    ///   - Duplicate keys: the first occurrence wins for insertion order.
    ///   - Unhashable keys raise `TypeError`.
    ///
    /// Subclass dispatch (`MyDict.fromkeys(...)` returning a `MyDict`) is not
    /// yet implemented; this always returns a plain `dict`.
    #[py_name = "dict.fromkeys"]
    fn dict_fromkeys(args) -> Result<Value> {
        let has_kw = args.iter().any(|a| a.name.is_some());
        if has_kw {
            return Err(PyError::named(
                "TypeError",
                "dict.fromkeys() takes no keyword arguments".to_string(),
            ));
        }
        // Filter out any leading PyClass arg (guard for safety; not normally present).
        let user_args: Vec<&ExpandedCallArg> = args
            .iter()
            .filter(|a| !matches!(a.value.kind(), ValueKind::PyClass(_)))
            .collect();
        if user_args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "fromkeys expected at most 2 arguments, got {}",
                    user_args.len()
                ),
            ));
        }
        let iterable = match user_args.first() {
            Some(a) => a.value.clone(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "fromkeys expected at least 1 argument, got 0".to_string(),
                ));
            }
        };
        let default_val = user_args
            .get(1)
            .map(|a| a.value.clone())
            .unwrap_or_else(Value::none);

        let keys = _interp.collect_iterable(&iterable)?;
        let mut map: PyDict =
            PyDict::with_capacity_and_hasher(keys.len(), Default::default());
        for key in keys {
            let py_key = _interp.value_to_pykey(&key)?;
            // #1914: `dict_insert` dedups `PyKey::Object` keys via user `__eq__`
            // (raw `IndexMap` identity for primitive keys keeps the fast path).
            // The value is always the same default, so last-wins == first-wins;
            // `IndexMap::insert` preserves the first-occurrence position.
            _interp.dict_insert(&mut map, py_key, default_val.clone())?;
        }
        Ok(Value::dict(map))
    }

    /// Issue #1256: `set.__len__(self)`
    #[py_name = "set.__len__"]
    fn set_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "set")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Set(items) => Ok(Value::int(items.len() as i64)),
            // Issue #1434: set subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::Set(items)) => Ok(Value::int(items.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "set", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "set", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1256: `bytes.__len__(self)`
    #[py_name = "bytes.__len__"]
    fn bytes_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "bytes")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::Bytes(b) => Ok(Value::int(b.len() as i64)),
            // Issue #1434: bytes subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::Bytes(b)) => Ok(Value::int(b.len() as i64)),
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "bytes", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "bytes", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1548: `frozenset.__len__(self)`
    #[py_name = "frozenset.__len__"]
    fn frozenset_len_dunder(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__len__", "frozenset")
        })?;
        let self_val = coerce_numeric(&self_val);
        match self_val.kind() {
            ValueKind::BuiltinObject { ops, state }
                if ops.type_name() == pyrust_builtins::frozenset::TYPE_NAME =>
            {
                Ok(Value::int(ops.len(state).unwrap_or(0) as i64))
            }
            // Issue #1548: frozenset subclasses arrive as PyInstance; delegate to backing data.
            ValueKind::PyInstance(inst) => {
                let inst_rc = Rc::clone(inst);
                match instance_builtin_data(&inst_rc).as_ref().map(|v| v.kind()) {
                    Some(ValueKind::BuiltinObject { ops, state })
                        if ops.type_name() == pyrust_builtins::frozenset::TYPE_NAME =>
                    {
                        Ok(Value::int(ops.len(state).unwrap_or(0) as i64))
                    }
                    _ => Err(pyrust_core::descriptor_requires!("__len__", "frozenset", inst_rc.borrow().class.borrow().name)),
                }
            }
            _ => Err(pyrust_core::descriptor_requires!("__len__", "frozenset", value_type_name_str(&self_val))),
        }
    }

    /// Issue #1254: `object.__getattribute__(self, name)` — the default
    /// attribute lookup used by all instances that do not override
    /// `__getattribute__`.  Performs the standard descriptor protocol (data
    /// descriptor -> instance dict -> non-data descriptor / class attr ->
    /// __getattr__ fallback) without re-invoking the `__getattribute__`
    /// dispatch, so `object.__getattribute__(self, name)` inside a custom
    /// `__getattribute__` terminates the MRO walk cleanly.
    ///
    /// CPython signature: `object.__getattribute__(self, name, /)`
    #[py_name = "object.__getattribute__"]
    fn object_getattribute(args) -> Result<Value> {
        // CPython error messages for argument count mismatches:
        //   0 args: "descriptor '__getattribute__' of 'object' object needs an argument"
        //   1 arg (self only, 0 name args): "expected 1 argument, got 0"
        //   3+ args: "expected 1 argument, got N" where N = args.len() - 1
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__getattribute__", "object"));
        }
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("expected 1 argument, got {}", args.len() - 1),
            ));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                // CPython: "attribute name must be string, not 'TYPE'"
                let type_name = value_type_name_str(&args[1].value);
                return Err(PyError::named(
                    "TypeError",
                    format!("attribute name must be string, not '{type_name}'"),
                ));
            }
        };
        let instance_rc = match args[0].value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => {
                return _interp.get_attr(&args[0].value, &name);
            }
        };
        _interp.get_attr_instance_raw(instance_rc, &name)
    }

    /// Issue #1402: `object.__setattr__(self, name, value)` — the default
    /// attribute setter used by all instances that do not override
    /// `__setattr__`.  Performs the descriptor protocol (__set__) then writes
    /// to the instance __dict__, without re-invoking `__setattr__` dispatch
    /// (which would cause infinite recursion when called from inside a custom
    /// `__setattr__`).
    ///
    /// CPython signature: `object.__setattr__(self, name, value, /)`
    #[py_name = "object.__setattr__"]
    fn object_setattr_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__setattr__", "object"));
        }
        if args.len() != 3 {
            return Err(PyError::named(
                "TypeError",
                format!(" expected 2 arguments, got {}", args.len() - 1),
            ));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                let type_name = value_type_name_str(&args[1].value);
                return Err(PyError::named(
                    "TypeError",
                    format!("attribute name must be string, not '{type_name}'"),
                ));
            }
        };
        let value = args[2].value.clone();
        match args[0].value.kind() {
            ValueKind::PyInstance(rc) => {
                let instance_rc = Rc::clone(rc);
                _interp.assign_attr_instance_raw(instance_rc, &name, value)?;
            }
            _ => {
                // CPython raises AttributeError for non-instance values (int, str,
                // list, etc.) — their slots are immutable from Python.  The
                // general assign_attr catch-all returns RuntimeError here, which
                // is wrong; emit the same message CPython does instead.
                let type_name = value_type_name_str(&args[0].value);
                return Err(PyError::named(
                    "AttributeError",
                    format!("'{type_name}' object has no attribute '{name}'"),
                ));
            }
        }
        Ok(Value::none())
    }

    /// Issue #1402: `object.__delattr__(self, name)` — the default attribute
    /// deleter used by all instances that do not override `__delattr__`.
    /// Performs the descriptor protocol (__delete__) then removes from the
    /// instance __dict__, without re-invoking `__delattr__` dispatch.
    ///
    /// CPython signature: `object.__delattr__(self, name, /)`
    #[py_name = "object.__delattr__"]
    fn object_delattr_dunder(args) -> Result<Value> {
        if args.is_empty() {
            return Err(pyrust_core::descriptor_needs_arg!("__delattr__", "object"));
        }
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!("expected 1 argument, got {}", args.len() - 1),
            ));
        }
        let name = match args[1].value.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                let type_name = value_type_name_str(&args[1].value);
                return Err(PyError::named(
                    "TypeError",
                    format!("attribute name must be string, not '{type_name}'"),
                ));
            }
        };
        match args[0].value.kind() {
            ValueKind::PyInstance(rc) => {
                let instance_rc = Rc::clone(rc);
                _interp.delete_attr_instance_raw(instance_rc, &name)?;
            }
            _ => {
                // CPython raises AttributeError for non-instance values — same
                // pattern as in object_setattr_dunder; the general delete_attr
                // catch-all returns RuntimeError here, which is wrong.
                let type_name = value_type_name_str(&args[0].value);
                return Err(PyError::named(
                    "AttributeError",
                    format!("'{type_name}' object has no attribute '{name}'"),
                ));
            }
        }
        Ok(Value::none())
    }

    /// Issue #1112: `BaseException.__init__(self, *args)` — updates `self.args`
    /// so that `super().__init__(msg)` in an exception subclass sets the correct
    /// `.args` tuple on the already-constructed instance.  Also mirrors the
    /// `StopIteration.value` special-case from `instantiate_exception`.
    ///
    /// CPython signature: `BaseException.__init__(self, *args)`
    #[py_name = "BaseException.__init__"]
    fn base_exception_init(args) -> Result<Value> {
        let Some(first) = args.first() else {
            return Ok(Value::none());
        };
        let inst_rc = match first.value.kind() {
            ValueKind::PyInstance(rc) => Rc::clone(rc),
            _ => return Ok(Value::none()),
        };
        let exc_args: Vec<Value> = args[1..]
            .iter()
            .filter(|a| a.name.is_none())
            .map(|a| a.value.clone())
            .collect();
        // Update .args on the existing instance.
        inst_rc
            .borrow_mut()
            .attrs
            .insert("args", Value::tuple(exc_args.clone()));
        // Mirror the StopIteration.value special-case.
        let (is_stop_iteration, is_unicode_decode, is_unicode_encode, is_unicode_translate) = {
            let class = Rc::clone(&inst_rc.borrow().class);
            let decode = class_chain_contains_name(&class, "UnicodeDecodeError");
            let encode =
                !decode && class_chain_contains_name(&class, "UnicodeEncodeError");
            let translate = !decode
                && !encode
                && class_chain_contains_name(&class, "UnicodeTranslateError");
            (class_chain_contains_name(&class, "StopIteration"), decode, encode, translate)
        };
        if is_stop_iteration {
            let val = exc_args.into_iter().next().unwrap_or_else(Value::none);
            inst_rc
                .borrow_mut()
                .attrs
                .insert("value", val);
        } else if is_unicode_decode || is_unicode_encode || is_unicode_translate {
            // Mirror the Unicode-error attribute-setting from instantiate_exception
            // so that `super().__init__(enc, obj, start, end, reason)` in a
            // subclass's __init__ sets all five (or four) structured attributes.
            unicode_exc_set_attrs(
                &mut inst_rc.borrow_mut().attrs,
                &exc_args,
                is_unicode_decode || is_unicode_encode,
            );
        }
        Ok(Value::none())
    }

    /// Issue #2361: `BaseException.__reduce__(self)` — the pickle reduction.
    ///
    /// CPython's `BaseException.__reduce__` returns `(type(self), self.args)`,
    /// with a third element (the instance `__dict__`) appended whenever the
    /// instance carries any non-slot attributes.  The C-level exception slots
    /// (`args`, `__traceback__`, `__cause__`, `__context__`,
    /// `__suppress_context__`, and class-specific structured slots) are
    /// excluded from that state dict — which is why `copy`/`deepcopy` of a
    /// caught exception drop the traceback (#2360).
    ///
    /// CPython signature: `BaseException.__reduce__(self, /)`
    #[py_name = "BaseException.__reduce__"]
    fn base_exception_reduce(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__reduce__", "BaseException", method)
        })?;
        Ok(base_exception_reduce_value(&self_val))
    }

    /// Issue #2361: `BaseException.__reduce_ex__(self, protocol)` — CPython
    /// inherits `object.__reduce_ex__`, which for an exception ends up calling
    /// `self.__reduce__()`.  We return the same `(type, args[, state])` tuple so
    /// that the protocol the `copy` module relies on is exception-correct.
    ///
    /// CPython signature: `BaseException.__reduce_ex__(self, protocol, /)`
    #[py_name = "BaseException.__reduce_ex__"]
    fn base_exception_reduce_ex(args) -> Result<Value> {
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__reduce_ex__", "BaseException", method)
        })?;
        Ok(base_exception_reduce_value(&self_val))
    }

    /// Issue #1067: `BaseException.add_note(note)` — Python 3.11+ method.
    ///
    /// Appends `note` (a str) to `self.__notes__`, creating `__notes__` as
    /// a fresh list if it does not yet exist.  Matches CPython 3.12 semantics:
    /// - `note` must be a `str`; otherwise raises `TypeError`.
    /// - Returns `None`.
    /// - `hasattr(exc, "__notes__")` is `False` until `add_note` is called.
    ///
    /// CPython signature: `BaseException.add_note(self, note, /)`
    #[py_name = "BaseException.add_note"]
    fn base_exception_add_note(args) -> Result<Value> {
        // args[0] = self, args[1] = note; exactly one user argument expected.
        let user_argc = args.len().saturating_sub(1);
        if user_argc != 1 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "BaseException.add_note() takes exactly one argument ({user_argc} given)"
                ),
            ));
        }
        // Reject keyword arguments.
        if args.iter().any(|a| a.name.is_some()) {
            return Err(PyError::named(
                "TypeError",
                "BaseException.add_note() takes no keyword arguments".to_string(),
            ));
        }
        let self_val = &args[0].value;
        let note_val = &args[1].value;

        // `note` must be a str.
        let note_str = match note_val.kind() {
            ValueKind::Str(s) => s.to_string(),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "note must be a str, not '{}'",
                        value_type_name_str(note_val)
                    ),
                ));
            }
        };

        // Mutate self.__notes__ in place.
        let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor 'add_note' for 'BaseException' objects doesn't apply to a '{}' object",
                    value_type_name_str(self_val),
                ),
            ));
        };
        // If __notes__ is absent, insert a fresh empty list so we can
        // always call list_push on the value without re-borrowing inst.
        {
            let mut inst = inst_rc.borrow_mut();
            if !inst.attrs.contains_key("__notes__") {
                inst.attrs.insert("__notes__", Value::list(vec![]));
            }
        }
        // Re-borrow immutably to read the list value and push to it.
        // Value::list_push takes &self and uses RefCell internally — no
        // need to hold a mutable borrow on the instance for this step.
        // Read back via `get_cloned` so a dict-backed instance (#1981/#2637)
        // resolves the list we just inserted into its live `__dict__`; a raw
        // `get` (entries only) would miss it and hand back a fresh orphan list,
        // silently dropping every appended note after a `__dict__` swap.
        let notes_val = inst_rc
            .borrow()
            .attrs
            .get_cloned("__notes__")
            .unwrap_or_else(|| Value::list(vec![]));
        notes_val
            .list_push(Value::string(note_str))
            .map_err(|_| {
                PyError::named(
                    "TypeError",
                    "Cannot add note: __notes__ is not a list".to_string(),
                )
            })?;
        Ok(Value::none())
    }

    /// Issue #1441: `BaseException.with_traceback(tb)` — sets `self.__traceback__`
    /// to `tb` and returns `self`.
    ///
    /// CPython 3.12: `tb` must be a traceback object or `None`; anything else
    /// raises `TypeError: __traceback__ must be a traceback or None`.
    ///
    /// CPython signature: `BaseException.with_traceback(self, tb, /)`
    #[py_name = "BaseException.with_traceback"]
    fn base_exception_with_traceback(args) -> Result<Value> {
        if args.len() != 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "BaseException.with_traceback() takes exactly one argument ({} given)",
                    args.len().saturating_sub(1)
                ),
            ));
        }
        let self_val = &args[0].value;
        let tb_val = &args[1].value;
        // tb must be None or a traceback object.
        let ok = match tb_val.kind() {
            ValueKind::None => true,
            ValueKind::BuiltinObject { ops, .. } => {
                ops.type_name() == pyrust_builtins::traceback::TYPE_NAME
            }
            _ => false,
        };
        if !ok {
            return Err(PyError::named(
                "TypeError",
                "__traceback__ must be a traceback or None".to_string(),
            ));
        }
        let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "descriptor 'with_traceback' for 'BaseException' objects doesn't apply to a '{}' object",
                    value_type_name_str(self_val),
                ),
            ));
        };
        inst_rc
            .borrow_mut()
            .attrs
            .insert("__traceback__", tb_val.clone());
        Ok(self_val.clone())
    }

    /// CPython: __import__(name, globals=None, locals=None, fromlist=(), level=0)
    /// <https://docs.python.org/3/library/functions.html#import__>
    ///
    /// The hook behind the import statement. Empty or absent fromlist returns
    /// the top-level package (e.g. os for "os.path"); non-empty fromlist
    /// returns the leaf module directly. globals, locals, and level are
    /// accepted but ignored.
    fn __import__(args) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() missing required argument 'name' (pos 1)"),
            ));
        }
        let name = match args[0].value.as_str() {
            Some(s) => s.to_string(),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    "module name must be a string".to_string(),
                ));
            }
        };
        // CPython raises ValueError for an empty module name.
        if name.is_empty() {
            return Err(PyError::named("ValueError", "Empty module name".to_string()));
        }
        // Arg index 3 is `fromlist`; also accept as a keyword arg.
        let fromlist: Option<&Value> = args.get(3).map(|a| &a.value).or_else(|| {
            args.iter()
                .find(|a| a.name.as_deref() == Some("fromlist"))
                .map(|a| &a.value)
        });
        let fromlist_nonempty = match fromlist {
            None => false,
            Some(v) => match v.kind() {
                ValueKind::None => false,
                ValueKind::Tuple(items) => !items.is_empty(),
                _ => v.as_list().map(|l| !l.is_empty()).unwrap_or(true),
            },
        };
        // Load the full dotted module (triggers caching of submodules).
        let leaf = _interp.load_module(&name)?;
        if fromlist_nonempty {
            // Non-empty fromlist: return the leaf (rightmost component).
            Ok(leaf)
        } else {
            // Empty fromlist: return the top-level package.
            let top_name = name.split('.').next().unwrap_or(&name);
            if top_name == name {
                Ok(leaf)
            } else {
                _interp.load_module(top_name)
            }
        }
    }

    /// PEP 695: `TypeAliasType.__repr__` — returns the alias name string.
    /// CPython: `print(Vector)` outputs just `Vector` (the alias name).
    #[py_name = "builtins.TypeAliasType.__repr__"]
    fn type_alias_type_repr(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "TypeAliasType")
        })?;
        if let ValueKind::PyInstance(inst_rc) = self_val.kind() {
            let borrowed = inst_rc.borrow();
            if let Some(name_val) = borrowed.attrs.get("__name__") {
                return Ok(Value::string(name_val.to_string()));
            }
        }
        Ok(Value::string(self_val.repr_raw()))
    }

    /// PEP 695: `TypeVar.__repr__` — returns the TypeVar name string.
    /// CPython: `repr(T)` outputs `~T` for invariant TypeVars, but the
    /// `__name__` attribute is just the bare name `T`.
    #[py_name = "builtins.TypeVar.__repr__"]
    fn typevar_repr(args) -> Result<Value> {
        let _ = _interp;
        let self_val = args.first().map(|a| a.value.clone()).ok_or_else(|| {
            pyrust_core::descriptor_needs_arg!("__repr__", "TypeVar")
        })?;
        if let ValueKind::PyInstance(inst_rc) = self_val.kind() {
            let borrowed = inst_rc.borrow();
            if let Some(name_val) = borrowed.attrs.get("__name__") {
                return Ok(Value::string(name_val.to_string()));
            }
        }
        Ok(Value::string(self_val.repr_raw()))
    }
}

/// Issue #2623: the `tp_new` subtype guard for primitive `__new__` slots.
///
/// CPython's `int_new` / `str_new` / `tuple_new` / … all start by validating
/// that the explicit `cls` argument is a subtype of the type owning the slot
/// (`PyType_IsSubtype(cls, base)`), raising `TypeError` otherwise.  `base` is
/// the primitive whose `__new__` is being invoked (e.g. `int` for
/// `int.__new__`).  Without this guard, `bool.__new__(int, 5)` or
/// `str.__new__(int)` silently construct a value of the wrong type.
///
/// `class_rc` is the resolved `cls` argument; `base_name` is the primitive
/// slot owner (`"int"`, `"str"`, …).  Returns `Ok(())` when `cls` is a subtype
/// of `base`, otherwise the CPython 3.12 `TypeError`:
///
/// ```text
/// <base>.__new__(<cls>): <cls> is not a subtype of <base>
/// ```
///
/// with the special case for `int.__new__(bool)` (`bool` IS a subtype of `int`
/// but its instances are not safely allocatable via `int.__new__`):
///
/// ```text
/// int.__new__(bool) is not safe, use bool.__new__()
/// ```
fn check_new_subtype(class_rc: &Rc<RefCell<PyClass>>, base_name: &str) -> Result<()> {
    let base = match primitive_class_by_name(base_name) {
        Some(b) => b,
        // Defensive: every caller passes a registered primitive name.
        None => return Ok(()),
    };
    if Rc::ptr_eq(class_rc, &base) {
        return Ok(());
    }
    let cls_name = class_rc.borrow().name.clone();
    // CPython rejects allocating the built-in `bool` through `int.__new__`, even
    // though `issubclass(bool, int)` holds, because `int`'s allocator is "not
    // safe" for `bool`'s singleton storage.  Keyed on the *identity* of the
    // built-in `bool` class (not its name) so a user class named `bool` that
    // genuinely subclasses `int` is still allocatable, matching CPython.
    if base_name == "int"
        && primitive_class_by_name("bool").is_some_and(|b| Rc::ptr_eq(class_rc, &b))
    {
        return Err(PyError::named(
            "TypeError",
            "int.__new__(bool) is not safe, use bool.__new__()".to_string(),
        ));
    }
    if class_is_subclass_of(class_rc, &base) {
        return Ok(());
    }
    Err(PyError::named(
        "TypeError",
        format!("{base_name}.__new__({cls_name}): {cls_name} is not a subtype of {base_name}"),
    ))
}

/// `type(obj)` — the class object for any value, mirroring CPython's
/// `obj.__class__`.  Extracted from the `type` builtin so the `__class__`
/// attribute (issue #2150) and the object-protocol fallback (#2151) share a
/// single source of truth: `obj.__class__ is type(obj)` for every value.
/// Issue #2361: build the `BaseException.__reduce__` tuple for an exception
/// instance: `(type(self), self.args)`, with a third element (the non-slot
/// `__dict__` state) appended only when the instance has any such attributes.
/// Matches CPython 3.12 `BaseException.__reduce__`.
fn base_exception_reduce_value(self_val: &Value) -> Value {
    let cls = value_class(self_val);
    let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
        // Non-instance receiver — preserve the `(type, ())` shape.
        return Value::tuple(vec![cls, Value::tuple(Vec::new())]);
    };
    // `self.args` is stored as the "args" attr (a tuple); default to ().
    let args_val = inst_rc
        .borrow()
        .attrs
        .get("args")
        .cloned()
        .unwrap_or_else(|| Value::tuple(Vec::new()));
    let state = pyrust_builtins::instance_dict::exception_dict_state(inst_rc);
    if state.is_empty() {
        return Value::tuple(vec![cls, args_val]);
    }
    let mut dict: PyDict = PyDict::with_capacity_and_hasher(state.len(), Default::default());
    for (k, v) in state {
        dict.insert(PyKey::str_from(&k), v);
    }
    Value::tuple(vec![cls, args_val, Value::dict(dict)])
}

/// True when `e` is the `TypeError: '<type>' object is not iterable` raised by
/// `collect_iterable` for a value that does not support iteration at all.
///
/// Used by `dict()` to rewrite that specific failure into CPython's
/// `cannot convert dictionary update sequence element #N to a sequence`
/// message, while leaving errors raised *inside* an element's own iteration
/// (e.g. a user `__iter__` that raises) to propagate unchanged.
fn is_not_iterable_error(e: &PyError) -> bool {
    e.class_name_is("TypeError") && e.to_string().ends_with("object is not iterable")
}

pub(crate) fn value_class(obj: &Value) -> Value {
    // User-defined class instances: return the actual Rc so that
    // `type(x) is type(x)` works via Rc::ptr_eq.
    if let ValueKind::PyInstance(inst) = obj.kind() {
        return Value::py_class(Rc::clone(&inst.borrow().class));
    }
    // Issue #462: the migrated primitives (`int`, `str`, …) resolve to their
    // per-thread `PyClass` singletons.
    if let Some(class) = crate::interpreter::primitive_class_for_value(obj) {
        return Value::py_class(class);
    }
    match obj.kind() {
        // Every class is an instance of `type`: `type(int) is type`.
        // Issue #1626: return the stored metatype when set, else the
        // per-thread `type` singleton.
        ValueKind::PyClass(cls_rc) => {
            let meta = cls_rc.borrow().metatype.clone();
            Value::py_class(meta.unwrap_or_else(type_class_singleton))
        }
        ValueKind::UserFunction(f) => match f.kind {
            UserFunctionKind::StaticMethod => Value::builtin_function("staticmethod"),
            UserFunctionKind::ClassMethod => Value::builtin_function("classmethod"),
            _ => Value::py_class(function_type_singleton()),
        },
        ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
            Value::py_class(method_type_singleton())
        }
        ValueKind::BuiltinFunction(name) => {
            // Issue #2397: `type(list.__len__)` is the slot-wrapper descriptor
            // class `wrapper_descriptor`, not `builtin_function_or_method`.
            if pyrust_core::slot_wrapper_dunder(name).is_some() {
                Value::builtin_function("wrapper_descriptor")
            } else if pyrust_core::method_descriptor_name(name).is_some() {
                // Issue #2422: `type(list.append)` is `method_descriptor`.
                Value::builtin_function("method_descriptor")
            } else {
                Value::builtin_function("builtin_function_or_method")
            }
        }
        ValueKind::PyModule(_) => Value::builtin_function("module"),
        ValueKind::SuperProxy { .. }
        | ValueKind::SuperProxyClass { .. }
        | ValueKind::SuperProxyUnbound { .. } => Value::builtin_function("super"),
        ValueKind::Generator(state_rc) => {
            let borrow = state_rc.borrow();
            if borrow.downcast_ref::<MapIter>().is_some() {
                Value::builtin_function("map")
            } else if borrow.downcast_ref::<FilterIter>().is_some() {
                Value::builtin_function("filter")
            } else if borrow.downcast_ref::<ChainFromIterableIter>().is_some() {
                // `chain.from_iterable(...)` is a native iterator, but CPython
                // reports it as an `itertools.chain` instance.  Route `type()`
                // to the real `chain` PyClass captured on import so that
                // `type(chain.from_iterable(...)) is type(chain(...))` (#2370).
                // `chain.from_iterable` can only be reached after `import
                // itertools`, so the singleton is always populated here; the
                // bare-`chain` fallback covers the unreachable un-imported case.
                if let Some(chain_cls) = crate::interpreter::itertools_chain_class() {
                    Value::py_class(chain_cls)
                } else {
                    Value::builtin_function("chain")
                }
            } else if borrow.downcast_ref::<EnumerateIter>().is_some() {
                Value::builtin_function("enumerate")
            } else if borrow.downcast_ref::<ZipIter>().is_some() {
                Value::builtin_function("zip")
            } else if borrow.downcast_ref::<CallableIter>().is_some() {
                Value::builtin_function("callable_iterator")
            } else if borrow.downcast_ref::<GetItemIter>().is_some() {
                Value::builtin_function("iterator")
            } else if borrow.downcast_ref::<BigRangeIter>().is_some() {
                Value::builtin_function("longrange_iterator")
            } else if let Some(native) = borrow.downcast_ref::<NativeIterFrame>() {
                Value::builtin_function(native.type_name)
            } else if borrow.downcast_ref::<AsyncGenASend>().is_some() {
                // The awaitable returned by `__anext__`/`asend` (#2280) reports
                // as `async_generator_asend`, matching CPython.
                Value::builtin_function("async_generator_asend")
            } else if let Some(frame) = borrow.downcast_ref::<GeneratorFrame>() {
                // Coroutines (`async def`, issue #1039) share the Generator
                // value tag but report `type(coro).__name__ == "coroutine"`.
                // An async generator (`async def` containing `yield`, #2280)
                // reports `async_generator`.
                if frame.is_async_generator() {
                    Value::builtin_function("async_generator")
                } else if frame.is_coroutine {
                    Value::builtin_function("coroutine")
                } else {
                    Value::builtin_function("generator")
                }
            } else {
                Value::builtin_function("generator")
            }
        }
        ValueKind::BuiltinObject { ops, .. } => {
            // instance_dict is a live-proxy for obj.__dict__; its Python
            // type is `dict` (same as CPython's actual __dict__).
            if ops.type_name() == pyrust_builtins::instance_dict::TYPE_NAME
                && let Some(dict_class) = crate::interpreter::primitive_class_by_name("dict") {
                    return Value::py_class(dict_class);
                }
            // Issue #2397: a bound builtin slot dunder (`[1].__len__`) is a
            // CPython `method-wrapper`, not `builtin_function_or_method`.
            if pyrust_builtins::bound_method::is_method_wrapper(obj) {
                return Value::builtin_function("method-wrapper");
            }
            // Issue #2733: `type(list[int])` is the `types.GenericAlias` class
            // (a proper PyClass singleton), not a `BuiltinFunction` sentinel, so
            // its repr is `<class 'types.GenericAlias'>`, `__module__` is
            // `'types'`, and `__name__` is `'GenericAlias'`.
            if ops.type_name() == pyrust_builtins::generic_alias::TYPE_NAME {
                return Value::py_class(crate::interpreter::generic_alias_class_singleton());
            }
            Value::builtin_function(ops.type_name())
        }
        // Migrated primitives are handled above via
        // `primitive_class_for_value`; the explicit `unreachable!`
        // documents that and lets rustc verify exhaustiveness.
        ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Str(_)
        | ValueKind::List(_)
        | ValueKind::Tuple(_)
        | ValueKind::Dict(_)
        | ValueKind::Set(_)
        | ValueKind::Bytes(_)
        | ValueKind::Complex(_, _)
        | ValueKind::None
        | ValueKind::NotImplemented
        | ValueKind::Ellipsis
        | ValueKind::Range { .. }
        | ValueKind::BigRange { .. }
        | ValueKind::PyInstance(_) => {
            unreachable!("primitive_class_for_value should have handled this variant")
        }
    }
}

/// Detect the numeric base and build the digits string to parse when
/// `int(s, 0)` is called (base=0 means "auto-detect from prefix").
///
/// CPython 3.12 rules for base=0:
///   - Whitespace must have been stripped before calling.
///   - An optional sign (`+`/`-`) is consumed first, then re-prepended to the
///     returned digits string so that `i64::from_str_radix` handles it.
///   - `0x`/`0X` → base 16; `0b`/`0B` → base 2; `0o`/`0O` → base 8.
///   - `0` alone, or repeated `0`s with no letter prefix → base 10 (value 0).
///   - A leading `0` followed by a non-zero digit (e.g. `"09"`) → None.
///   - Anything else is parsed as base-10 decimal.
///
/// Returns `Some((base, digits))` on success or `None` on error.
fn int_parse_base_zero(s: &str) -> Option<(u32, String)> {
    let (sign, after_sign) = if let Some(rest) = s.strip_prefix('-') {
        ("-", rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        ("", rest) // `+` consumed but not forwarded; from_str_radix doesn't accept it
    } else {
        ("", s)
    };

    let signed = |digits: &str| -> String {
        if sign.is_empty() {
            digits.to_owned()
        } else {
            format!("{sign}{digits}")
        }
    };

    if let Some(rest) = after_sign
        .strip_prefix("0x")
        .or_else(|| after_sign.strip_prefix("0X"))
    {
        if rest.is_empty() {
            return None;
        }
        // PEP 515: a `_` may immediately follow the base prefix (`0x_FF`).
        let digits = pep515_strip_int(rest, true)?;
        if digits.is_empty() {
            return None;
        }
        return Some((16, signed(&digits)));
    }
    if let Some(rest) = after_sign
        .strip_prefix("0b")
        .or_else(|| after_sign.strip_prefix("0B"))
    {
        if rest.is_empty() {
            return None;
        }
        let digits = pep515_strip_int(rest, true)?;
        if digits.is_empty() {
            return None;
        }
        return Some((2, signed(&digits)));
    }
    if let Some(rest) = after_sign
        .strip_prefix("0o")
        .or_else(|| after_sign.strip_prefix("0O"))
    {
        if rest.is_empty() {
            return None;
        }
        let digits = pep515_strip_int(rest, true)?;
        if digits.is_empty() {
            return None;
        }
        return Some((8, signed(&digits)));
    }
    // No letter prefix: PEP 515 underscores only between digits.
    let after_sign = pep515_strip_int(after_sign, false)?;
    // A leading `0` followed by more chars must all be `0`
    // (Python 3 forbids the Python 2 octal syntax `09` etc.).
    if after_sign.starts_with('0') && after_sign.len() > 1 {
        if after_sign.chars().all(|c| c == '0') {
            return Some((10, signed(&after_sign)));
        }
        return None;
    }
    Some((10, signed(&after_sign)))
}

/// Validate PEP 515 underscore placement in the digit portion of an integer
/// literal (after any sign and base prefix have been removed) and return the
/// underscore-stripped digits.
///
/// CPython rule: a `_` must be preceded by a digit and followed by a digit,
/// with the single exception that a `_` may immediately follow a base prefix
/// (`0x_FF`, `0o_17`, `0b_101`).  Leading (without a preceding prefix),
/// trailing, and doubled underscores are all rejected.
///
/// `allow_leading` is `true` when a base prefix was present, permitting the
/// post-prefix underscore.  Returns `None` on any invalid placement.
fn pep515_strip_int(digits: &str, allow_leading: bool) -> Option<String> {
    if digits.is_empty() {
        return Some(String::new());
    }
    let bytes = digits.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'_' {
            // Doubled underscore.
            if i + 1 < bytes.len() && bytes[i + 1] == b'_' {
                return None;
            }
            // Trailing underscore.
            if i + 1 == bytes.len() {
                return None;
            }
            // Leading underscore: only allowed immediately after a prefix.
            if i == 0 && !allow_leading {
                return None;
            }
        }
    }
    Some(digits.chars().filter(|&c| c != '_').collect())
}

/// Validate PEP 515 underscore placement in a float literal string (sign and
/// surrounding whitespace already stripped) and return the underscore-stripped
/// string ready for `f64::from_str`.
///
/// CPython rule for floats: every `_` must be both preceded and followed by a
/// decimal digit.  Underscores adjacent to `.`, `e`/`E`, signs, or the string
/// boundary are invalid, as are doubled underscores.  Returns `None` on any
/// invalid placement.
fn pep515_strip_float(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'_' {
            let prev_ok = i > 0 && bytes[i - 1].is_ascii_digit();
            let next_ok = i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit();
            if !prev_ok || !next_ok {
                return None;
            }
        }
    }
    Some(s.chars().filter(|&c| c != '_').collect())
}

/// Strip the optional sign and matching base prefix from a string passed to
/// `int(str, base)` with an explicit `base` (2 / 8 / 16 accept their prefix;
/// other bases have none), validate PEP 515 underscore placement, and return
/// the sign-prefixed, underscore-stripped digits ready for `from_str_radix`.
///
/// Returns `None` on invalid underscore placement.  Invalid *digits* are left
/// for `from_str_radix` to reject so the caller produces the right message.
fn int_strip_explicit_base(trimmed: &str, base: u32) -> Option<String> {
    let (sign, after_sign) = if let Some(rest) = trimmed.strip_prefix('-') {
        ("-", rest)
    } else if let Some(rest) = trimmed.strip_prefix('+') {
        ("", rest) // `+` consumed but not forwarded; from_str_radix rejects it.
    } else {
        ("", trimmed)
    };

    let (digits_part, has_prefix) = match base {
        16 if after_sign.starts_with("0x") || after_sign.starts_with("0X") => {
            (&after_sign[2..], true)
        }
        2 if after_sign.starts_with("0b") || after_sign.starts_with("0B") => {
            (&after_sign[2..], true)
        }
        8 if after_sign.starts_with("0o") || after_sign.starts_with("0O") => {
            (&after_sign[2..], true)
        }
        _ => (after_sign, false),
    };

    let stripped = pep515_strip_int(digits_part, has_prefix)?;
    if sign.is_empty() {
        Some(stripped)
    } else {
        Some(format!("{sign}{stripped}"))
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
        // `a - modulo` can overflow i64 when `a` is near `i64::MIN` and
        // `modulo > 0` (e.g. `divmod(-2**63, 3)` → modulo=1, a-modulo wraps).
        // `a_adj / b` can overflow when `a = i64::MIN` and `b = -1`
        // (quotient = 2^63, which exceeds i64::MAX).
        // Fall through to BigInt arithmetic in either case.
        if let Some(quotient) = a.checked_sub(modulo).and_then(|a_adj| a_adj.checked_div(b)) {
            return Ok(Value::tuple(vec![Value::int(quotient), Value::int(modulo)]));
        }
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
    // CPython's fmod-based float_divmod handles infinities and signed zeros
    // correctly and keeps divmod consistent with `//` and `%` (#2025).
    let (quotient, modulo) = float_divmod(a, b);
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
        let items = interp.collect_iterable(&v)?;
        Ok(Value::list(items))
    } else {
        Ok(v)
    }
}

/// Convert an arbitrary Python iterable value into an iterator object without
/// consuming any elements.
///
/// Mirrors the single-argument `iter()` builtin logic:
/// - `Generator` values (already-created iterators: map, filter, enumerate,
///   generator objects, etc.) are returned as-is.
/// - `PyInstance` values with `__iter__` have it called; the resulting iterator
///   object is returned.  `PyInstance` values with only `__getitem__` are
///   wrapped in a `GetItemIter`.
/// - All other values (lists, tuples, ranges, dict views, …) are wrapped in a
///   `NativeIterFrame` so they can be advanced one element at a time without
///   materialising the entire sequence upfront.
///
/// Used by `map()` and `filter()` to avoid eagerly exhausting generator sources
/// at construction time (issue #1388).
pub(crate) fn make_iterator(interp: &mut crate::Interpreter, v: &Value) -> Result<Value> {
    enum IterKind {
        Generator,
        PyInstance(Rc<RefCell<crate::value::PyInstance>>),
        BigRange(crate::value::PyBigInt, crate::value::PyBigInt, crate::value::PyBigInt),
        // A `BuiltinObject` that is itself an iterator (`reversed`, `enumerate`,
        // `zip`, `chain`, file objects).  Its `__iter__` returns `self`, so it
        // is returned unchanged and shares position with the original — never
        // re-wrapped in a fresh `NativeIterFrame` (#2117).
        SelfIterator,
        Other,
    }
    // A coroutine (`async def`, issue #1039) — and an async generator (#2280)
    // — is not iterable.
    if is_coroutine_value(v) {
        let tn = full_type_name_str(v);
        return Err(pyrust_core::type_err!("'{tn}' object is not iterable"));
    }
    let kind = match v.kind() {
        ValueKind::Generator(_) => IterKind::Generator,
        ValueKind::PyInstance(inst) => IterKind::PyInstance(Rc::clone(inst)),
        // Arbitrary-precision range (#2118): return a lazy iterator so callers
        // (iter / enumerate / zip) never materialize a huge sequence.
        ValueKind::BigRange { start, stop, step } => {
            IterKind::BigRange(start.clone(), stop.clone(), step.clone())
        }
        ValueKind::BuiltinObject { ops, .. } if ops.is_iterator() => IterKind::SelfIterator,
        _ => IterKind::Other,
    };
    match kind {
        IterKind::Generator | IterKind::SelfIterator => Ok(v.clone()),
        IterKind::BigRange(cur, stop, step) => {
            Ok(Value::generator(Box::new(BigRangeIter { cur, stop, step })))
        }
        IterKind::PyInstance(inst_rc) => {
            let class = Rc::clone(&inst_rc.borrow().class);
            if let Some(method_val) = lookup_class_attr(&class, "__iter__") {
                let iter_obj =
                    invoke_class_method(interp, method_val, Value::py_instance(inst_rc), &[])?;
                let is_valid_iter = match iter_obj.kind() {
                    ValueKind::Generator(_) => true,
                    ValueKind::PyInstance(it) => {
                        let it_class = Rc::clone(&it.borrow().class);
                        lookup_class_attr(&it_class, "__next__").is_some()
                    }
                    ValueKind::BuiltinObject { ops, .. } => ops.is_iterable(),
                    _ => false,
                };
                if !is_valid_iter {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "iter() returned non-iterator of type '{}'",
                            value_type_name_str(&iter_obj),
                        ),
                    ));
                }
                Ok(iter_obj)
            } else if lookup_class_attr(&class, "__getitem__").is_some() {
                interp.make_getitem_iter(inst_rc)
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!("'{}' object is not iterable", class.borrow().name),
                ))
            }
        }
        IterKind::Other => {
            let iter_type_name = builtin_iter_type_name(v);
            let items = iter_values(v).map_err(|_| {
                PyError::named(
                    "TypeError",
                    format!("'{}' object is not iterable", value_type_name_str(v)),
                )
            })?;
            let mut frame = NativeIterFrame::new(items, iter_type_name);
            // dict / set / dict-views: guard the manual `iter()` iterator
            // against size mutation, mirroring the `for`-loop guard (#1988).
            if let Some(recorded_len) = crate::interpreter::live_collection_len(v) {
                let msg = if v.set_len().is_some() {
                    "Set changed size during iteration"
                } else {
                    "dictionary changed size during iteration"
                };
                frame.guard = Some(Box::new(NativeIterGuard {
                    container: v.clone(),
                    version: recorded_len as i64,
                    kind: GuardVersion::Size,
                    msg,
                    exhaust_first: false,
                    od_seq: 0,
                }));
            }
            Ok(Value::generator(Box::new(frame)))
        }
    }
}

/// Build the `reversed()` iterator for a `dict` or one of its views (#2448).
///
/// `items` must already be in *reverse* order (the caller materialises the
/// forward key/value/pair list and reverses it).  `container` is the live
/// `dict` Value or dict-view whose size is re-read on each step: like CPython's
/// forward dict iterators, mutating the dict's size during the `reversed()`
/// walk raises `RuntimeError` on the next `next()` call.  The wording and
/// check-ordering follow the OrderedDict-aware convention shared with the
/// forward path (`is_ordered_view`): OrderedDict-backed views test exhaustion
/// before the guard (`exhaust_first`), plain dicts test the guard first.
fn make_reversed_dict_iter(items: Vec<Value>, container: Value) -> NativeIterFrame {
    let recorded_len = items.len();
    let ordered = pyrust_builtins::dict_views::is_ordered_view(&container);
    // CPython names reversed OrderedDict iterators `odict_iterator` — the same
    // type as a forward OrderedDict iterator, shared across keys/values/items
    // views (issue #2741).  Plain-dict reversed iterators are CPython's
    // `dict_reversekeyiterator` etc.; pyrust still reports the generic
    // `list_reverseiterator` for those (a pre-existing type-name divergence,
    // out of scope for #2448 / #2741).
    let type_name = if ordered { "odict_iterator" } else { "list_reverseiterator" };
    let mut frame = NativeIterFrame::new(items, type_name);
    let (msg, exhaust_first) = if ordered {
        ("OrderedDict mutated during iteration", true)
    } else {
        ("dictionary changed size during iteration", false)
    };
    // issue #2465: ordered reversed-views snapshot the clear tick so a `clear()`
    // mid-`reversed(od)` reports "changed size".
    let od_seq = if ordered { crate::interpreter::od_clear_seq_now() } else { 0 };
    frame.guard = Some(Box::new(NativeIterGuard {
        container,
        version: recorded_len as i64,
        kind: GuardVersion::Size,
        msg,
        exhaust_first,
        od_seq,
    }));
    frame
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

// ── CPython 3.12 tuple / slice hash kernels ──────────────────────────────────
// tuplehash (Python 3.8+): xxHash-based, Objects/tupleobject.c
//   acc = PRIME5
//   for each item:  acc += lane*PRIME2; acc = rotl31(acc); acc *= PRIME1
//   acc += n ^ (PRIME5 ^ 3527539)
//   if acc == u64::MAX: acc = 1546275796
//   return acc as i64
//
// slice_hash (CPython 3.12, Objects/sliceobject.c): same kernel but WITHOUT
// the final length-mixing step.
//
// Both functions share the same per-element accumulation step via `xxstep` so
// the two paths can't silently diverge if the kernel is ever updated.

const XX_PRIME1: u64 = 11400714785074694791;
const XX_PRIME2: u64 = 14029467366897019727;
const XX_PRIME5: u64 = 2870177450012600261;

#[inline(always)]
fn xxstep(acc: u64, lane: u64) -> u64 {
    let acc = acc.wrapping_add(lane.wrapping_mul(XX_PRIME2));
    let acc = acc.rotate_left(31); // rotl31
    acc.wrapping_mul(XX_PRIME1)
}

fn tuple_hash_cpython(items: impl Iterator<Item = Result<i64>>) -> Result<i64> {
    let mut acc: u64 = XX_PRIME5;
    let mut n: u64 = 0;
    for h in items {
        acc = xxstep(acc, h? as u64);
        n += 1;
    }
    acc = acc.wrapping_add(n ^ (XX_PRIME5 ^ 3527539u64));
    if acc == u64::MAX {
        acc = 1546275796;
    }
    Ok(acc as i64)
}

fn slice_hash_cpython(items: impl Iterator<Item = Result<i64>>) -> Result<i64> {
    let mut acc: u64 = XX_PRIME5;
    for h in items {
        acc = xxstep(acc, h? as u64);
    }
    if acc == u64::MAX {
        acc = 1546275796;
    }
    Ok(acc as i64)
}

/// Compute the hash of a `Value` for the `hash()` builtin.  Mirrors
/// CPython's semantics:
/// - numeric types use their integer value (so `hash(True) == hash(1)`
///   and `hash(1.0) == hash(1)`);
/// - strings use an FNV-1a-style byte hash;
/// - tuples use the CPython 3.12 xxHash-based formula (issue #892);
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
        ValueKind::Bytes(rc) => {
            // FNV-1a over the raw byte content, matching PyKey::Bytes hashing
            // so that py_hash_pykey(v.to_key()) == hash(v) for bytes values.
            let mut h: u64 = 14695981039346656037u64;
            for b in rc.iter() {
                h ^= *b as u64;
                h = h.wrapping_mul(1099511628211u64);
            }
            let result = h as i64;
            Ok(if result == -1 { -2 } else { result })
        }
        ValueKind::None => Ok(pyrust_core::py_hash_none()),
        ValueKind::NotImplemented => Ok(pyrust_core::py_hash_not_implemented()),
        ValueKind::Ellipsis => Ok(pyrust_core::py_hash_ellipsis()),
        ValueKind::Tuple(items) => {
            tuple_hash_cpython(items.iter().map(hash_value))
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
        // Range: mirrors CPython Objects/rangeobject.c range_hash (Ubuntu 3.12.3).
        //
        // CPython always builds a 3-element tuple (length, a, b) and hashes it:
        //   len == 0 -> hash((len, None, None))
        //   len == 1 -> hash((len, start, None))
        //   len  > 1 -> hash((len, start, step))
        //
        // `py_hash_int` applies the Mersenne-prime reduction and -1→-2 remap for
        // integer components; `py_hash_none` mirrors CPython's pointer-based
        // `hash(None)` using a stable per-process static address.
        ValueKind::Range { start, stop, step } => {
            let len = range_len(start, stop, step);
            let h_len = py_hash_int(len);
            let h_none = pyrust_core::py_hash_none();
            tuple_hash_cpython(
                [
                    Ok(h_len),
                    Ok(if len >= 1 { py_hash_int(start) } else { h_none }),
                    Ok(if len >= 2 { py_hash_int(step) } else { h_none }),
                ]
                .into_iter(),
            )
        }
        // Arbitrary-precision range (#2118): same tuple(len, start, step) hash as
        // the i64 case, computed via the BigInt-aware integer hash helper so the
        // big start/step components reduce correctly.  `len` itself is reduced as
        // a BigInt because it may exceed i64.
        ValueKind::BigRange { start, stop, step } => {
            let len = pyrust_core::bigrange_len(start, stop, step);
            let one = pyrust_core::PyBigInt::from(1);
            let two = pyrust_core::PyBigInt::from(2);
            let h_len = py_hash_bigint(&len);
            let h_none = pyrust_core::py_hash_none();
            tuple_hash_cpython(
                [
                    Ok(h_len),
                    Ok(if len >= one { py_hash_bigint(start) } else { h_none }),
                    Ok(if len >= two { py_hash_bigint(step) } else { h_none }),
                ]
                .into_iter(),
            )
        }
        // BuiltinObject: probe the BuiltinTypeOps hash hook (added in PR #781).
        // Types that override BuiltinTypeOps::hash (e.g. frozenset) return
        // Some(u64); anything that leaves it at the default None is unhashable.
        // Note: slice is intercepted before this match in hash_value_with_interp;
        // reaching this arm for a slice correctly returns None (unhashable) because
        // SliceOps::hash was removed in PR #850.
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
        // Class objects are hashable by identity in CPython (type.__hash__
        // returns id(cls) >> 4, but pointer identity is what matters for
        // correctness).  Use the Rc pointer as the hash value, applying the
        // -1 → -2 sentinel remap matching CPython's tp_hash sentinel rule.
        ValueKind::PyClass(rc) => {
            let ptr = Rc::as_ptr(rc) as i64;
            Ok(if ptr == -1 { -2 } else { ptr })
        }
        // User-defined functions and lambdas are hashable by identity in CPython
        // (function.__hash__ returns id(f) >> 4, but pointer identity is what
        // matters for correctness).  Use the Rc pointer as the hash value.
        ValueKind::UserFunction(rc) => {
            let ptr = Rc::as_ptr(rc) as i64;
            Ok(if ptr == -1 { -2 } else { ptr })
        }
        // Built-in functions (e.g. `print`, `len`) are interned per-name in a
        // thread-local cache, so the name pointer is stable and unique per
        // built-in.  Use the raw pointer of the static name string as the hash.
        ValueKind::BuiltinFunction(name) => {
            let ptr = name.as_ptr() as i64;
            Ok(if ptr == -1 { -2 } else { ptr })
        }
        // Bound methods: CPython hashes as hash(func) ^ hash(self), where func
        // and self are each hashed by pointer identity.  Mirror that here using
        // the Rc pointer of the underlying UserFunction and the Rc pointer of
        // the bound instance.
        ValueKind::BoundMethod { function, receiver } => {
            let func_ptr = Rc::as_ptr(function) as i64;
            let recv_ptr = Rc::as_ptr(receiver) as i64;
            let h = func_ptr ^ recv_ptr;
            Ok(if h == -1 { -2 } else { h })
        }
        // Class-bound methods (classmethods): same XOR pattern, but the second
        // component is the bound class rather than an instance.
        ValueKind::ClassBoundMethod { function, class } => {
            let func_ptr = Rc::as_ptr(function) as i64;
            let class_ptr = Rc::as_ptr(class) as i64;
            let h = func_ptr ^ class_ptr;
            Ok(if h == -1 { -2 } else { h })
        }
        // Complex: CPython Objects/complexobject.c complex_hash.
        //
        //   hash_real = _Py_HashDouble(re)  (as Py_uhash_t)
        //   hash_imag = _Py_HashDouble(im)  (as Py_uhash_t)
        //   combined  = hash_real + _Py_HASH_IMAG * hash_imag  (wrapping u64)
        //   result    = combined as Py_hash_t (i64); if -1 return -2
        //
        // No additional modulo is applied to the sum: CPython uses wrapping
        // unsigned arithmetic matching Py_uhash_t overflow in C.
        ValueKind::Complex(re, im) => {
            const HASH_IMAG: u64 = 1000003;
            let hash_re = py_hash_float(re) as u64;
            let hash_im = py_hash_float(im) as u64;
            let combined = hash_re.wrapping_add(HASH_IMAG.wrapping_mul(hash_im));
            let result = combined as i64;
            Ok(if result == -1 { -2 } else { result })
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
/// `Tuple`: uses the CPython 3.12 xxHash-based tuplehash algorithm (issue #892),
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
        ValueKind::Tuple(inner) => tuple_needs_interp(inner),
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
    // All slices need the slow path: SliceOps::hash is not implemented, so
    // hash_value would always produce a misleading "unhashable type: 'slice'"
    // error regardless of whether the components are actually hashable.
    // hash_value_with_interp handles all three cases correctly: unhashable
    // primitive component (names the component type), PyInstance component
    // (dispatches __hash__), and all-hashable components (computes the hash).
    if let ValueKind::BuiltinObject { ops, .. } = v.kind()
        && ops.type_name() == pyrust_builtins::slice::TYPE_NAME {
            return true;
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
            tuple_hash_cpython(items.iter().map(|item| hash_value_with_interp(interp, item)))
        }
        // Slices: CPython 3.12 makes slice hashable when all components are
        // hashable.  Always recurse into each component via this function so
        // that unhashable components (e.g. a list bound) surface the correct
        // per-component "unhashable type: 'list'" TypeError instead of the
        // misleading "unhashable type: 'slice'" that the pure
        // `SliceOps::hash` (DefaultHasher) path used to produce (issue #850).
        // This also handles `PyInstance` bounds that need interpreter access
        // for `__hash__` dispatch.
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
            // Check for unhashable primitive components while the borrow is live.
            let unhashable = if !needs_interp {
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
            // Clone before dropping the borrow — all accesses to s must happen here.
            let (start, stop, step) = (s.start.clone(), s.stop.clone(), s.step.clone());
            drop(borrow);
            if let Some(bad_type) = unhashable {
                return Err(PyError::named(
                    "TypeError",
                    format!("unhashable type: '{bad_type}'"),
                ));
            }
            let hstart = hash_value_with_interp(interp, &start)?;
            let hstop = hash_value_with_interp(interp, &stop)?;
            let hstep = hash_value_with_interp(interp, &step)?;
            // Hash components using CPython 3.12 slice hash: same xxHash kernel as
            // tuplehash but without the final length-mixing XOR step (issue #892).
            slice_hash_cpython([hstart, hstop, hstep].into_iter().map(Ok))
        }
        ValueKind::PyInstance(inst) => {
            // Issue #1936: a builtin-subclass instance (int/str/float/bytes/
            // tuple/frozenset subclass) with no user `__hash__` override hashes
            // by its backing value (`hash(I(5)) == hash(5)`).  Mirror the
            // `value_to_pykey` path so `hash()` and dict/set keying agree.
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
                    return hash_value_with_interp(interp, &backing);
                }
            }
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
                // Issue #2299: the unhashable built-in types (list/dict/set/
                // bytearray) set `__hash__ = None` on the *type*, so a subclass
                // that does not override `__hash__` inherits unhashability.  The
                // MRO lookup lands on the inherited `object.__hash__` sentinel,
                // OR — when an unhashable builtin and a user `__hash__`-defining
                // base are *both* in the MRO — on the user method if it sits
                // after the builtin (`class C(list, M)`: MRO `[C, list, M, …]`).
                // `class_hash_inherits_builtin_none` walks the MRO and reports
                // whether an unhashable builtin precedes any `__hash__`-defining
                // class, so it covers both shapes regardless of which method the
                // attribute lookup resolved (#2611).  A subclass that re-enables
                // hashing (`__hash__ = object.__hash__` in its own dict) shadows
                // that `None` and the helper returns false.
                if class_hash_inherits_builtin_none(&class) {
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
///
/// Note: `isinstance_check` dispatches `__instancecheck__` for ABC classes
/// before reaching this function, so `cls` here is never an ABC class when
/// called from `isinstance_check`.  `isinstance_single` is also called from
/// other internal sites that do not go through `isinstance_check`.
fn isinstance_single(obj: &Value, cls: &Value) -> bool {
    // Migrated primitives: `type(obj)` returns the per-thread PyClass
    // singleton, so a class-vs-class walk handles every primitive check
    // (including `bool` → `int` via base inheritance).
    if let ValueKind::PyClass(expected) = cls.kind() {
        // Deprecated `typing.List`/`typing.Dict`/… aliases (#2601): delegate
        // the check to the underlying builtin (`list`, `dict`, …) so
        // `isinstance([], typing.List)` behaves like `isinstance([], list)`.
        if let Some(delegate) =
            crate::builtin_modules::typing::legacy_alias_delegate(expected)
        {
            return isinstance_single(obj, &delegate);
        }
        // Fast path: `object` is the universal base — every Python value
        // is an instance of `object`.  Check before the primitive-class
        // dispatch so that `isinstance(None, object)`,
        // `isinstance(print, object)`, etc. all return `True`.
        if Rc::ptr_eq(expected, &crate::interpreter::object_class_singleton()) {
            return true;
        }
        // Fast path: `type` is the metaclass — every class is an instance of
        // `type` in CPython: `isinstance(int, type)` is True,
        // `isinstance(42, type)` is False (issue #1312).
        if Rc::ptr_eq(expected, &type_class_singleton()) {
            return matches!(obj.kind(), ValueKind::PyClass(_));
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
            ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
                Some(method_type_singleton())
            }
            ValueKind::UserFunction(f)
                if !matches!(
                    f.kind,
                    UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
                ) =>
            {
                Some(function_type_singleton())
            }
            // Issue #1626: a class object is an instance of its metatype.
            // When the class has no custom metatype (None), fall back to the
            // `type` singleton so `isinstance(int, type)` etc. still works.
            ValueKind::PyClass(cls_rc) => {
                let meta = cls_rc.borrow().metatype.clone();
                Some(meta.unwrap_or_else(type_class_singleton))
            }
            // `chain.from_iterable(...)` is a native iterator that CPython
            // reports as an `itertools.chain` instance, so `isinstance(it,
            // chain)` must walk the captured `chain` class (#2370).
            ValueKind::Generator(state_rc)
                if state_rc.borrow().downcast_ref::<ChainFromIterableIter>().is_some() =>
            {
                crate::interpreter::itertools_chain_class()
            }
            // Issue #2733: `isinstance(list[int], type(list[int]))` is True —
            // a PEP 585 alias is an instance of the `types.GenericAlias` class.
            ValueKind::BuiltinObject { ops, .. }
                if ops.type_name() == pyrust_builtins::generic_alias::TYPE_NAME =>
            {
                Some(crate::interpreter::generic_alias_class_singleton())
            }
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
    // migrated to PyClass yet.
    match (obj.kind(), cls.kind()) {
        (ValueKind::UserFunction(f), ValueKind::BuiltinFunction("staticmethod")) => {
            f.kind == UserFunctionKind::StaticMethod
        }
        (ValueKind::UserFunction(f), ValueKind::BuiltinFunction("classmethod")) => {
            f.kind == UserFunctionKind::ClassMethod
        }
        (ValueKind::BuiltinObject { ops, .. }, ValueKind::BuiltinFunction(name)) => {
            ops.type_name() == name
        }
        _ => false,
    }
}

/// True if `inst`'s class is a (proper or improper) subclass of the built-in
/// `dict` type.  Used by `dict()` to drive the `keys()` + `__getitem__`
/// mapping-conversion path for dict subclasses (e.g. `collections.Counter`)
/// that keep their backing map in a custom attr rather than
/// `__builtin_data__` (issue #2010).
fn is_dict_subclass_instance(inst: &Rc<RefCell<crate::value::PyInstance>>) -> bool {
    let class = Rc::clone(&inst.borrow().class);
    match crate::interpreter::primitive_class_by_name("dict") {
        Some(dict_class) => class_is_subclass_of(&class, &dict_class),
        None => false,
    }
}

/// `isinstance(obj, classinfo)` — accept a class *or* an
/// arbitrarily-nested tuple of classes or `UnionType`, matching CPython's
/// recursive contract.  Raises `TypeError` if a leaf is neither a class nor a
/// tuple.  See <https://docs.python.org/3/library/functions.html#isinstance>.
fn isinstance_check(
    fn_name: &str,
    obj: &Value,
    cls: &Value,
    interp: &mut crate::Interpreter,
) -> Result<bool> {
    if let ValueKind::Tuple(items) = cls.kind() {
        let items: Vec<Value> = items.to_vec();
        for item in &items {
            if isinstance_check(fn_name, obj, item, interp)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    // PEP 604: `isinstance(x, int | str)` — unwrap UnionType to its __args__.
    if let Some(args) = pyrust_builtins::union_type::union_type_args(cls) {
        if let ValueKind::Tuple(items) = args.kind() {
            let items: Vec<Value> = items.to_vec();
            for item in &items {
                if isinstance_check(fn_name, obj, item, interp)? {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    // `isinstance(x, typing.Union[int, str])` — CPython 3.12 accepts a
    // `typing.Union[...]` alias as the second arg, treating it like the tuple
    // of its `__args__`.  Detect the alias by its origin being the `Union`
    // special form and recurse over its members.
    if let Some(args) = pyrust_builtins::generic_alias::as_typing_union_args(cls) {
        if let ValueKind::Tuple(items) = args.kind() {
            let items: Vec<Value> = items.to_vec();
            for item in &items {
                if isinstance_check(fn_name, obj, item, interp)? {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    // Issue #2525: when `cls` is a plain instance (not a class) whose *type*
    // defines `__instancecheck__`, CPython invokes
    // `type(cls).__instancecheck__(cls, obj)` rather than rejecting it.  The
    // special method is looked up on the type, so resolve it on the instance's
    // class MRO before applying the `is_class_like` guard.  `get_attr` binds the
    // method to the instance receiver, so calling it with `[obj]` yields the
    // `(cls, obj)` argument pairing CPython uses.
    if let ValueKind::PyInstance(inst) = cls.kind() {
        let inst_class = Rc::clone(&inst.borrow().class);
        if crate::interpreter::lookup_class_attr(&inst_class, "__instancecheck__").is_some() {
            let ic_fn = interp.get_attr(cls, "__instancecheck__")?;
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: obj.clone(),
            }];
            let result = interp.call_function_expanded(ic_fn, &call_args)?;
            return interp.truthy_value(&result);
        }
    }
    if !is_class_like(cls) {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() arg 2 must be a type, a tuple of types, or a union"),
        ));
    }
    // Dispatch through __instancecheck__ when cls is a PyClass that defines it
    // (e.g. all ABC classes).  We look up the attr via get_attr so that the
    // descriptor protocol applies (`is_builtin_classmethod` wraps the raw
    // BuiltinFunction as `super_bound_builtin(fn, cls)` on access).  Calling
    // the resulting super_bound_builtin with [obj] causes it to prepend cls,
    // giving abc_instancecheck([cls, obj]).  This also makes
    // `Iterable.__instancecheck__(x)` callable directly.
    if let ValueKind::PyClass(cls_rc) = cls.kind() {
        // Fast path: when `cls` is one of the 11 primitive class singletons
        // (`int`, `str`, …) a direct `ValueKind` tag check settles the result
        // without the `metaclass_dunder` / `__instancecheck__` / Protocol
        // probing below.  Primitives can never carry those hooks nor be a
        // Protocol subclass, so this both preserves the hot `isinstance(x, int)`
        // path and absorbs the cost of the #2526 Protocol check added later.
        if let Some(hit) =
            crate::interpreter::primitive_class_isinstance_fast(obj, cls_rc)
        {
            return Ok(hit);
        }
        // Issue #1955: a metaclass `__instancecheck__` override takes
        // precedence, mirroring CPython's `type(cls).__instancecheck__(cls, x)`
        // dispatch.  `metaclass_dunder` returns `Some` only for a user
        // override, so ordinary classes skip this and keep the fast path.
        if let Some(ic_fn) = crate::interpreter::metaclass_dunder(cls_rc, "__instancecheck__")
            && let ValueKind::UserFunction(f) = ic_fn.kind() {
                let func = Rc::clone(f);
                let call_args = [crate::interpreter::ExpandedCallArg {
                    name: None,
                    value: obj.clone(),
                }];
                let result = interp.call_user_function_expanded(
                    func,
                    &call_args,
                    &[Value::py_class(Rc::clone(cls_rc))],
                )?;
                return interp.truthy_value(&result);
            }
        // Legacy ABC path: ABC classes store `__instancecheck__` directly in
        // their own attrs dict (not on a metaclass).
        let has_ic = cls_rc.borrow().attrs.contains_key("__instancecheck__");
        if has_ic {
            let cls_val = Value::py_class(Rc::clone(cls_rc));
            let ic_fn = interp.get_attr(&cls_val, "__instancecheck__")?;
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: obj.clone(),
            }];
            let result = interp.call_function_expanded(ic_fn, &call_args)?;
            return interp.truthy_value(&result);
        }
        // Issue #2526: structural `isinstance` for `typing.Protocol` subclasses.
        // `@runtime_checkable` records the required member names in
        // `__protocol_attrs__`; the subject is an instance iff it has every one
        // of them (`hasattr` semantics).  A Protocol subclass that was NOT
        // decorated raises, matching CPython 3.12's `_ProtocolMeta`.  The bare
        // `Protocol` class itself is skipped so it keeps ordinary behaviour.
        //
        // Primitive classes (`int`, `str`, …) can never be Protocol subclasses,
        // so a single pointer-keyed dispatch-table lookup short-circuits the
        // recursive `is_protocol_subclass` base-chain walk on the hot
        // `isinstance(x, int)` path (keeps the check perf-neutral, #2526).
        if !crate::interpreter::is_primitive_class(cls_rc)
            && crate::builtin_modules::typing::is_protocol_subclass(cls_rc)
            && !crate::builtin_modules::typing::is_protocol_marker_class(cls_rc)
        {
            return protocol_structural_isinstance(obj, cls_rc);
        }
    }
    Ok(isinstance_single(obj, cls))
}

/// Structural `isinstance(obj, P)` for a `typing.Protocol` subclass `cls_rc`
/// (issue #2526).  Requires `@runtime_checkable` (a `__protocol_attrs__` /
/// `__protocol_runtime_checkable__` pair recorded by the decorator); otherwise
/// raises the CPython 3.12 `TypeError`.  Returns `True` iff `obj` statically has
/// every name in `__protocol_attrs__`.  `isinstance` permits data-member
/// protocols (unlike `issubclass`), so no data-member guard here.
fn protocol_structural_isinstance(obj: &Value, cls_rc: &Rc<RefCell<PyClass>>) -> Result<bool> {
    require_runtime_checkable(cls_rc)?;
    // `isinstance` resolves members on the subject's *type* (issue #2551).  When
    // the subject is itself a class, that type is its metaclass — so a member
    // supplied by the metaclass counts, matching `getattr_static`.
    Ok(protocol_members_present(obj, cls_rc, false))
}

/// Structural `issubclass(cls, P)` for a `typing.Protocol` subclass `cls_rc`
/// (issue #2552).  Like `isinstance`, but the subject is the candidate *class*
/// rather than an instance, so member presence is checked across the candidate's
/// own MRO.  CPython 3.12 forbids `issubclass` against a protocol that declares
/// any non-method (data) member, raising `TypeError` even before the structural
/// walk; `isinstance` is still allowed for such protocols.
fn protocol_structural_issubclass(
    candidate: &Value,
    cls_rc: &Rc<RefCell<PyClass>>,
) -> Result<bool> {
    require_runtime_checkable(cls_rc)?;
    let non_callable = protocol_attr_names(
        crate::interpreter::lookup_class_attr(cls_rc, "__non_callable_proto_members__").as_ref(),
    );
    if !non_callable.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "Protocols with non-method members don't support issubclass()".to_string(),
        ));
    }
    // `issubclass` checks the candidate *class*'s own MRO — the class is its own
    // lookup target, NOT its metaclass.  `isinstance(C, P)` and `issubclass(C, P)`
    // therefore differ when a member lives on the metaclass (CPython 3.12).
    Ok(protocol_members_present(candidate, cls_rc, true))
}

/// Shared guard for both Protocol checks: a Protocol subclass must be
/// `@runtime_checkable` (carry `__protocol_runtime_checkable__ == True`) before it
/// can be used with `isinstance`/`issubclass`, matching CPython 3.12's
/// `_ProtocolMeta`.
fn require_runtime_checkable(cls_rc: &Rc<RefCell<PyClass>>) -> Result<()> {
    let runtime_checkable =
        crate::interpreter::lookup_class_attr(cls_rc, "__protocol_runtime_checkable__")
            .is_some_and(|v| matches!(v.kind(), ValueKind::Bool(true)));
    if !runtime_checkable {
        return Err(PyError::named(
            "TypeError",
            "Instance and class checks can only be used with @runtime_checkable protocols"
                .to_string(),
        ));
    }
    Ok(())
}

/// Shared member-presence walk for `isinstance`/`issubclass` against a
/// `@runtime_checkable` Protocol.  `subject` is the instance (isinstance) or the
/// candidate class (issubclass).  Every name in `__protocol_attrs__` must resolve
/// via static attribute lookup (issue #2551 — bypassing `__getattr__` and
/// descriptors).  Missing/empty `__protocol_attrs__` matches everything, mirroring
/// CPython for an attribute-free protocol body.
///
/// `subject_is_class` selects the lookup target: for `issubclass` the subject is
/// the candidate class itself (walk its own MRO), while for `isinstance` the
/// subject is resolved to its type — its metaclass when it happens to be a class.
fn protocol_members_present(
    subject: &Value,
    cls_rc: &Rc<RefCell<PyClass>>,
    subject_is_class: bool,
) -> bool {
    let attrs = crate::interpreter::lookup_class_attr(cls_rc, "__protocol_attrs__");
    let names: Vec<String> = protocol_attr_names(attrs.as_ref());
    // CPython 3.12 treats a member that resolves to `None` as absent unless the
    // member is a declared non-callable (data) member.  `runtime_checkable`
    // records the non-callable subset in `__non_callable_proto_members__`.
    let non_callable = protocol_attr_names(
        crate::interpreter::lookup_class_attr(cls_rc, "__non_callable_proto_members__").as_ref(),
    );
    for name in &names {
        // Issue #2551: CPython's `_ProtocolMeta` resolves each member with
        // `inspect.getattr_static` semantics — it scans the instance `__dict__`
        // and the type's MRO dicts directly, never invoking `__getattr__` or
        // descriptor `__get__`.  A dynamic `get_attr` probe both over-matches
        // (`__getattr__`-supplied attrs count as present) and lets a raising
        // `__getattr__` abort the check.  `has_static_attr` never raises.
        match has_static_attr(subject, name, subject_is_class) {
            None => return false,
            Some(val) => {
                if matches!(val.kind(), ValueKind::None) && !non_callable.iter().any(|n| n == name)
                {
                    // A callable (method) member resolved to `None` → absent.
                    return false;
                }
            }
        }
    }
    true
}

/// Resolve attribute `name` on `value` the way CPython's `inspect.getattr_static`
/// does: consult the instance's own `__dict__` first (for a `PyInstance`), then
/// each class in the MRO's own attribute dict directly, without invoking
/// `__getattr__` or descriptor `__get__`.  Returns the raw stored `Value` if
/// found, else `None`.  Never raises — a missing attribute, or a `__getattr__`
/// that would raise, is simply "absent" (issues #2551 / #2552).
///
/// `value_is_class` selects the lookup target when `value` is a class:
/// `issubclass(C, P)` (`true`) walks `C`'s own MRO, treating `C` as the lookup
/// target; `isinstance(C, P)` (`false`) resolves `C`'s type — its metaclass — so
/// a protocol member supplied by the metaclass counts, matching CPython's
/// `getattr_static(C, name)` which searches the metaclass MRO.
fn has_static_attr(value: &Value, name: &str, value_is_class: bool) -> Option<Value> {
    // Instance `__dict__` shadows the class, matching attribute resolution order.
    if let ValueKind::PyInstance(inst) = value.kind()
        && let Some(v) = inst.borrow().attrs.get(name)
    {
        return Some(v.clone());
    }
    // Class-side static walk via `lookup_class_attr`, which reads each class's own
    // `attrs` dict directly along the C3 MRO — no `__getattr__`, no descriptor
    // binding.
    if let ValueKind::PyClass(cls_rc) = value.kind() {
        // The subject is a class.  `issubclass(C, P)` checks `C`'s own MRO only.
        if value_is_class {
            return crate::interpreter::lookup_class_attr(cls_rc, name);
        }
        // `isinstance(C, P)` mirrors `getattr_static(C, name)`, which searches both
        // `C`'s own MRO and `C`'s metaclass MRO (a classmethod on `C` and a method
        // on the metaclass both satisfy the protocol).
        if let Some(v) = crate::interpreter::lookup_class_attr(cls_rc, name) {
            return Some(v);
        }
        if let ValueKind::PyClass(meta_rc) = value_class(value).kind() {
            return crate::interpreter::lookup_class_attr(meta_rc, name);
        }
        return None;
    }
    // Non-class subject: resolve its type — the instance's class for a
    // `PyInstance`, or the primitive-type singleton for `list`/`int`/… so e.g.
    // `isinstance([], Sized)` still sees `list.__len__`.
    if let ValueKind::PyClass(cls_rc) = value_class(value).kind() {
        return crate::interpreter::lookup_class_attr(cls_rc, name);
    }
    None
}

/// Extract the string names from a Protocol `set`-valued bookkeeping attribute
/// (`__protocol_attrs__` / `__non_callable_proto_members__`).  A missing or
/// non-`set` value yields an empty list.
fn protocol_attr_names(attr: Option<&Value>) -> Vec<String> {
    match attr.map(|v| v.kind()) {
        Some(ValueKind::Set(items)) => items
            .iter()
            .filter_map(|k| match k {
                pyrust_core::PyKey::Str(v) => v.as_str().map(|s| s.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `issubclass(cls, classinfo)` — same tuple-recursive contract as
/// `isinstance_check`, but compares classes rather than instances.
/// Dispatches through `__subclasscheck__` for PyClass leaves (e.g. ABC
/// classes), mirroring CPython's `type.__subclasscheck__` dispatch.
fn issubclass_check(
    fn_name: &str,
    cls: &Value,
    classinfo: &Value,
    interp: &mut crate::Interpreter,
) -> Result<bool> {
    if let ValueKind::Tuple(items) = classinfo.kind() {
        let items: Vec<Value> = items.to_vec();
        for item in &items {
            if issubclass_check(fn_name, cls, item, interp)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    // Deprecated `typing.List`/`typing.Dict`/… aliases (#2601): delegate the
    // check to the underlying builtin so `issubclass(list, typing.List)`
    // behaves like `issubclass(list, list)`.
    if let ValueKind::PyClass(classinfo_rc) = classinfo.kind()
        && let Some(delegate) =
            crate::builtin_modules::typing::legacy_alias_delegate(classinfo_rc)
    {
        return issubclass_check(fn_name, cls, &delegate, interp);
    }
    // PEP 604: `issubclass(X, int | str)` — unwrap UnionType to its __args__.
    if let Some(args) = pyrust_builtins::union_type::union_type_args(classinfo) {
        if let ValueKind::Tuple(items) = args.kind() {
            let items: Vec<Value> = items.to_vec();
            for item in &items {
                if issubclass_check(fn_name, cls, item, interp)? {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    // `issubclass(X, typing.Union[int, str])` — accept a `typing.Union[...]`
    // alias as the second arg, treating it like the tuple of its `__args__`
    // (CPython 3.12).
    if let Some(args) = pyrust_builtins::generic_alias::as_typing_union_args(classinfo) {
        if let ValueKind::Tuple(items) = args.kind() {
            let items: Vec<Value> = items.to_vec();
            for item in &items {
                if issubclass_check(fn_name, cls, item, interp)? {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    // Dispatch through __subclasscheck__ when classinfo is a PyClass that
    // defines it (e.g. all ABC classes).  This handles structural subtyping
    // for `issubclass(UserClass, Iterable)` and tuple forms like
    // `issubclass(UserClass, (Iterable, Hashable))` (fixes #1799).
    if let ValueKind::PyClass(classinfo_rc) = classinfo.kind() {
        // Issue #1955: a metaclass `__subclasscheck__` override takes
        // precedence, mirroring CPython's
        // `type(classinfo).__subclasscheck__(classinfo, cls)` dispatch.
        if let Some(sc_fn) =
            crate::interpreter::metaclass_dunder(classinfo_rc, "__subclasscheck__")
            && let ValueKind::UserFunction(f) = sc_fn.kind() {
                let func = Rc::clone(f);
                let call_args = [crate::interpreter::ExpandedCallArg {
                    name: None,
                    value: cls.clone(),
                }];
                let result = interp.call_user_function_expanded(
                    func,
                    &call_args,
                    &[Value::py_class(Rc::clone(classinfo_rc))],
                )?;
                return interp.truthy_value(&result);
            }
        // Legacy ABC path: ABC classes store `__subclasscheck__` directly in
        // their own attrs dict (not on a metaclass).
        let has_sc = classinfo_rc.borrow().attrs.contains_key("__subclasscheck__");
        if has_sc {
            let classinfo_val = Value::py_class(Rc::clone(classinfo_rc));
            let sc_fn = interp.get_attr(&classinfo_val, "__subclasscheck__")?;
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: cls.clone(),
            }];
            let result = interp.call_function_expanded(sc_fn, &call_args)?;
            return interp.truthy_value(&result);
        }
    }
    // Issue #2525: when `classinfo` is a plain instance (not a class) whose
    // *type* defines `__subclasscheck__`, CPython invokes
    // `type(classinfo).__subclasscheck__(classinfo, cls)` rather than raising
    // `TypeError`.  Resolve the hook on the instance's class MRO before the
    // match's `arg 2 must be a class` fallback.  `get_attr` binds the method to
    // the instance receiver, so calling it with `[cls]` yields the
    // `(classinfo, cls)` pairing CPython uses.
    if let ValueKind::PyInstance(inst) = classinfo.kind() {
        let inst_class = Rc::clone(&inst.borrow().class);
        if crate::interpreter::lookup_class_attr(&inst_class, "__subclasscheck__").is_some() {
            let sc_fn = interp.get_attr(classinfo, "__subclasscheck__")?;
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: cls.clone(),
            }];
            let result = interp.call_function_expanded(sc_fn, &call_args)?;
            return interp.truthy_value(&result);
        }
    }
    // `cls` may be either a user-defined class (`PyClass`) or a built-in type
    // token (`BuiltinFunction("int")` etc.); anything else is a `TypeError`,
    // matching CPython.  This runs *after* the `__subclasscheck__` dispatch
    // above so a custom hook on `type(classinfo)` can accept a non-class
    // `cls` (issue #2525); it is reached per tuple/union leaf, matching
    // CPython's lazy per-leaf validation.
    if !is_class_like(cls) {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() arg 1 must be a class"),
        ));
    }
    // Issue #2552: structural `issubclass` for `typing.Protocol` subclasses.
    // Mirrors the `isinstance` short-circuit but checks the candidate *class*'s
    // MRO rather than an instance.  Reached only after the `arg 1 must be a
    // class` guard above, matching CPython's error precedence (a non-class
    // `cls` raises before the protocol's data-member `TypeError`).  Primitive
    // classes can never be Protocol subclasses, so the dispatch-table guard
    // keeps the hot `issubclass(x, int)` path off the base-chain walk.
    if let ValueKind::PyClass(classinfo_rc) = classinfo.kind()
        && !crate::interpreter::is_primitive_class(classinfo_rc)
        && crate::builtin_modules::typing::is_protocol_subclass(classinfo_rc)
        && !crate::builtin_modules::typing::is_protocol_marker_class(classinfo_rc)
    {
        return protocol_structural_issubclass(cls, classinfo_rc);
    }
    match (cls.kind(), classinfo.kind()) {
        // User-defined → user-defined: walk the `base` chain.
        (ValueKind::PyClass(c), ValueKind::PyClass(expected)) => {
            Ok(class_is_subclass_of(c, expected))
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

/// Format a `PyBigInt` as Python's `hex()`/`oct()`/`bin()` output.
/// `prefix` is `"0x"`, `"0o"`, or `"0b"`; `radix` is 16, 8, or 2.
/// Negative values get a `-` prepended: `"-0x2a"` etc.
fn format_bigint_radix(b: &crate::value::PyBigInt, radix: u32, prefix: &str) -> String {
    use num_bigint::Sign;
    let (sign, digits) = (b.sign(), b.magnitude().to_str_radix(radix));
    if sign == Sign::Minus {
        format!("-{prefix}{digits}")
    } else {
        format!("{prefix}{digits}")
    }
}

/// The canonical `TypeError` for a value that is not an integer and has no
/// `__index__` — `"'X' object cannot be interpreted as an integer"`.  Passed as
/// the `not_index_err` closure to `Interpreter::value_to_index` from the
/// `bin`/`oct`/`hex`/`chr` catch-alls (#2022).
fn not_an_integer_err(v: &Value) -> PyError {
    PyError::named(
        "TypeError",
        format!(
            "'{}' object cannot be interpreted as an integer",
            value_type_name_str(v),
        ),
    )
}

/// Format an already-resolved index `Value` (guaranteed `Int`/`Bool`/`BigInt`
/// by `value_to_index`) as a radix string for `bin`/`oct`/`hex`.  Small ints go
/// through `small_fmt` (`format_bin_i64` etc.); `BigInt` uses
/// `format_bigint_radix`.
fn format_index_radix(v: &Value, radix: u32, prefix: &str, small_fmt: fn(i64) -> String) -> String {
    match v.kind() {
        ValueKind::Bool(b) => small_fmt(if b { 1 } else { 0 }),
        ValueKind::Int(n) => small_fmt(n),
        ValueKind::BigInt(b) => format_bigint_radix(b, radix, prefix),
        _ => unreachable!("format_index_radix: value_to_index guarantees an integer"),
    }
}

/// Validate a codepoint and return the corresponding single-char `str`
/// `Value`.  Shared by the `PyInt` and `PyBool` overloads of the typed
/// `chr` builtin (#400).  Out-of-range codepoints raise `ValueError` with
/// the same wording CPython 3.12 uses (`"chr() arg not in range(0x110000)"`).
///
/// CPython's `chr()` converts its argument to a C `int` (int32_t) before the
/// Unicode range check.  Values outside `[i32::MIN, i32::MAX]` therefore raise
/// `OverflowError("Python int too large to convert to C int")`, even if they
/// fit in an i64.  Values inside the C-int range but outside `0..0x110000`
/// raise `ValueError`.  (#1584)
///
/// CPython accepts any value in `range(0x110000)`, including the surrogate
/// range (0xD800–0xDFFF).  Rust's `char` rejects surrogates (they are not
/// Unicode scalar values), so we write the CESU-8 three-byte sequence
/// directly for that range, matching the representation used throughout the
/// runtime for surrogate-containing strings (#1573).
fn chr_from_code_point(code_point: i64) -> Result<Value> {
    // CPython converts to C int (int32_t) first.  Anything outside that range
    // raises OverflowError regardless of the Unicode range check.
    if !(i64::from(i32::MIN)..=i64::from(i32::MAX)).contains(&code_point) {
        return Err(PyError::named(
            "OverflowError",
            "Python int too large to convert to C int".to_string(),
        ));
    }
    if !(0..=1114111).contains(&code_point) {
        return Err(PyError::named(
            "ValueError",
            "chr() arg not in range(0x110000)".to_string(),
        ));
    }
    // Lone surrogates (0xD800–0xDFFF) are stored as CESU-8; non-surrogates go
    // through `char`.  Both cases are handled by the shared encoder, which is
    // the inverse of `cesu8_codepoints`.
    Ok(Value::string(pyrust_core::cesu8_encode_codepoint(code_point as u32)))
}

/// Encode a Python `str` to `bytes` for `bytes(source, encoding[, errors])`.
/// Delegates to `pyrust_builtins::string::encode_str_to_bytes`.
fn encode_str_to_bytes(source: &str, encoding: &str, errors: &str) -> Result<Value> {
    pyrust_builtins::string::encode_str_to_bytes(source, encoding, errors)
}

/// If `v` is a bytes-like object (`bytes` or `bytearray`), return its byte
/// contents plus its Python `repr()`, for the `float()` bytes-like parse path
/// (#2077).  `float()`'s `could not convert string to float` message uses the
/// operand's own repr — `b'…'` for bytes, `bytearray(b'…')` for bytearray.
/// (Unlike `int()`, which always renders the bytes repr; see that path.)
fn float_bytes_like(v: &Value) -> Option<(Vec<u8>, String)> {
    match v.kind() {
        ValueKind::Bytes(rc) => Some((rc.as_slice().to_vec(), v.repr_raw())),
        _ => pyrust_builtins::bytearray::as_bytearray_snapshot(v).map(|data| (data, v.repr_raw())),
    }
}

/// Parse a bytes-like buffer as an `int` for `int(bytes_like[, base])`,
/// decoding the buffer as ASCII and reusing the exact same numeric parse as
/// the `str` operand (whitespace trim, PEP 515 underscores, base handling).
/// `repr` is the operand's `repr()` for the `invalid literal` error message.
fn int_parse_bytes_like(bytes: &[u8], repr: &str, base_arg: i64) -> Result<Value> {
    use num_traits::Num as _;
    let err = || {
        PyError::named(
            "ValueError",
            format!("invalid literal for int() with base {base_arg}: {repr}"),
        )
    };
    let s = std::str::from_utf8(bytes).map_err(|_| err())?;
    let trimmed = s.trim();
    if base_arg == 0 {
        let (base, digits) = int_parse_base_zero(trimmed).ok_or_else(err)?;
        pyrust_core::check_int_parse_digits(&digits, base)?;
        match i64::from_str_radix(&digits, base) {
            Ok(v) => Ok(Value::int(v)),
            Err(_) => crate::value::PyBigInt::from_str_radix(&digits, base)
                .map(Value::bigint)
                .map_err(|_| err()),
        }
    } else {
        let base = base_arg as u32;
        let stripped = int_strip_explicit_base(trimmed, base).ok_or_else(err)?;
        pyrust_core::check_int_parse_digits(&stripped, base)?;
        match i64::from_str_radix(&stripped, base) {
            Ok(v) => Ok(Value::int(v)),
            Err(_) => crate::value::PyBigInt::from_str_radix(&stripped, base)
                .map(Value::bigint)
                .map_err(|_| err()),
        }
    }
}

/// Parse a bytes-like buffer as a `float` for `float(bytes_like)`, decoding as
/// ASCII and reusing the same parse as the `str` operand (PEP 515 underscores,
/// surrounding whitespace, `inf`/`nan`).  `repr` is used in the error message.
fn float_parse_bytes_like(bytes: &[u8], repr: &str) -> Result<Value> {
    let err = || {
        PyError::named(
            "ValueError",
            format!("could not convert string to float: {repr}"),
        )
    };
    let s = std::str::from_utf8(bytes).map_err(|_| err())?;
    let cleaned = pep515_strip_float(s.trim()).ok_or_else(err)?;
    cleaned.parse::<f64>().map(Value::float).map_err(|_| err())
}

/// Bind `bytes()` / `bytearray()` call args into the equivalent positional
/// slice the bodies' `match args.len()` logic expects, accepting the CPython
/// 3.12 keyword names `source` / `encoding` / `errors`.
///
/// The encode form is selected by the presence of `encoding`; when only
/// `errors` is supplied, CPython raises a dedicated message rather than
/// treating it as an encode — replicated here for parity.
fn bind_bytes_like_args(
    function_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Vec<ExpandedCallArg>> {
    let slots = bind_constructor_kwargs(
        function_name,
        args,
        &["source", "encoding", "errors"],
        &[true, true, true],
        3,
    )?;
    let source = &slots[0];
    let encoding = &slots[1];
    let errors = &slots[2];

    let make = |v: Value| ExpandedCallArg { name: None, value: v };

    if encoding.is_some() {
        // Encode form: source defaults to a non-str placeholder so the
        // encode path reports "encoding without a string argument" when the
        // source is omitted, matching CPython.
        let mut out = Vec::with_capacity(3);
        out.push(make(source.clone().unwrap_or_else(Value::none)));
        out.push(make(encoding.clone().unwrap()));
        if let Some(e) = errors.clone() {
            out.push(make(e));
        }
        Ok(out)
    } else if errors.is_some() {
        // `errors` given without `encoding`: CPython reports a string-specific
        // message when source is a str, else the generic errors message.
        if matches!(source.as_ref().map(|v| v.kind()), Some(ValueKind::Str(_))) {
            Err(PyError::named(
                "TypeError",
                "string argument without an encoding".to_string(),
            ))
        } else {
            Err(PyError::named(
                "TypeError",
                "errors without a string argument".to_string(),
            ))
        }
    } else {
        // Buffer-protocol form: 0-arg (no source) or 1-arg.
        match source.clone() {
            Some(v) => Ok(vec![make(v)]),
            None => Ok(Vec::new()),
        }
    }
}

/// Warm-path element conversion for `bytes()` / `bytearray()` from a `List` /
/// `Tuple` slice without allocating or dispatching: each `int` in `0..=255` (or
/// `bool`) is converted in place.  Returns:
/// - `Ok(Ok(out))` — every element was a plain int/bool (the common case);
/// - `Ok(Err((out, i)))` — element `i` is a `PyInstance` that may carry
///   `__index__`; the caller resolves `items[i..]` via `bytes_element_to_u8`;
/// - `Err(_)` — an out-of-range int (`ValueError`) or a non-int non-instance
///   (`TypeError`), raised immediately (CPython stops at the first bad element).
#[allow(clippy::type_complexity)]
fn try_fast_bytes_elems(items: &[Value]) -> Result<std::result::Result<Vec<u8>, (Vec<u8>, usize)>> {
    let mut out = Vec::with_capacity(items.len());
    for (i, v) in items.iter().enumerate() {
        match v.kind() {
            ValueKind::Int(n) if (0..=255).contains(&n) => out.push(n as u8),
            ValueKind::Bool(b) => out.push(b as u8),
            ValueKind::Int(_) | ValueKind::BigInt(_) => {
                return Err(PyError::named(
                    "ValueError",
                    "bytes must be in range(0, 256)".to_string(),
                ))
            }
            ValueKind::PyInstance(_) => return Ok(Err((out, i))),
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
    Ok(Ok(out))
}

/// Convert an owned `Vec<Value>` of `bytes()` / `bytearray()` elements to a
/// `Vec<u8>`, taking the allocation-free fast path when every element is a plain
/// int/bool and only dispatching `__index__` for the rare `PyInstance` element.
fn bytes_from_items(interp: &mut crate::Interpreter, items: Vec<Value>) -> Result<Vec<u8>> {
    match try_fast_bytes_elems(&items)? {
        Ok(out) => Ok(out),
        Err((mut out, from)) => {
            for v in &items[from..] {
                out.push(bytes_element_to_u8(interp, v)?);
            }
            Ok(out)
        }
    }
}

/// Resolve `int.from_bytes(source, ...)`'s `source` argument to a `Vec<u8>`.
///
/// CPython's `int.from_bytes` accepts any bytes-like object (bytes, bytearray,
/// memoryview — the buffer protocol) or any iterable of ints in `0..=255`, via
/// the same `PyBytes_FromObject` machinery the `bytes()` constructor uses.  It
/// differs from `bytes()` in that a bare `int` is *not* a length count and a
/// `str` is rejected: both raise `TypeError: cannot convert 'X' object to
/// bytes` (an int/str is not, for this purpose, a valid byte source).
pub(crate) fn from_bytes_source_to_bytes(
    interp: &mut crate::Interpreter,
    source: &Value,
) -> Result<Vec<u8>> {
    // bytes-like buffer objects (bytearray, and any future memoryview).
    if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(source) {
        return Ok(data);
    }
    match source.kind() {
        ValueKind::Bytes(rc) => Ok((**rc).clone()),
        // `str` and bare numbers are iterable-shaped (or index-shaped) but
        // rejected by CPython with the buffer-protocol message rather than the
        // per-element message.
        ValueKind::Str(_) | ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
            Err(PyError::named(
                "TypeError",
                format!(
                    "cannot convert '{}' object to bytes",
                    pyrust_core::builtin_type_name(source),
                ),
            ))
        }
        // General iterable of ints (list, tuple, range, generator, user iters).
        _ => {
            let type_name = pyrust_core::builtin_type_name(source).into_owned();
            let items = interp.collect_iterable(source).map_err(|e| {
                if e.class_name_is("TypeError") {
                    PyError::named(
                        "TypeError",
                        format!("cannot convert '{type_name}' object to bytes"),
                    )
                } else {
                    e
                }
            })?;
            bytes_from_items(interp, items)
        }
    }
}

/// Convert a single element of a `bytes()` / `bytearray()` source iterable to a
/// `u8`, honoring CPython 3.12's `__index__` protocol.  Plain `int` / `bool`
/// short-circuit (the warm path); only a `PyInstance` triggers a `__index__`
/// dispatch.  An int outside `0..=255` (after `__index__`) raises
/// `ValueError: bytes must be in range(0, 256)`; a non-integer without
/// `__index__` raises `TypeError: 'X' object cannot be interpreted as an
/// integer`; `__index__` returning a non-int raises
/// `TypeError: __index__ returned non-int (type X)`.
fn bytes_element_to_u8(interp: &mut crate::Interpreter, v: &Value) -> Result<u8> {
    match v.kind() {
        ValueKind::Int(n) if (0..=255).contains(&n) => Ok(n as u8),
        ValueKind::Bool(b) => Ok(b as u8),
        ValueKind::Int(_) | ValueKind::BigInt(_) => Err(PyError::named(
            "ValueError",
            "bytes must be in range(0, 256)".to_string(),
        )),
        ValueKind::PyInstance(inst) => {
            let inst_rc = Rc::clone(inst);
            let class = Rc::clone(&inst_rc.borrow().class);
            let Some(method) = lookup_class_attr(&class, "__index__") else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        pyrust_core::builtin_type_name(v),
                    ),
                ));
            };
            let self_val = Value::py_instance(inst_rc);
            let result = invoke_class_method(interp, method, self_val, &[])?;
            bytes_index_result_to_u8(&result)
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(v),
            ),
        )),
    }
}

/// Range-check the result of an element `__index__` call for `bytes()` /
/// `bytearray()`.  `bool` / `int` in `0..=255` succeed; out-of-range ints raise
/// `ValueError`; anything else raises the `__index__ returned non-int` TypeError.
fn bytes_index_result_to_u8(result: &Value) -> Result<u8> {
    match result.kind() {
        ValueKind::Bool(b) => Ok(b as u8),
        ValueKind::Int(n) if (0..=255).contains(&n) => Ok(n as u8),
        ValueKind::Int(_) | ValueKind::BigInt(_) => Err(PyError::named(
            "ValueError",
            "bytes must be in range(0, 256)".to_string(),
        )),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "__index__ returned non-int (type {})",
                value_type_name_str(result),
            ),
        )),
    }
}

/// Resolve the `bytes(n)` / `bytearray(n)` count argument through `__index__`
/// when the argument is a `PyInstance`.  Returns `Some(count)` (the non-negative
/// byte count) on success, `None` when the instance has no `__index__` (so the
/// caller should fall through to the iterable path).  A negative count or a
/// `__index__` returning a non-int / out-of-range value raises directly.
fn bytes_count_via_index(interp: &mut crate::Interpreter, val: &Value) -> Result<Option<usize>> {
    let ValueKind::PyInstance(inst) = val.kind() else {
        return Ok(None);
    };
    let inst_rc = Rc::clone(inst);
    let class = Rc::clone(&inst_rc.borrow().class);
    let Some(method) = lookup_class_attr(&class, "__index__") else {
        return Ok(None);
    };
    let self_val = Value::py_instance(inst_rc);
    let result = invoke_class_method(interp, method, self_val, &[])?;
    let count = match result.kind() {
        ValueKind::Bool(b) => b as i64,
        ValueKind::Int(n) => n,
        ValueKind::BigInt(_) => {
            // CPython names the *original* object here, not the int the
            // __index__ returned: `bytes(obj)` -> "cannot fit 'obj-type' into
            // an index-sized integer" (#1908).
            return Err(PyError::named(
                "OverflowError",
                format!(
                    "cannot fit '{}' into an index-sized integer",
                    value_type_name_str(val),
                ),
            ));
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "__index__ returned non-int (type {})",
                    value_type_name_str(&result),
                ),
            ))
        }
    };
    if count < 0 {
        return Err(PyError::named("ValueError", "negative count".to_string()));
    }
    Ok(Some(count as usize))
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

/// Validate and extract an attribute-name argument for the
/// `getattr` / `setattr` / `hasattr` / `delattr` builtins (#2350).
///
/// CPython requires the name be a `str` (any `str` subclass is also
/// accepted via the `isinstance` relationship) and otherwise raises
/// `TypeError: attribute name must be string, not '<type>'` — note the
/// wording has no function-name prefix, says "be string" (no article),
/// and names the offending type.  This is the single shared validator
/// so all four builtins emit byte-identical messages.
fn attr_name_arg(name: &Value) -> Result<String> {
    if is_str_or_str_subclass(name) {
        Ok(extract_str_value(name))
    } else {
        let type_name = value_type_name_str(name);
        Err(PyError::named(
            "TypeError",
            format!("attribute name must be string, not '{type_name}'"),
        ))
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
    // Collect positional args first: CPython emits the "at least 1 argument"
    // error before any kwarg validation when no positionals are present.
    let positional: Vec<&ExpandedCallArg> =
        args.iter().filter(|a| a.name.is_none()).collect();
    if positional.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name} expected at least 1 argument, got 0"),
        ));
    }
    let key_fn = args.iter().find(|a| a.name.as_deref() == Some("key"))
        .map(|a| a.value.clone())
        .filter(|v| !v.is_none());
    let default_val = args.iter().find(|a| a.name.as_deref() == Some("default"))
        .map(|a| a.value.clone());
    for a in args.iter().filter(|a| a.name.is_some()) {
        if a.name.as_deref() != Some("key") && a.name.as_deref() != Some("default") {
            return Err(PyError::named(
                "TypeError",
                format!("'{}' is an invalid keyword argument for {fn_name}()", a.name.as_ref().unwrap()),
            ));
        }
    }
    let items: Vec<Value> = if positional.len() == 1 {
        interp.collect_iterable(&positional[0].value)?
    } else {
        // positional.len() >= 2
        if default_val.is_some() {
            return Err(PyError::named(
                "TypeError",
                format!("Cannot specify a default for {fn_name}() with multiple positional arguments"),
            ));
        }
        positional.iter().map(|a| a.value.clone()).collect()
    };
    if items.is_empty() {
        if let Some(default) = default_val {
            return Ok(default);
        }
        return Err(PyError::named(
            "ValueError",
            format!("{fn_name}() iterable argument is empty"),
        ));
    }
    if let Some(kfn) = key_fn {
        let mut keyed: Vec<(Value, Value)> = Vec::with_capacity(items.len());
        for v in items {
            let k = interp.call_function_expanded(
                kfn.clone(),
                &[ExpandedCallArg { name: None, value: v.clone() }],
            )?;
            keyed.push((k, v));
        }
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

/// Returns `true` when a container element (a `Value`) requires interpreter
/// access during repr — i.e., when `Value::repr_raw()` alone is insufficient.
///
/// The only cases that need interpreter dispatch are:
/// - `PyInstance` — may have a user-defined `__repr__`
/// - Container types (`List`, `Tuple`, `Dict`, `Set`) — may *contain* an
///   instance at any nesting depth
/// - `BuiltinObject` — may be a frozenset containing `PyKey::Object`, or
///   another builtin type with user-backing
///
/// Plain scalars (`Int`, `Str`, `Float`, `Bool`, `None`, `BigInt`, `Bytes`,
/// `Complex`, `Ellipsis`, `NotImplemented`) always return `false`.
#[inline]
fn value_needs_interp_repr(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::PyInstance(_)
            | ValueKind::BuiltinObject { .. }
            | ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_)
    )
}

/// Returns `true` when a container key (`PyKey`) requires interpreter access
/// during repr.  Only `PyKey::Object` (user instance), `PyKey::Tuple` (may
/// contain an Object), and `PyKey::FrozenSet` (may contain an Object) need
/// the slow path; all primitive key variants are handled by `key_repr`.
#[inline]
fn key_needs_interp_repr(k: &PyKey) -> bool {
    matches!(k, PyKey::Object { .. } | PyKey::Tuple(_) | PyKey::FrozenSet(_))
}

/// Render `value` to its Python repr string, honouring `__repr__` on user
/// instances and recursing into containers (list/tuple/dict/set) with the same
/// interpreter-aware dispatch on each element.
///
/// Shared by the `repr()` builtin and `render_instance_str` (for the container
/// case, where `str(list)` is defined as `repr(list)` in CPython).
///
/// Cycle detection mirrors `Value::repr_raw()`: a per-call-stack thread-local
/// tracks which container object ids are currently being formatted; a second
/// visit short-circuits to the CPython placeholder (`[...]` / `(...)` /
/// `{...}`).
pub(crate) fn render_value_repr(interp: &mut crate::Interpreter, value: &Value) -> Result<String> {
    // Dispatch __repr__ for user instances.
    if let ValueKind::PyInstance(instance) = value.kind() {
        let instance_rc = Rc::clone(instance);
        let class = Rc::clone(&instance_rc.borrow().class);
        if let Some(method_val) = lookup_class_attr(&class, "__repr__") {
            // Issue #1537: primitive types now have `object` as an explicit
            // MRO base, so `object.__repr__` is reachable for user subclasses
            // (e.g. `class MyList(list): pass`).  Skip the `object.__repr__`
            // sentinel when backing data is present — the backing-data path
            // below renders the contents correctly, matching CPython's
            // `list.__repr__`, `dict.__repr__`, etc. behaviour.
            let is_object_repr =
                matches!(method_val.kind(), ValueKind::BuiltinFunction("object.__repr__"));
            // Builtin BaseException.__repr__ sentinel: render arg reprs with
            // interpreter dispatch when any arg is a PyInstance — core's
            // data-only exception_repr cannot honour a user __repr__
            // override on an arg (issue #2390 review).
            if matches!(method_val.kind(), ValueKind::BuiltinFunction(_))
                && pyrust_core::is_exception_instance(&instance_rc)
                && let Some(rendered) = crate::interpreter::exception_repr_with_dispatch(
                    interp,
                    &instance_rc,
                )?
            {
                return Ok(rendered);
            }
            if !is_object_repr || instance_builtin_data(&instance_rc).is_none() {
                let result = invoke_class_method(
                    interp,
                    method_val,
                    Value::py_instance(instance_rc),
                    &[],
                )?;
                return if matches!(result.kind(), ValueKind::Str(_)) {
                    Ok(result.as_str().unwrap_or("").to_string())
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
        // Issue #1204: no __repr__ defined (or object.__repr__ skipped) —
        // if the instance has a scalar
        // primitive backing (str/int/float/bytes subclass), delegate repr()
        // to the backing value so that repr(MyInt(42)) gives "42" rather
        // than the default object repr.  (Counter/defaultdict/deque define
        // their own __repr__ as BuiltinFunctions; the lookup above handles
        // those; this path only fires when lookup returned None.)
        // Issue #1205: extend to container backings (list/dict/tuple/set
        // subclasses).  list/dict/tuple render the same as the backing
        // container.  set/frozenset subclasses prefix the class name:
        // `MySet({1, 2})` / `MySet()`, matching CPython's set_repr().
        if let Some(backing) = instance_builtin_data(&instance_rc) {
            match backing.kind() {
                ValueKind::Str(_)
                | ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Bool(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _)
                | ValueKind::Bytes(_) => return Ok(backing.repr_raw()),
                ValueKind::List(_) | ValueKind::Dict(_) | ValueKind::Tuple(_) => {
                    return render_value_repr(interp, &backing);
                }
                ValueKind::Set(items) => {
                    let class_name = class.borrow().name.clone();
                    if items.is_empty() {
                        return Ok(format!("{class_name}()"));
                    }
                    let inner = render_value_repr(interp, &backing)?;
                    return Ok(format!("{class_name}({inner})"));
                }
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::frozenset::TYPE_NAME =>
                {
                    let class_name = class.borrow().name.clone();
                    let items = pyrust_builtins::frozenset::as_items(&backing);
                    let is_empty = items.as_ref().is_none_or(|rc| rc.is_empty());
                    if is_empty {
                        return Ok(format!("{class_name}()"));
                    }
                    // Render elements as `{e1, e2}` (without the outer
                    // `frozenset(...)` wrapper that render_value_repr adds).
                    let snapshot: Vec<_> = items.unwrap().iter().cloned().collect();
                    let mut parts = Vec::with_capacity(snapshot.len());
                    for k in &snapshot {
                        parts.push(render_key_repr(interp, k)?);
                    }
                    return Ok(format!("{class_name}({{{}}})", parts.join(", ")));
                }
                // bytearray subclass (#2386): CPython renders `ClassName(b'...')`
                // — the subclass name wrapping the bytes-content repr — unlike a
                // bytes subclass, which renders the bare base `b'...'` form.
                ValueKind::BuiltinObject { ops, .. }
                    if ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME =>
                {
                    if let Some(data) =
                        pyrust_builtins::bytearray::as_bytearray_snapshot(&backing)
                    {
                        let class_name = class.borrow().name.clone();
                        let inner = Value::bytes(data).repr_raw();
                        return Ok(format!("{class_name}({inner})"));
                    }
                }
                _ => {}
            }
        }
        // No __repr__ defined — fall back to default object repr (handles
        // exception instances via exception_repr() and plain instances via
        // the address-based format).
        return Ok(value.repr_raw());
    }

    // For containers, we need to recurse with interpreter access on each
    // element.  Use a thread-local cycle-detection stack identical in spirit
    // to the one in `Value::repr_raw()`.
    thread_local! {
        static REPR_IN_PROGRESS: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    }

    match value.kind() {
        ValueKind::List(items) => {
            // Fast path: all elements are plain scalars — no interpreter
            // dispatch needed.  `Value::repr_raw()` handles cycle detection
            // internally and produces the same output without a snapshot.
            if !items.iter().any(value_needs_interp_repr) {
                drop(items);
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in = id.is_some_and(|id| {
                REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id))
            });
            if already_in {
                return Ok("[...]".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            // Snapshot items so we can drop the `Ref` guard before calling
            // the interpreter (which may re-borrow the list).
            let snapshot: Vec<Value> = items.iter().cloned().collect();
            drop(items);
            let mut parts = Vec::with_capacity(snapshot.len());
            for item in &snapshot {
                parts.push(render_value_repr(interp, item)?);
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            Ok(format!("[{}]", parts.join(", ")))
        }
        ValueKind::Tuple(items) => {
            // Fast path: all elements are plain scalars — no interpreter
            // dispatch needed.
            if !items.iter().any(value_needs_interp_repr) {
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in = id.is_some_and(|id| {
                REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id))
            });
            if already_in {
                return Ok("(...)".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            // Tuple items are `&[Value]` — no Ref guard to drop.
            let snapshot: Vec<Value> = items.to_vec();
            let mut parts = Vec::with_capacity(snapshot.len());
            for item in &snapshot {
                parts.push(render_value_repr(interp, item)?);
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            let inner = parts.join(", ");
            if snapshot.len() == 1 {
                Ok(format!("({inner},)"))
            } else {
                Ok(format!("({inner})"))
            }
        }
        ValueKind::Dict(items) => {
            // Fast path: all keys and values are plain scalars — no interpreter
            // dispatch needed.
            if !items
                .iter()
                .any(|(k, v)| key_needs_interp_repr(k) || value_needs_interp_repr(v))
            {
                drop(items);
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in = id.is_some_and(|id| {
                REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id))
            });
            if already_in {
                return Ok("{...}".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            // Snapshot key-value pairs before dropping the guard.
            let snapshot: Vec<(PyKey, Value)> = items
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            drop(items);
            let mut out = String::new();
            out.push('{');
            for (i, (k, v)) in snapshot.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&render_key_repr(interp, k)?);
                out.push_str(": ");
                out.push_str(&render_value_repr(interp, v)?);
            }
            out.push('}');
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            Ok(out)
        }
        ValueKind::Set(items) => {
            if items.is_empty() {
                return Ok("set()".to_string());
            }
            // Fast path: all elements are plain scalar keys — no interpreter
            // dispatch needed.
            if !items.iter().any(key_needs_interp_repr) {
                drop(items);
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in = id.is_some_and(|id| {
                REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id))
            });
            if already_in {
                return Ok("{...}".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            let snapshot: Vec<PyKey> = items.iter().cloned().collect();
            drop(items);
            let mut parts = Vec::with_capacity(snapshot.len());
            for k in &snapshot {
                parts.push(render_key_repr(interp, k)?);
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            Ok(format!("{{{}}}", parts.join(", ")))
        }
        // Frozenset is stored as a BuiltinObject; its elements are PyKey so
        // they need render_key_repr to dispatch __repr__ on PyKey::Object.
        ValueKind::BuiltinObject { ops, .. }
            if ops.type_name() == pyrust_builtins::frozenset::TYPE_NAME =>
        {
            let items = match pyrust_builtins::frozenset::as_items(value) {
                Some(rc) => rc,
                None => return Ok(value.repr_raw()),
            };
            if items.is_empty() {
                return Ok("frozenset()".to_string());
            }
            // Fast path: all elements are plain scalar keys — no interpreter
            // dispatch needed.
            if !items.iter().any(key_needs_interp_repr) {
                drop(items);
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in = id.is_some_and(|id| {
                REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id))
            });
            if already_in {
                return Ok("{...}".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            let snapshot: Vec<PyKey> = items.iter().cloned().collect();
            drop(items);
            let mut parts = Vec::with_capacity(snapshot.len());
            for k in &snapshot {
                parts.push(render_key_repr(interp, k)?);
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            Ok(format!("frozenset({{{}}})", parts.join(", ")))
        }
        // Generators and built-in iterators (#2019): the pure
        // `Value::repr_raw()` cannot tell the concrete iterator kind apart
        // (all are `ValueKind::Generator`), so it returns a fixed
        // `<generator object>`.  Reconstruct CPython's real repr here:
        //   - true generators (def-generator / genexpr):
        //         `<generator object {qualname} at 0x...>`
        //   - everything else (map/filter/zip/enumerate/list_iterator/…):
        //         `<{type_name} object at 0x...>`
        ValueKind::Generator(_) => Ok(generator_repr(value)),
        // For all other value types (int, float, str, bool, None, …), the
        // pure `Value::repr_raw()` is correct and needs no interpreter.
        _ => Ok(value.repr_raw()),
    }
}

/// True when `v` is a *coroutine* object (an `async def` frame, issue #1039).
///
/// Coroutines share the `ValueKind::Generator` value tag but must behave
/// distinctly from plain generators: they are not iterable with `for` and they
/// report `type(coro).__name__ == "coroutine"`.
pub(crate) fn is_coroutine_value(v: &Value) -> bool {
    if let ValueKind::Generator(state_rc) = v.kind()
        // `try_borrow`: when the frame is currently checked out (mid-drive), the
        // borrow is held by the driver.  A frame being driven is not something
        // these iterability guards ever query, so treat a busy cell as "not a
        // coroutine" rather than panicking on a double borrow.
        && let Ok(borrow) = state_rc.try_borrow()
        && let Some(frame) = borrow.downcast_ref::<GeneratorFrame>()
    {
        return frame.is_coroutine;
    }
    false
}

/// True when `v` is an *async generator* object (`async def` containing
/// `yield`, issue #2280).  Async generators are coroutine-tagged but are not
/// themselves awaitable / runnable as coroutines: `asyncio.run(agen())` raises
/// `ValueError("a coroutine was expected, ...")`, matching CPython.
pub(crate) fn is_async_generator_value(v: &Value) -> bool {
    if let ValueKind::Generator(state_rc) = v.kind()
        && let Ok(borrow) = state_rc.try_borrow()
        && let Some(frame) = borrow.downcast_ref::<GeneratorFrame>()
    {
        return frame.is_async_generator();
    }
    false
}

/// Render the CPython-compatible repr of a `ValueKind::Generator` value
/// (#2019).  True generator frames carry a qualname
/// (`<generator object {qualname} at 0x...>`); built-in iterators use their
/// type name (`<{type_name} object at 0x...>`).  The address is the identity
/// of the underlying generator state, matching `id()` / `Value::value_id`.
fn generator_repr(value: &Value) -> String {
    let addr = value.value_id().unwrap_or(0) as usize;
    if let ValueKind::Generator(state_rc) = value.kind()
        && let Some(frame) = state_rc.borrow().downcast_ref::<GeneratorFrame>() {
            // Coroutines (`async def`, issue #1039) render as
            // `<coroutine object {qualname} at 0x...>`; async generators
            // (#2280) as `<async_generator object {qualname} at 0x...>`.
            let kind = if frame.is_async_generator() {
                "async_generator"
            } else if frame.is_coroutine {
                "coroutine"
            } else {
                "generator"
            };
            return format!("<{kind} object {} at 0x{addr:x}>", frame.qualname);
        }
    let type_name = full_type_name_str(value);
    format!("<{type_name} object at 0x{addr:x}>")
}

/// Render a `PyKey` dict key or set element to its repr string, honouring
/// `__repr__` on user instances stored as `PyKey::Object`.
///
/// Also recurses into `PyKey::Tuple` and `PyKey::FrozenSet` so that nested
/// user objects inside hashable compound keys get their `__repr__` called.
pub(crate) fn render_key_repr(interp: &mut crate::Interpreter, key: &PyKey) -> Result<String> {
    match key {
        PyKey::Object { value, .. } => render_value_repr(interp, value),
        PyKey::Tuple(items) => {
            if items.is_empty() {
                return Ok("()".to_string());
            }
            let mut parts = Vec::with_capacity(items.len());
            for item in items.iter() {
                parts.push(render_key_repr(interp, item)?);
            }
            if items.len() == 1 {
                Ok(format!("({},)", parts[0]))
            } else {
                Ok(format!("({})", parts.join(", ")))
            }
        }
        PyKey::FrozenSet(items) => {
            if items.is_empty() {
                return Ok("frozenset()".to_string());
            }
            let mut parts = Vec::with_capacity(items.len());
            for item in items.iter() {
                parts.push(render_key_repr(interp, item)?);
            }
            Ok(format!("frozenset({{{}}})", parts.join(", ")))
        }
        _ => Ok(pyrust_core::key_repr(key)),
    }
}

/// Render `value` to its Python-string form, honouring `__str__` / `__repr__`
/// on user instances (in that priority order) and falling back to
/// `<ClassName object>` for instances of classes that define neither.
///
/// Shared by `print` and `str(x)` — both want the same dunder-aware
/// rendering, just wrapped differently (`print` collects into a `Vec<String>`,
/// `str(x)` returns a `Value::string(...)`).  Exception instances without a
/// user-defined `__str__` fall back to `Value::to_py_str()` (matching
/// CPython's `BaseException.__str__`); those with a user-defined `__str__`
/// call it via the normal dunder dispatch loop.
fn render_instance_str(interp: &mut crate::Interpreter, value: &Value) -> Result<String> {
    // gh-95778: reject a base-10 int->str conversion (directly or nested inside
    // a container) that exceeds `sys.get_int_max_str_digits()`.
    // `check_int_str_conversion` fast-rejects non-BigInt/non-container values
    // from their NaN-box tag alone, so the common `str(int)` path pays nothing.
    pyrust_core::check_int_str_conversion(value)?;
    let ValueKind::PyInstance(inst) = value.kind() else {
        // For containers, str() is defined as repr() in CPython.  Route
        // through render_value_repr so that PyInstance elements inside a
        // list/tuple/dict/set get their __repr__ called.
        return match value.kind() {
            ValueKind::List(_)
            | ValueKind::Tuple(_)
            | ValueKind::Dict(_)
            | ValueKind::Set(_) => render_value_repr(interp, value),
            // frozenset: str() == repr() in CPython; delegate to
            // render_value_repr so nested user instances get __repr__.
            ValueKind::BuiltinObject { ops, .. }
                if ops.type_name() == pyrust_builtins::frozenset::TYPE_NAME =>
            {
                render_value_repr(interp, value)
            }
            _ => Ok(value.to_py_str()),
        };
    };
    let inst_rc = Rc::clone(inst);
    let class = Rc::clone(&inst_rc.borrow().class);
    // For exception instances, fall back to built-in exception formatting only
    // when the class has no user-defined __str__.  A user-defined __str__ is
    // one whose resolved value is not a BuiltinFunction — i.e. it was declared
    // in user code, not registered as a Rust built-in.
    if is_exception_class(&class) {
        let has_user_str = lookup_class_attr(&class, "__str__")
            .map(|v| !matches!(v.kind(), ValueKind::BuiltinFunction(_)))
            .unwrap_or(false);
        if !has_user_str {
            return crate::interpreter::exception_str_with_dispatch(interp, value, &inst_rc, &class);
        }
    }
    // Issue #1204 / #1564: if this instance subclasses a scalar primitive,
    // delegate str() to the backing primitive value when appropriate.
    //
    // For str/bytes subclasses: CPython's str.__str__ returns self directly and
    // never consults __repr__.  So the early-return for str/bytes backing must
    // happen AFTER a user __str__ but BEFORE the __repr__ dispatch.
    //
    // For int/float/bool/BigInt subclasses: CPython's int.__str__ calls
    // __repr__, so the early-return for numeric types must only happen when
    // neither __str__ nor __repr__ is user-defined.
    let has_user_str_dunder = lookup_class_attr(&class, "__str__")
        .map(|v| matches!(v.kind(), ValueKind::UserFunction(_)))
        .unwrap_or(false);
    let has_user_repr_dunder = lookup_class_attr(&class, "__repr__")
        .map(|v| matches!(v.kind(), ValueKind::UserFunction(_)))
        .unwrap_or(false);
    // str/bytes backing: return early unless a user __str__ is defined.
    // (A user __repr__ does NOT override str.__str__ in CPython.)
    if !has_user_str_dunder
        && let Some(backing) = instance_builtin_data(&inst_rc) {
            match backing.kind() {
                ValueKind::Str(_) | ValueKind::Bytes(_) => return Ok(backing.to_py_str()),
                _ => {}
            }
        }
    // int/float/bool/BigInt backing: return early only when neither user
    // __str__ nor user __repr__ is defined (matching CPython's int.__str__
    // which calls __repr__).
    if !has_user_str_dunder && !has_user_repr_dunder
        && let Some(backing) = instance_builtin_data(&inst_rc) {
            match backing.kind() {
                ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Bool(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _) => return Ok(backing.to_py_str()),
                _ => {}
            }
        }
    // Issue #1537: skip `object.__str__` / `object.__repr__` sentinels when
    // the instance has a primitive backing store.  Primitive types now set
    // `object` as an explicit MRO base, making these reachable for user
    // subclasses.  The backing-data path below renders the contents correctly.
    for dunder in &["__str__", "__repr__"] {
        if let Some(method_val) = lookup_class_attr(&class, dunder) {
            let is_object_dunder = matches!(
                method_val.kind(),
                ValueKind::BuiltinFunction("object.__str__")
                    | ValueKind::BuiltinFunction("object.__repr__")
            );
            if is_object_dunder && instance_builtin_data(&inst_rc).is_some() {
                continue;
            }
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
    // Issue #1205: no __str__ or __repr__ in MRO (or object.* sentinels
    // skipped) — delegate to the backing
    // container so that list/dict/tuple/set subclasses render their contents
    // via str() just as they do via repr().
    if let Some(backing) = instance_builtin_data(&inst_rc) {
        match backing.kind() {
            ValueKind::List(_) | ValueKind::Dict(_) | ValueKind::Tuple(_) => {
                return render_value_repr(interp, &backing);
            }
            ValueKind::Set(items) => {
                let class_name = class.borrow().name.clone();
                if items.is_empty() {
                    return Ok(format!("{class_name}()"));
                }
                let inner = render_value_repr(interp, &backing)?;
                return Ok(format!("{class_name}({inner})"));
            }
            ValueKind::BuiltinObject { ops, .. }
                if ops.type_name() == pyrust_builtins::frozenset::TYPE_NAME =>
            {
                let class_name = class.borrow().name.clone();
                let items = pyrust_builtins::frozenset::as_items(&backing);
                let is_empty = items.as_ref().is_none_or(|rc| rc.is_empty());
                if is_empty {
                    return Ok(format!("{class_name}()"));
                }
                // Render elements as `{e1, e2}` without the outer `frozenset(...)`
                let snapshot: Vec<_> = items.unwrap().iter().cloned().collect();
                let mut parts = Vec::with_capacity(snapshot.len());
                for k in &snapshot {
                    parts.push(render_key_repr(interp, k)?);
                }
                return Ok(format!("{class_name}({{{}}})", parts.join(", ")));
            }
            // bytearray subclass (#2386): `str(BA(...))` == `repr(BA(...))` in
            // CPython (bytearray has no `__str__`, so `object.__str__` calls
            // `__repr__`), rendering `ClassName(b'...')`.  Delegate to
            // `render_value_repr`, which now handles the bytearray subclass.
            ValueKind::BuiltinObject { ops, .. }
                if ops.type_name() == pyrust_builtins::bytearray::TYPE_NAME =>
            {
                return render_value_repr(interp, value);
            }
            _ => {}
        }
    }
    Ok(value.repr_raw())
}

/// Return the Python iterator type name for a builtin collection, matching
/// CPython 3.12's iterator type names (e.g. "list_iterator", "set_iterator").
///
/// Used by `iter()` to tag `NativeIterFrame` with the right name so that
/// `type(iter([1,2,3])).__name__` and error messages in `reversed()` are
/// correct.  Any type that does not map to a specific CPython iterator name
/// falls back to "generator".
fn builtin_iter_type_name(v: &Value) -> &'static str {
    match v.kind() {
        ValueKind::List(_) => "list_iterator",
        ValueKind::Tuple(_) => "tuple_iterator",
        // CPython 3.12 uses "str_ascii_iterator" for pure-ASCII strings and
        // "str_iterator" for strings containing non-ASCII characters.
        ValueKind::Str(_) => {
            if v.str_is_ascii() {
                "str_ascii_iterator"
            } else {
                "str_iterator"
            }
        }
        ValueKind::Set(_) => "set_iterator",
        ValueKind::Dict(_) => "dict_keyiterator",
        ValueKind::Range { .. } => "range_iterator",
        ValueKind::Bytes(_) => "bytes_iterator",
        // dict view iterators: "dict_keys" → "dict_keyiterator", etc.
        // CPython uses "set_iterator" for frozenset iteration as well.
        // OrderedDict-backed views (keys/values/items, tagged `ordered`) all
        // iterate as "odict_iterator" in CPython 3.12, regardless of the view
        // kind (#2748) — matching `iter(od)` itself.
        ValueKind::BuiltinObject { .. }
            if pyrust_builtins::dict_views::is_ordered_view(v) =>
        {
            "odict_iterator"
        }
        ValueKind::BuiltinObject { ops, .. } => match ops.type_name() {
            "dict_keys" => "dict_keyiterator",
            "dict_values" => "dict_valueiterator",
            "dict_items" => "dict_itemiterator",
            "frozenset" => "set_iterator",
            _ => "generator",
        },
        _ => "generator",
    }
}

/// Parse and validate arguments for `exec()` and `eval()`.
///
/// Both accept `(source[, globals[, locals]])`.  Returns
/// `(source_value, globals_option, locals_option)`.
fn parse_exec_eval_args(
    fn_name: &str,
    args: &[crate::interpreter::ExpandedCallArg],
) -> Result<(Value, Option<Value>, Option<Value>)> {
    // Reject keyword arguments.
    if args.iter().any(|a| a.name.is_some()) {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() takes no keyword arguments"),
        ));
    }
    if args.is_empty() || args.len() > 3 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes from 1 to 3 positional arguments ({} given)",
                args.len()
            ),
        ));
    }
    let source_val = args[0].value.clone();
    let globals_opt = args.get(1).map(|a| a.value.clone()).and_then(|v| {
        if matches!(v.kind(), ValueKind::None) { None } else { Some(v) }
    });
    let locals_opt = args.get(2).map(|a| a.value.clone()).and_then(|v| {
        if matches!(v.kind(), ValueKind::None) { None } else { Some(v) }
    });
    Ok((source_val, globals_opt, locals_opt))
}


/// CPython injects `__builtins__` into a caller-supplied globals dict the first
/// time `eval()` or `exec()` is called with it (see `PyEval_EvalCode`).  If the
/// dict does not already contain `"__builtins__"`, insert the builtins module.
///
/// Existing values (including `{}` as a deliberate override) are left alone.
fn inject_builtins_into_globals(globals: &Value) {
    use crate::value::StrKey;
    let already_present = globals
        .dict_with(|d| d.contains_key(&StrKey("__builtins__")))
        .unwrap_or(true); // not a dict — leave it alone
    if !already_present {
        let builtins = crate::interpreter::cached_builtins_module();
        let _ = globals.dict_insert(PyKey::str_from("__builtins__"), builtins);
    }
}

/// Return the Python type name for a value, with correct iterator type names
/// for Generator variants (e.g. "list_iterator", "map", "filter").
///
/// `value_type_name_str` (from `pyrust-core`) cannot distinguish between
/// `NativeIterFrame`-based iterators and true generator frames because they
/// are both stored as `ValueKind::Generator`.  This wrapper downcasts the
/// generator state to recover the specific type name stored in
/// `NativeIterFrame::type_name`, or the sentinel names for `MapIter` /
/// `FilterIter` / `EnumerateIter` / `ZipIter` / `CallableIter` /
/// `GetItemIter`.  All other value kinds delegate to `value_type_name_str`.
fn full_type_name_str(v: &Value) -> std::borrow::Cow<'static, str> {
    if let ValueKind::Generator(state_rc) = v.kind() {
        let borrow = state_rc.borrow();
        if borrow.downcast_ref::<MapIter>().is_some() {
            return std::borrow::Cow::Borrowed("map");
        }
        if borrow.downcast_ref::<FilterIter>().is_some() {
            return std::borrow::Cow::Borrowed("filter");
        }
        if borrow.downcast_ref::<ChainFromIterableIter>().is_some() {
            // Fully-qualified for repr / error messages (`<itertools.chain
            // object ...>`, `'itertools.chain' object ...`), matching the
            // native `chain` BuiltinObject and CPython (#2362).  `type().__name__`
            // strips the module prefix in `type_of`'s Generator arm below.
            return std::borrow::Cow::Borrowed("itertools.chain");
        }
        if borrow.downcast_ref::<EnumerateIter>().is_some() {
            return std::borrow::Cow::Borrowed("enumerate");
        }
        if borrow.downcast_ref::<ZipIter>().is_some() {
            return std::borrow::Cow::Borrowed("zip");
        }
        if borrow.downcast_ref::<CallableIter>().is_some() {
            return std::borrow::Cow::Borrowed("callable_iterator");
        }
        if borrow.downcast_ref::<GetItemIter>().is_some() {
            return std::borrow::Cow::Borrowed("iterator");
        }
        if borrow.downcast_ref::<BigRangeIter>().is_some() {
            return std::borrow::Cow::Borrowed("longrange_iterator");
        }
        if let Some(native) = borrow.downcast_ref::<NativeIterFrame>() {
            return std::borrow::Cow::Borrowed(native.type_name);
        }
        if borrow.downcast_ref::<AsyncGenASend>().is_some() {
            return std::borrow::Cow::Borrowed("async_generator_asend");
        }
        if let Some(frame) = borrow.downcast_ref::<GeneratorFrame>() {
            if frame.is_async_generator() {
                return std::borrow::Cow::Borrowed("async_generator");
            }
            if frame.is_coroutine {
                return std::borrow::Cow::Borrowed("coroutine");
            }
        }
    }
    value_type_name_str(v)
}

/// Public wrapper over [`full_type_name_str`] for call sites outside this
/// module (e.g. the VM's `for x in coro` TypeError, #2280) that need the
/// async-aware type name (`coroutine` / `async_generator`).
pub(crate) fn full_type_name_str_pub(v: &Value) -> std::borrow::Cow<'static, str> {
    full_type_name_str(v)
}
