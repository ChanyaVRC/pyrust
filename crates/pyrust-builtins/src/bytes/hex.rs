/// `bytes.hex(sep=None, bytes_per_sep=1)`
///
/// Without a separator: return a hex string of all bytes.
/// With a separator: positive `bytes_per_sep` groups from the RIGHT (leftmost
/// group gets the remainder); negative groups from the LEFT (rightmost group
/// gets the remainder).  `bytes_per_sep=0` is treated as "no separator".
/// The separator must be a single ASCII character.
fn bytes_hex(bytes: &[u8], args: &[Value], kwargs: &PyDict) -> Result<Value> {
    // Merge positional args with `sep` / `bytes_per_sep` keyword arguments.
    // Positional args take precedence; a keyword that duplicates a positional
    // arg is a TypeError, matching CPython.
    let merged = merge_hex_kwargs(args, kwargs)?;

    // CPython runs the bytes_per_sep converter unconditionally and *before* any
    // sep validation — even when no sep is present (where the value is otherwise
    // unused). It is the Argument Clinic `int` converter, so it (1) rejects
    // non-integers with the standard coercion TypeError and (2) rejects integers
    // that do not fit a C `int` (32-bit) with OverflowError. merge_hex_kwargs
    // returns an empty vec when no sep is given, so we check the original
    // args/kwargs to reach a kwarg-only bytes_per_sep.
    let bps_raw: Option<&Value> = if merged.len() >= 2 {
        merged.get(1)
    } else {
        args.get(1).or_else(|| kwargs.get(&StrKey("bytes_per_sep")))
    };
    let bytes_per_sep: i64 = match bps_raw.map(|v| v.kind()) {
        None => 1,
        Some(ValueKind::Bool(b)) => b as i64,
        Some(ValueKind::Int(n)) => {
            // CPython's int converter targets a C `int` (32-bit).
            if i32::try_from(n).is_err() {
                return Err(PyError::named(
                    "OverflowError",
                    "Python int too large to convert to C int".to_string(),
                ));
            }
            n
        }
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C int".to_string(),
            ));
        }
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    builtin_type_name(bps_raw.unwrap())
                ),
            ));
        }
    };

    let args: &[Value] = &merged;
    if args.is_empty() {
        // Fast path: no separator. bytes_per_sep already validated above.
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
        }
        return Ok(Value::string(out));
    }

    // args[0] = sep (str or bytes), args[1] = bytes_per_sep (int, default 1)
    //
    // CPython 3.12 validation order (from Objects/bytesobject.c):
    //   1. len(sep) — raises TypeError "object of type 'X' has no len()" if
    //      the type has no __len__.
    //   2. len != 1 → ValueError "sep must be length 1."
    //   3. not str/bytes → TypeError "sep must be str or bytes."
    //   4. non-ASCII → ValueError "sep must be ASCII."
    let sep_buf: String;
    let sep_arg = &args[0];
    // Step 1: determine the static len for sized container types.  For types
    // that have no concept of length (int, float, …) raise the same TypeError
    // CPython does via PyObject_Length → PySequence_Length.
    let sep_len: usize = match sep_arg.kind() {
        ValueKind::Str(s) => s.chars().count(),
        ValueKind::Bytes(rc) => rc.len(),
        ValueKind::List(v) => v.len(),
        ValueKind::Tuple(rc) => rc.len(),
        ValueKind::Dict(v) => v.len(),
        ValueKind::Set(v) => v.len(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "object of type '{}' has no len()",
                    builtin_type_name(sep_arg)
                ),
            ));
        }
    };
    // Step 2: length check.
    if sep_len != 1 {
        return Err(PyError::named(
            "ValueError",
            "sep must be length 1.".to_string(),
        ));
    }
    // Step 3 + 4: type check then ASCII check.
    let sep: &str = match sep_arg.kind() {
        ValueKind::Str(s) => {
            if !s.is_ascii() {
                return Err(PyError::named(
                    "ValueError",
                    "sep must be ASCII.".to_string(),
                ));
            }
            s
        }
        ValueKind::Bytes(rc) => {
            let byte = rc[0];
            if !byte.is_ascii() {
                return Err(PyError::named(
                    "ValueError",
                    "sep must be ASCII.".to_string(),
                ));
            }
            sep_buf = (byte as char).to_string();
            &sep_buf
        }
        _ => {
            // len == 1 but not str or bytes (e.g. a one-element list).
            return Err(PyError::named(
                "TypeError",
                "sep must be str or bytes.".to_string(),
            ));
        }
    };

    // bytes_per_sep was resolved and range-checked up front (before sep
    // validation), matching CPython's Argument Clinic converter ordering.
    if bytes_per_sep == 0 {
        // CPython 3.12: bytes_per_sep=0 means "no separator" — returns plain hex.
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
        }
        return Ok(Value::string(out));
    }

    // Group the hex nibbles according to bytes_per_sep.
    //
    // CPython semantics (from the C source):
    //   Positive bytes_per_sep: separator inserted from the RIGHT — rightmost
    //   groups have exactly `bytes_per_sep` bytes; the leftmost group gets the
    //   remainder (may be shorter).  e.g. b'\x00\x01\x02\x03\x04'.hex(':', 2)
    //   → "00:0102:0304"
    //
    //   Negative bytes_per_sep: separator inserted from the LEFT — leftmost
    //   groups have exactly `|bytes_per_sep|` bytes; the rightmost group gets
    //   the remainder.  e.g. b'\x00\x01\x02\x03\x04'.hex(':', -2)
    //   → "0001:0203:04"
    let n = bytes.len();
    if n == 0 {
        return Ok(Value::string(""));
    }

    // Render all bytes as hex pairs first.
    let hex_pairs: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();

    let group_size = bytes_per_sep.unsigned_abs() as usize;
    let mut groups: Vec<String> = Vec::new();

    if bytes_per_sep > 0 {
        // Group from right: the first group may be smaller (the remainder).
        let rem = n % group_size;
        let mut i = 0;
        if rem > 0 {
            groups.push(hex_pairs[0..rem].concat());
            i = rem;
        }
        while i < n {
            let end = i + group_size;
            groups.push(hex_pairs[i..end].concat());
            i = end;
        }
    } else {
        // Group from left: the last group may be smaller (the remainder).
        let mut i = 0;
        while i < n {
            let end = (i + group_size).min(n);
            groups.push(hex_pairs[i..end].concat());
            i = end;
        }
    }

    Ok(Value::string(groups.join(sep)))
}

