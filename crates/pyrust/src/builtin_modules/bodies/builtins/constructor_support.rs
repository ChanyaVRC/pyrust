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

/// Invoke issue #3000's existing exact built-in constructor as a native
/// `__new__` slot.  Exact classes return that constructor's value directly;
/// subclassable iterator types retain a matching native iterator as the
/// subclass instance's internal backing.
fn builtin_type_new(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    kind: BuiltinTypeClass,
) -> Result<Value> {
    let Some((class_arg, constructor_args)) = args
        .split_first()
        .filter(|(class_arg, _)| class_arg.name.is_none())
    else {
        return Err(pyrust_core::type_err!(
            "{}.__new__(): not enough arguments",
            kind.class_name()
        ));
    };
    let class = match class_arg.value.kind() {
        ValueKind::PyClass(class) => Rc::clone(class),
        _ => {
            return Err(pyrust_core::type_err!(
                "{}.__new__(X): X is not a type object ({})",
                kind.class_name(),
                value_type_name_str(&class_arg.value)
            ));
        }
    };
    let base = kind.singleton();
    if !class_is_subclass_of(&class, &base) {
        let class_name = class.borrow().name.clone();
        return Err(pyrust_core::type_err!(
            "{}.__new__({class_name}): {class_name} is not a subtype of {}",
            kind.class_name(),
            kind.class_name()
        ));
    }
    // Preserve factory provenance across the registry call.  A user
    // `__reversed__` result always passes through unchanged; its concrete type
    // is not evidence that the native constructor created it.
    let reversed_passthrough = kind == BuiltinTypeClass::Reversed
        && constructor_args.len() == 1
        && constructor_args[0].name.is_none()
        && iteration::reversed_uses_special_method(&constructor_args[0].value);
    let dispatch = crate::builtin_registry::lookup(kind.class_name()).ok_or_else(|| {
        PyError::Runtime(format!(
            "internal: {} constructor is not registered",
            kind.class_name()
        ))
    })?;
    let backing = dispatch(interp, constructor_args)?;
    if Rc::ptr_eq(&class, &base) {
        return Ok(backing);
    }
    if reversed_passthrough {
        return Ok(backing);
    }

    // `reversed` is a factory as well as the generic reverse-iterator type.
    // A sequence-specialised cursor (`list_reverseiterator`, `range_iterator`)
    // or an arbitrary value returned by `obj.__reversed__()` is already the
    // final constructor result.  Only the generic reversed backing belongs in
    // a `reversed` subclass carrier.
    let backing_matches = builtin_type_class_isinstance_fast(&backing, &base) == Some(true);
    if !backing_matches {
        if kind == BuiltinTypeClass::Reversed {
            return Ok(backing);
        }
        return Err(PyError::Runtime(format!(
            "internal: {} constructor returned an incompatible backing",
            kind.class_name()
        )));
    }

    let mut attrs = InstanceAttrs::new();
    attrs.insert(crate::interpreter::BUILTIN_DATA_ATTR, backing);
    Ok(Value::py_instance(Rc::new(std::cell::RefCell::new(
        crate::value::PyInstance { class, attrs },
    ))))
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
        .get_slot("args")
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
    Ok(Value::tuple(vec![
        Value::float(quotient),
        Value::float(modulo),
    ]))
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
