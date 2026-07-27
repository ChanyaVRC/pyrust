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
    Ok(Value::string(pyrust_core::cesu8_encode_codepoint(
        code_point as u32,
    )))
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

    let make = |v: Value| ExpandedCallArg {
        name: None,
        value: v,
    };

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
/// - `Ok(Err((out, i)))` — element `i` is a user object that may carry
///   `__index__` on its class or metaclass; the caller resolves `items[i..]`
///   via `bytes_element_to_u8`;
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
                ));
            }
            ValueKind::PyInstance(_) | ValueKind::PyClass(_) => return Ok(Err((out, i))),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        pyrust_core::builtin_type_name(v),
                    ),
                ));
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

/// Convert a single element of a `bytes()` / `bytearray()` source iterable to a
/// `u8`, honoring CPython 3.12's shared `__index__` protocol. Plain `int` /
/// `bool` short-circuit in [`try_fast_bytes_elems`]; this slow path owns only
/// the byte-specific range check. An int outside `0..=255` (after
/// `__index__`) raises
/// `ValueError: bytes must be in range(0, 256)`; a non-integer without
/// `__index__` raises `TypeError: 'X' object cannot be interpreted as an
/// integer`; `__index__` returning a non-int raises
/// `TypeError: __index__ returned non-int (type X)`.
fn bytes_element_to_u8(interp: &mut crate::Interpreter, v: &Value) -> Result<u8> {
    let resolved = interp.value_to_index(v, |value| {
        PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                pyrust_core::builtin_type_name(value),
            ),
        )
    })?;
    bytes_index_value_to_u8(&resolved)
}

/// Range-check an integer already resolved by `value_to_index` for `bytes()` /
/// `bytearray()`.
fn bytes_index_value_to_u8(result: &Value) -> Result<u8> {
    match result.kind() {
        ValueKind::Bool(b) => Ok(b as u8),
        ValueKind::Int(n) if (0..=255).contains(&n) => Ok(n as u8),
        ValueKind::Int(_) | ValueKind::BigInt(_) => Err(PyError::named(
            "ValueError",
            "bytes must be in range(0, 256)".to_string(),
        )),
        _ => unreachable!("value_to_index guarantees an integer"),
    }
}

/// Resolve the `bytes(n)` / `bytearray(n)` count argument through the shared
/// optional index protocol. Returns `Some(count)` on success and `None` only
/// when the value has no usable count. CPython treats a `TypeError` from
/// `__index__` (including an invalid slot result) like a missing count and
/// continues with the iterable source form; other slot exceptions propagate.
fn bytes_count_via_index(interp: &mut crate::Interpreter, val: &Value) -> Result<Option<usize>> {
    let result = match interp.try_value_to_index(val) {
        Ok(Some(result)) => result,
        Ok(None) => return Ok(None),
        Err(error) if error.class_name_is("TypeError") => return Ok(None),
        Err(error) => return Err(error),
    };
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
        _ => unreachable!("try_value_to_index guarantees an integer"),
    };
    if count < 0 {
        return Err(PyError::named("ValueError", "negative count".to_string()));
    }
    Ok(Some(count as usize))
}