/// Merge `bytes.hex()` positional args with `sep` / `bytes_per_sep` keyword
/// arguments into a single positional vector `[sep?, bytes_per_sep?]`.
///
/// CPython's argument clinic checks, in order:
///   1. total arg count > 2 → "takes at most 2 (keyword )arguments (N given)";
///   2. unknown keyword → "'K' is an invalid keyword argument for hex()";
///   3. a keyword duplicating a positional → "argument for hex() given by name
///      ('K') and position (P)".
fn merge_hex_kwargs(args: &[Value], kwargs: &PyDict) -> Result<Vec<Value>> {
    // The total-count overflow check runs even for the all-positional form:
    // `hex("-", 1, 2)` is a TypeError in CPython, not a silent drop of the
    // third positional. The noun is "keyword arguments" only when every excess
    // argument came in by keyword (i.e. no positionals at all).
    let total = args.len() + kwargs.len();
    if total > 2 {
        let noun = if args.is_empty() {
            "keyword arguments"
        } else {
            "arguments"
        };
        return Err(PyError::named(
            "TypeError",
            format!("hex() takes at most 2 {noun} ({total} given)"),
        ));
    }

    if kwargs.is_empty() {
        return Ok(args.to_vec());
    }

    // Reject unknown keyword arguments.
    for key in kwargs.keys() {
        if let PyKey::Str(s) = key {
            let name = s.as_str().unwrap_or("");
            if name != "sep" && name != "bytes_per_sep" {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{name}' is an invalid keyword argument for hex()"),
                ));
            }
        }
    }

    let kw_sep = kwargs.get(&StrKey("sep"));
    let kw_bps = kwargs.get(&StrKey("bytes_per_sep"));

    // A keyword must not duplicate a positional argument.
    if !args.is_empty() && kw_sep.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for hex() given by name ('sep') and position (1)".to_string(),
        ));
    }
    if args.get(1).is_some() && kw_bps.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for hex() given by name ('bytes_per_sep') and position (2)".to_string(),
        ));
    }

    // Assemble the effective positional vector. `bytes_per_sep` only takes
    // effect when a separator is present (CPython treats `hex(bytes_per_sep=2)`
    // with no sep as a plain hex string).
    let sep = args.first().or(kw_sep);
    let bps = args.get(1).or(kw_bps);
    let mut merged = Vec::with_capacity(2);
    match sep {
        None => return Ok(merged),
        Some(s) => merged.push(s.clone()),
    }
    if let Some(b) = bps {
        merged.push(b.clone());
    }
    Ok(merged)
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------
