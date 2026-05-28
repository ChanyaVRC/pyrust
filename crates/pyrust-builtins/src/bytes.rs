use indexmap::IndexMap;
use pyrust_core::{
    PyError, PyKey, Result, StrKey, Value, ValueKind, builtin_type_name, py_value_display_name,
};

/// Canonical list of method names dispatched by `call`.
/// Single source of truth for `has_method` and the drift-guard test.
pub const METHODS: &[&str] = &[
    "__iter__",
    "hex",
    "decode",
    "startswith",
    "endswith",
    "find",
    "rfind",
    "index",
    "rindex",
    "count",
    "upper",
    "lower",
    // Added in #829
    "replace",
    "strip",
    "lstrip",
    "rstrip",
    "removeprefix",
    "removesuffix",
    "split",
    "rsplit",
    "splitlines",
    "join",
    "title",
    "capitalize",
    "isdigit",
    "isalpha",
    "isalnum",
    "isupper",
    "islower",
    "isspace",
    "center",
    "ljust",
    "rjust",
    "zfill",
    "translate",
    // Added in #1425
    "partition",
    "rpartition",
    "swapcase",
    "isascii",
    "istitle",
    // Added in #1170
    "expandtabs",
];

/// Returns `true` if `method` is the name of a built-in `bytes` method.
/// Used by `hasattr` / `getattr` to validate attribute names without
/// invoking the method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

pub fn call(
    method: &str,
    receiver: &Value,
    args: &[Value],
    kwargs: &IndexMap<PyKey, Value>,
) -> Result<Value> {
    let bytes: &[u8] = match receiver.kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "expected bytes receiver, got {}",
                    pyrust_core::builtin_type_name(receiver)
                ),
            ));
        }
    };
    match method {
        "hex" => bytes_hex(bytes, args),
        "decode" => bytes_decode(bytes, args, kwargs),
        "startswith" => bytes_startswith(bytes, args),
        "endswith" => bytes_endswith(bytes, args),
        "find" => bytes_find(bytes, args),
        "rfind" => bytes_rfind(bytes, args),
        "index" => bytes_index(bytes, args),
        "rindex" => bytes_rindex(bytes, args),
        "count" => bytes_count(bytes, args),
        "upper" => Ok(Value::bytes(
            bytes.iter().map(|b| b.to_ascii_uppercase()).collect(),
        )),
        "lower" => Ok(Value::bytes(
            bytes.iter().map(|b| b.to_ascii_lowercase()).collect(),
        )),
        // Added in #829
        "replace" => bytes_replace(bytes, args),
        "strip" => bytes_strip(bytes, args, true, true),
        "lstrip" => bytes_strip(bytes, args, true, false),
        "rstrip" => bytes_strip(bytes, args, false, true),
        "removeprefix" => bytes_removeprefix(bytes, args),
        "removesuffix" => bytes_removesuffix(bytes, args),
        "split" => bytes_split(bytes, args),
        "rsplit" => bytes_rsplit(bytes, args),
        "splitlines" => bytes_splitlines(bytes, args),
        "join" => bytes_join(bytes, args),
        "title" => Ok(Value::bytes(bytes_title(bytes))),
        "capitalize" => Ok(Value::bytes(bytes_capitalize(bytes))),
        "isdigit" => Ok(Value::bool_(
            !bytes.is_empty() && bytes.iter().all(|b| b.is_ascii_digit()),
        )),
        "isalpha" => Ok(Value::bool_(
            !bytes.is_empty() && bytes.iter().all(|b| b.is_ascii_alphabetic()),
        )),
        "isalnum" => Ok(Value::bool_(
            !bytes.is_empty() && bytes.iter().all(|b| b.is_ascii_alphanumeric()),
        )),
        "isupper" => Ok(Value::bool_(bytes_isupper(bytes))),
        "islower" => Ok(Value::bool_(bytes_islower(bytes))),
        "isspace" => Ok(Value::bool_(
            !bytes.is_empty() && bytes.iter().all(|b| b.is_ascii_whitespace()),
        )),
        "center" => bytes_center(bytes, args),
        "ljust" => bytes_ljust(bytes, args),
        "rjust" => bytes_rjust(bytes, args),
        "zfill" => bytes_zfill(bytes, args),
        "translate" => bytes_translate(bytes, args, kwargs),
        // Added in #1425
        "partition" => bytes_partition(bytes, args, false),
        "rpartition" => bytes_partition(bytes, args, true),
        "swapcase" => Ok(Value::bytes(
            bytes
                .iter()
                .map(|&b| {
                    if b.is_ascii_uppercase() {
                        b.to_ascii_lowercase()
                    } else if b.is_ascii_lowercase() {
                        b.to_ascii_uppercase()
                    } else {
                        b
                    }
                })
                .collect(),
        )),
        "isascii" => Ok(Value::bool_(bytes.iter().all(|&b| b < 128))),
        "istitle" => Ok(Value::bool_(bytes_istitle(bytes))),
        // Added in #1170
        "expandtabs" => bytes_expandtabs(bytes, args),
        _ => Err(PyError::named(
            "AttributeError",
            format!("'bytes' object has no attribute '{method}'"),
        )),
    }
}

// ---------------------------------------------------------------------------
// hex
// ---------------------------------------------------------------------------

/// `bytes.hex(sep=None, bytes_per_sep=1)`
///
/// Without a separator: return a hex string of all bytes.
/// With a separator: positive `bytes_per_sep` groups from the RIGHT (leftmost
/// group gets the remainder); negative groups from the LEFT (rightmost group
/// gets the remainder).  `bytes_per_sep=0` is treated as "no separator".
/// The separator must be a single ASCII character.
fn bytes_hex(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.is_empty() {
        // Fast path: no separator.
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

    let bytes_per_sep: i64 = match args.get(1).map(|v| v.kind()) {
        None => 1,
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.hex() bytes_per_sep must be an integer".to_string(),
            ));
        }
    };

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
        return Ok(Value::string("".to_string()));
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

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

fn bytes_decode(bytes: &[u8], args: &[Value], kwargs: &IndexMap<PyKey, Value>) -> Result<Value> {
    // Signature: decode(encoding='utf-8', errors='strict')
    //
    // CPython checks the total argument count before individual duplicate checks.
    // When positional args are present the message says "arguments"; when it is
    // all-kwargs it says "keyword arguments".
    let total = args.len() + kwargs.len();
    if total > 2 {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("decode() takes at most 2 keyword arguments ({total} given)"),
            ));
        }
        return Err(PyError::named(
            "TypeError",
            format!("decode() takes at most 2 arguments ({total} given)"),
        ));
    }
    // Reject unknown keyword arguments first.
    for key in kwargs.keys() {
        if let PyKey::Str(s) = key {
            let name = s.as_str().unwrap_or("");
            if name != "encoding" && name != "errors" {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{name}' is an invalid keyword argument for decode()"),
                ));
            }
        }
    }

    let kw_encoding: Option<String> = match kwargs.get(&StrKey("encoding")) {
        None => None,
        Some(v) => match v.kind() {
            ValueKind::Str(s) => Some(s.to_owned()),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decode() argument 'encoding' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };
    let kw_errors: Option<String> = match kwargs.get(&StrKey("errors")) {
        None => None,
        Some(v) => match v.kind() {
            ValueKind::Str(s) => Some(s.to_owned()),
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decode() argument 'errors' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };

    // Validate that a keyword isn't also supplied positionally.
    if args.first().is_some() && kw_encoding.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for decode() given by name ('encoding') and position (1)".to_string(),
        ));
    }
    if args.get(1).is_some() && kw_errors.is_some() {
        return Err(PyError::named(
            "TypeError",
            "argument for decode() given by name ('errors') and position (2)".to_string(),
        ));
    }

    let encoding: &str = match args.first() {
        None => kw_encoding.as_deref().unwrap_or("utf-8"),
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decode() argument 'encoding' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };

    let errors: &str = match args.get(1) {
        None => kw_errors.as_deref().unwrap_or("strict"),
        Some(v) => match v.kind() {
            ValueKind::Str(s) => s,
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "decode() argument 'errors' must be str, not {}",
                        py_value_display_name(v)
                    ),
                ));
            }
        },
    };

    decode_bytes(bytes, encoding, errors)
}

/// Decode `bytes` using the given `encoding` and `errors` handler.
///
/// Shared implementation for `bytes.decode()` and the 2/3-arg form of
/// `str(bytes, encoding[, errors])`.
///
/// Supported encodings: `utf-8` (and aliases), `latin-1` (and aliases), `ascii`.
/// Supported error handlers: `strict`, `replace`, `ignore`.
pub fn decode_bytes(bytes: &[u8], encoding: &str, errors: &str) -> Result<Value> {
    // Normalise encoding name (strip hyphens/underscores, lowercase).
    let enc_norm: String = encoding
        .to_ascii_lowercase()
        .chars()
        .filter(|&c| c != '-' && c != '_')
        .collect();

    match enc_norm.as_str() {
        "utf8" => {
            // Fast path: if all bytes are valid UTF-8 the error handler is never
            // invoked, so we must not validate its name (CPython is lazy here).
            match std::str::from_utf8(bytes) {
                Ok(s) => Ok(Value::string(s)),
                Err(e) => {
                    // Decoding failed — now the error handler matters.
                    match errors {
                        "strict" => {
                            let start = e.valid_up_to();
                            let end = start + e.error_len().unwrap_or(bytes.len() - start);
                            let reason = if e.error_len().is_none() {
                                "unexpected end of data"
                            } else {
                                let b = bytes[start];
                                // CPython 3.12 reports "invalid continuation byte" when the
                                // byte at `start` is a valid multi-byte sequence start
                                // (0xC2..=0xF4) but the bytes that follow are not valid
                                // continuation bytes.  All other cases (lone continuation
                                // bytes 0x80..=0xBF, overlong-sequence starts 0xC0..=0xC1,
                                // and out-of-range starts 0xF5..=0xFF) are "invalid start
                                // byte" because the byte itself cannot begin a legal UTF-8
                                // sequence.
                                if matches!(b, 0xC2..=0xF4) {
                                    "invalid continuation byte"
                                } else {
                                    "invalid start byte"
                                }
                            };
                            Err(PyError::UnicodeDecodeError {
                                encoding: "utf-8".to_string(),
                                object: bytes.to_vec(),
                                start,
                                end,
                                reason: reason.to_string(),
                            })
                        }
                        "ignore" => Ok(Value::string(&bytes_decode_utf8_ignore(bytes))),
                        "replace" => Ok(Value::string(String::from_utf8_lossy(bytes).as_ref())),
                        _ => Err(PyError::named(
                            "LookupError",
                            format!("unknown error handler name '{errors}'"),
                        )),
                    }
                }
            }
        }
        "latin1" | "iso88591" | "iso8859" => {
            // Latin-1: byte N maps directly to Unicode code point N.
            // This encoding never fails, so the error handler is never invoked.
            let s: String = bytes.iter().map(|&b| b as char).collect();
            Ok(Value::string(&s))
        }
        "ascii" => {
            // Find the first non-ASCII byte, if any.
            let first_bad = bytes.iter().enumerate().find(|&(_, &b)| b > 0x7F);
            match first_bad {
                None => {
                    // All bytes are valid ASCII; error handler is never invoked.
                    // SAFETY: all bytes validated as ASCII (≤ 0x7F, valid UTF-8).
                    Ok(Value::string(unsafe {
                        std::str::from_utf8_unchecked(bytes)
                    }))
                }
                Some((i, _b)) => {
                    // At least one bad byte — now the error handler matters.
                    match errors {
                        "strict" => Err(PyError::UnicodeDecodeError {
                            encoding: "ascii".to_string(),
                            object: bytes.to_vec(),
                            start: i,
                            end: i + 1,
                            reason: "ordinal not in range(128)".to_string(),
                        }),
                        "ignore" => {
                            let s: String = bytes
                                .iter()
                                .filter(|&&b| b <= 0x7F)
                                .map(|&b| b as char)
                                .collect();
                            Ok(Value::string(&s))
                        }
                        "replace" => {
                            let s: String = bytes
                                .iter()
                                .map(|&b| if b <= 0x7F { b as char } else { '\u{FFFD}' })
                                .collect();
                            Ok(Value::string(&s))
                        }
                        _ => Err(PyError::named(
                            "LookupError",
                            format!("unknown error handler name '{errors}'"),
                        )),
                    }
                }
            }
        }
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {encoding}"),
        )),
    }
}

/// Decode UTF-8 bytes, skipping any invalid byte sequences (errors='ignore').
fn bytes_decode_utf8_ignore(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match std::str::from_utf8(&bytes[i..]) {
            Ok(s) => {
                out.push_str(s);
                break;
            }
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                // SAFETY: validated by from_utf8.
                out.push_str(unsafe { std::str::from_utf8_unchecked(&bytes[i..i + valid_up_to]) });
                // Skip the invalid byte(s): error_len() is None for truncated sequence.
                let skip = e.error_len().unwrap_or(1);
                i += valid_up_to + skip;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// startswith / endswith
// ---------------------------------------------------------------------------

/// Like `extract_bytes_arg` but also accepts an integer byte value (0..=255),
/// converting it to a one-byte owned `Vec<u8>` needle.  CPython's `find` and
/// `count` accept integer sub-sequences via this path.  Returns `Err(ValueError)`
/// when the integer is outside `0..=255`.
fn extract_bytes_or_int_arg(arg: &Value) -> Result<std::borrow::Cow<'_, [u8]>> {
    match arg.kind() {
        ValueKind::Bytes(rc) => Ok(std::borrow::Cow::Borrowed(rc.as_slice())),
        ValueKind::Int(n) => {
            let b = u8::try_from(n).map_err(|_| {
                PyError::named("ValueError", "byte must be in range(0, 256)".to_string())
            })?;
            Ok(std::borrow::Cow::Owned(vec![b]))
        }
        ValueKind::Bool(b) => Ok(std::borrow::Cow::Owned(vec![b as u8])),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "argument should be integer or bytes-like object, not '{}'",
                pyrust_core::builtin_type_name(arg),
            ),
        )),
    }
}

fn bytes_startswith(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let prefix_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.startswith() requires at least 1 argument".to_string(),
        )
    })?;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        return Ok(Value::bool_(false));
    };
    let haystack = &bytes[start..end];
    match prefix_val.kind() {
        ValueKind::Bytes(rc) => Ok(Value::bool_(haystack.starts_with(rc.as_slice()))),
        ValueKind::Tuple(prefixes) => {
            for pv in prefixes.iter() {
                match pv.kind() {
                    ValueKind::Bytes(rc) if haystack.starts_with(rc.as_slice()) => {
                        return Ok(Value::bool_(true));
                    }
                    ValueKind::Bytes(_) => {}
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "a bytes-like object is required, not a non-bytes item in the tuple"
                                .to_string(),
                        ));
                    }
                }
            }
            Ok(Value::bool_(false))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "startswith first arg must be bytes or a tuple of bytes, not {}",
                pyrust_core::builtin_type_name(prefix_val)
            ),
        )),
    }
}

fn bytes_endswith(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let suffix_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.endswith() requires at least 1 argument".to_string(),
        )
    })?;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        return Ok(Value::bool_(false));
    };
    let haystack = &bytes[start..end];
    match suffix_val.kind() {
        ValueKind::Bytes(rc) => Ok(Value::bool_(haystack.ends_with(rc.as_slice()))),
        ValueKind::Tuple(suffixes) => {
            for sv in suffixes.iter() {
                match sv.kind() {
                    ValueKind::Bytes(rc) if haystack.ends_with(rc.as_slice()) => {
                        return Ok(Value::bool_(true));
                    }
                    ValueKind::Bytes(_) => {}
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "a bytes-like object is required, not a non-bytes item in the tuple"
                                .to_string(),
                        ));
                    }
                }
            }
            Ok(Value::bool_(false))
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "endswith first arg must be bytes or a tuple of bytes, not {}",
                pyrust_core::builtin_type_name(suffix_val)
            ),
        )),
    }
}

// ---------------------------------------------------------------------------
// find
// ---------------------------------------------------------------------------

fn bytes_find(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.find() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython returns -1 even for empty sub.
        return Ok(Value::int(-1));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: empty sub in a valid (possibly empty) range returns start.
        return Ok(Value::int(start as i64));
    }

    match find_subsequence(haystack, sub) {
        Some(pos) => Ok(Value::int((start + pos) as i64)),
        None => Ok(Value::int(-1)),
    }
}

// ---------------------------------------------------------------------------
// rfind
// ---------------------------------------------------------------------------

fn bytes_rfind(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.rfind() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython returns -1 even for empty sub.
        return Ok(Value::int(-1));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: empty sub rfind in a valid range returns end offset.
        return Ok(Value::int(end as i64));
    }

    match rfind_subsequence(haystack, sub) {
        Some(pos) => Ok(Value::int((start + pos) as i64)),
        None => Ok(Value::int(-1)),
    }
}

// ---------------------------------------------------------------------------
// index
// ---------------------------------------------------------------------------

fn bytes_index(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.index() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython raises ValueError.
        return Err(PyError::named(
            "ValueError",
            "subsection not found".to_string(),
        ));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: empty sub in a valid (possibly empty) range returns start.
        return Ok(Value::int(start as i64));
    }

    match find_subsequence(haystack, sub) {
        Some(pos) => Ok(Value::int((start + pos) as i64)),
        None => Err(PyError::named(
            "ValueError",
            "subsection not found".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// rindex
// ---------------------------------------------------------------------------

fn bytes_rindex(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.rindex() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython raises ValueError.
        return Err(PyError::named(
            "ValueError",
            "subsection not found".to_string(),
        ));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: empty sub rindex in a valid range returns end offset.
        return Ok(Value::int(end as i64));
    }

    match rfind_subsequence(haystack, sub) {
        Some(pos) => Ok(Value::int((start + pos) as i64)),
        None => Err(PyError::named(
            "ValueError",
            "subsection not found".to_string(),
        )),
    }
}

// ---------------------------------------------------------------------------
// count
// ---------------------------------------------------------------------------

fn bytes_count(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let sub_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "bytes.count() requires at least 1 argument".to_string(),
        )
    })?;
    let sub_cow = extract_bytes_or_int_arg(sub_val)?;
    let sub: &[u8] = &sub_cow;
    let range = bytes_slice_args(bytes.len(), args)?;
    let Some((start, end)) = range else {
        // Inverted range: CPython returns 0 even for empty sub.
        return Ok(Value::int(0));
    };
    let haystack = &bytes[start..end];

    if sub.is_empty() {
        // CPython: count of empty sub = len(haystack) + 1.
        return Ok(Value::int((haystack.len() as i64) + 1));
    }

    let mut count: i64 = 0;
    let mut i = 0;
    while i + sub.len() <= haystack.len() {
        if &haystack[i..i + sub.len()] == sub {
            count += 1;
            i += sub.len();
        } else {
            i += 1;
        }
    }
    Ok(Value::int(count))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse start/end slice args (args[1], args[2]) into byte offsets.
///
/// `end` is clamped to `[0, len]`.  `start` is clamped to `[0, len]` only
/// when it is within bounds; a positive `start > len` is left unclamped so
/// that the `end < start` guard below returns `None`, matching CPython's
/// behaviour of returning -1 / ValueError for an out-of-bounds start.
///
/// Returns `Ok(Some((start, end)))` for a valid (possibly empty) range, or
/// `Ok(None)` when `end < start` — an inverted or out-of-bounds-start range
/// that CPython treats as "no match" (find → -1, count → 0,
/// startswith/endswith → False, index/rindex → ValueError).
fn bytes_slice_args(len: usize, args: &[Value]) -> Result<Option<(usize, usize)>> {
    let start: usize = match args.get(1).map(|v| v.kind()) {
        None | Some(ValueKind::None) => 0,
        Some(ValueKind::Int(i)) => normalise_idx(i, len),
        Some(ValueKind::Bool(b)) => normalise_idx(b as i64, len),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or None or have an __index__ method".to_string(),
            ));
        }
    };
    let end: usize = match args.get(2).map(|v| v.kind()) {
        None | Some(ValueKind::None) => len,
        Some(ValueKind::Int(i)) => normalise_idx(i, len).min(len),
        Some(ValueKind::Bool(b)) => normalise_idx(b as i64, len).min(len),
        _ => {
            return Err(PyError::named(
                "TypeError",
                "slice indices must be integers or None or have an __index__ method".to_string(),
            ));
        }
    };
    // Do NOT clamp start up to len here.  When the caller passes a positive
    // start > len, leaving it unclamped means end (which IS clamped to len)
    // satisfies end < start, triggering the None path below — the same
    // result CPython returns for an out-of-bounds start.  Clamping start to
    // len would incorrectly turn start=6 into start=5==end and produce a
    // valid empty range (returning 5 for empty-sub rfind instead of -1).
    if end < start {
        Ok(None)
    } else {
        Ok(Some((start.min(len), end)))
    }
}

fn normalise_idx(idx: i64, len: usize) -> usize {
    if idx < 0 {
        let from_end = (-idx) as usize;
        len.saturating_sub(from_end)
    } else {
        idx as usize
    }
}

/// Naive substring search: find first occurrence of `sub` in `haystack`,
/// returning the starting byte index, or `None` if not found.
fn find_subsequence(haystack: &[u8], sub: &[u8]) -> Option<usize> {
    if sub.len() > haystack.len() {
        return None;
    }
    haystack.windows(sub.len()).position(|w| w == sub)
}

/// Naive substring search: find last occurrence of `sub` in `haystack`,
/// returning the starting byte index, or `None` if not found.
fn rfind_subsequence(haystack: &[u8], sub: &[u8]) -> Option<usize> {
    if sub.len() > haystack.len() {
        return None;
    }
    haystack.windows(sub.len()).rposition(|w| w == sub)
}

// ---------------------------------------------------------------------------
// replace
// ---------------------------------------------------------------------------

fn bytes_replace(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.len() < 2 {
        return Err(PyError::named(
            "TypeError",
            "bytes.replace() requires at least 2 arguments".to_string(),
        ));
    }
    let old: &[u8] = match args[0].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[0])
                ),
            ));
        }
    };
    let new: &[u8] = match args[1].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[1])
                ),
            ));
        }
    };
    let count: i64 = match args.get(2).map(|v| v.kind()) {
        None => -1,
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.replace() count must be an integer".to_string(),
            ));
        }
    };

    if count == 0 {
        return Ok(Value::bytes(bytes.to_vec()));
    }

    let max_replacements: usize = if count < 0 {
        usize::MAX
    } else {
        count as usize
    };

    if old.is_empty() {
        // CPython inserts `new` between every byte and at start/end when old is empty.
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len() + new.len() * (bytes.len() + 1));
        let mut replacements = 0usize;
        if replacements < max_replacements {
            out.extend_from_slice(new);
            replacements += 1;
        }
        for &b in bytes {
            out.push(b);
            if replacements < max_replacements {
                out.extend_from_slice(new);
                replacements += 1;
            }
        }
        return Ok(Value::bytes(out));
    }

    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut replacements = 0usize;
    while i + old.len() <= bytes.len() {
        if replacements >= max_replacements {
            break;
        }
        if bytes[i..].starts_with(old) {
            out.extend_from_slice(new);
            i += old.len();
            replacements += 1;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    // Append the remaining bytes.
    out.extend_from_slice(&bytes[i..]);
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// strip / lstrip / rstrip
// ---------------------------------------------------------------------------

/// ASCII whitespace bytes: space, tab, newline, carriage return, vertical tab, form feed.
fn is_ascii_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn bytes_strip(bytes: &[u8], args: &[Value], left: bool, right: bool) -> Result<Value> {
    let chars_arg: Option<&[u8]> = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Bytes(rc)) => Some(rc.as_slice()),
        Some(ValueKind::None) | None => None,
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                "strip argument must be a bytes or None".to_string(),
            ));
        }
    };
    let mut start = 0;
    let mut end = bytes.len();
    match chars_arg {
        None => {
            if left {
                while start < end && is_ascii_whitespace(bytes[start]) {
                    start += 1;
                }
            }
            if right {
                while end > start && is_ascii_whitespace(bytes[end - 1]) {
                    end -= 1;
                }
            }
        }
        Some(chars) => {
            if left {
                while start < end && chars.contains(&bytes[start]) {
                    start += 1;
                }
            }
            if right {
                while end > start && chars.contains(&bytes[end - 1]) {
                    end -= 1;
                }
            }
        }
    }
    Ok(Value::bytes(bytes[start..end].to_vec()))
}

// ---------------------------------------------------------------------------
// removeprefix / removesuffix
// ---------------------------------------------------------------------------

fn bytes_removeprefix(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "bytes.removeprefix() takes exactly one argument (0 given)".to_string(),
        ));
    }
    let prefix: &[u8] = match args[0].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[0])
                ),
            ));
        }
    };
    if bytes.starts_with(prefix) {
        Ok(Value::bytes(bytes[prefix.len()..].to_vec()))
    } else {
        Ok(Value::bytes(bytes.to_vec()))
    }
}

fn bytes_removesuffix(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "bytes.removesuffix() takes exactly one argument (0 given)".to_string(),
        ));
    }
    let suffix: &[u8] = match args[0].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[0])
                ),
            ));
        }
    };
    if bytes.ends_with(suffix) {
        Ok(Value::bytes(bytes[..bytes.len() - suffix.len()].to_vec()))
    } else {
        Ok(Value::bytes(bytes.to_vec()))
    }
}

// ---------------------------------------------------------------------------
// split / rsplit
// ---------------------------------------------------------------------------

fn bytes_split(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let (sep_opt, maxsplit) = bytes_split_args(args)?;
    let parts = match sep_opt {
        None => bytes_split_whitespace(bytes, maxsplit, false),
        Some(sep) => {
            if sep.is_empty() {
                return Err(PyError::named("ValueError", "empty separator".to_string()));
            }
            bytes_split_by_sep(bytes, sep, maxsplit, false)
        }
    };
    Ok(Value::list(
        parts
            .into_iter()
            .map(|b| Value::bytes(b.to_vec()))
            .collect(),
    ))
}

fn bytes_rsplit(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let (sep_opt, maxsplit) = bytes_split_args(args)?;
    let parts = match sep_opt {
        None => bytes_split_whitespace(bytes, maxsplit, true),
        Some(sep) => {
            if sep.is_empty() {
                return Err(PyError::named("ValueError", "empty separator".to_string()));
            }
            bytes_split_by_sep(bytes, sep, maxsplit, true)
        }
    };
    Ok(Value::list(
        parts
            .into_iter()
            .map(|b| Value::bytes(b.to_vec()))
            .collect(),
    ))
}

/// Parse (sep, maxsplit) from bytes split/rsplit args.
fn bytes_split_args(args: &[Value]) -> Result<(Option<&[u8]>, i64)> {
    let sep = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Bytes(rc)) => Some(rc.as_slice()),
        Some(ValueKind::None) | None => None,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.split() argument 1 must be a bytes-like object or None".to_string(),
            ));
        }
    };
    let maxsplit: i64 = match args.get(1).map(|v| v.kind()) {
        None => -1,
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.split() maxsplit must be an integer".to_string(),
            ));
        }
    };
    Ok((sep, maxsplit))
}

/// Split on ASCII whitespace (consecutive whitespace is a single separator).
/// Mirrors CPython's `bytes.split()` with no sep argument.
fn bytes_split_whitespace<'a>(bytes: &'a [u8], maxsplit: i64, reverse: bool) -> Vec<&'a [u8]> {
    if reverse {
        let max = if maxsplit < 0 {
            usize::MAX
        } else {
            maxsplit as usize
        };
        let mut parts: Vec<&'a [u8]> = Vec::new();
        let mut end = bytes.len();
        let mut splits = 0;
        // Trim trailing whitespace.
        while end > 0 && is_ascii_whitespace(bytes[end - 1]) {
            end -= 1;
        }
        while end > 0 && splits < max {
            // Scan backwards to find start of the token.
            let token_end = end;
            let mut start = token_end;
            while start > 0 && !is_ascii_whitespace(bytes[start - 1]) {
                start -= 1;
            }
            parts.push(&bytes[start..token_end]);
            splits += 1;
            // Skip whitespace.
            end = start;
            while end > 0 && is_ascii_whitespace(bytes[end - 1]) {
                end -= 1;
            }
        }
        // Any remaining bytes before end go into the last (leftmost) chunk.
        if end > 0 {
            parts.push(&bytes[..end]);
        }
        parts.reverse();
        parts
    } else {
        let max = if maxsplit < 0 {
            usize::MAX
        } else {
            maxsplit as usize
        };
        let mut parts: Vec<&'a [u8]> = Vec::new();
        let mut i = 0;
        let len = bytes.len();
        // Skip leading whitespace.
        while i < len && is_ascii_whitespace(bytes[i]) {
            i += 1;
        }
        let mut splits = 0;
        while i < len {
            if splits >= max {
                // Remaining bytes as the last element.
                parts.push(&bytes[i..]);
                break;
            }
            // Collect non-whitespace token.
            let start = i;
            while i < len && !is_ascii_whitespace(bytes[i]) {
                i += 1;
            }
            parts.push(&bytes[start..i]);
            splits += 1;
            // Skip whitespace.
            while i < len && is_ascii_whitespace(bytes[i]) {
                i += 1;
            }
        }
        parts
    }
}

/// Split by a literal separator.
fn bytes_split_by_sep<'a>(
    bytes: &'a [u8],
    sep: &[u8],
    maxsplit: i64,
    reverse: bool,
) -> Vec<&'a [u8]> {
    let max = if maxsplit < 0 {
        usize::MAX
    } else {
        maxsplit as usize
    };
    if reverse {
        let mut parts: Vec<&'a [u8]> = Vec::new();
        let mut end = bytes.len();
        let mut splits = 0;
        while splits < max {
            match rfind_subsequence(&bytes[..end], sep) {
                Some(pos) => {
                    parts.push(&bytes[pos + sep.len()..end]);
                    end = pos;
                    splits += 1;
                }
                None => break,
            }
        }
        parts.push(&bytes[..end]);
        parts.reverse();
        parts
    } else {
        let mut parts: Vec<&'a [u8]> = Vec::new();
        let mut start = 0;
        let mut splits = 0;
        while splits < max {
            match find_subsequence(&bytes[start..], sep) {
                Some(pos) => {
                    parts.push(&bytes[start..start + pos]);
                    start += pos + sep.len();
                    splits += 1;
                }
                None => break,
            }
        }
        parts.push(&bytes[start..]);
        parts
    }
}

// ---------------------------------------------------------------------------
// splitlines
// ---------------------------------------------------------------------------

fn bytes_splitlines(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let keepends = match args.first().map(|v| v.kind()) {
        None => false,
        Some(ValueKind::Bool(b)) => b,
        Some(ValueKind::Int(n)) => n != 0,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "bytes.splitlines() keepends must be bool or int".to_string(),
            ));
        }
    };
    // bytes.splitlines() only recognises \r\n, \r, \n.
    let len = bytes.len();
    let mut lines: Vec<Value> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < len {
        let eol_len = match bytes[i] {
            b'\n' => 1,
            b'\r' => {
                if i + 1 < len && bytes[i + 1] == b'\n' {
                    2
                } else {
                    1
                }
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let end = if keepends { i + eol_len } else { i };
        lines.push(Value::bytes(bytes[start..end].to_vec()));
        i += eol_len;
        start = i;
    }
    // Trailing non-empty segment (no trailing newline).
    if start < len {
        lines.push(Value::bytes(bytes[start..].to_vec()));
    }
    Ok(Value::list(lines))
}

// ---------------------------------------------------------------------------
// join
// ---------------------------------------------------------------------------

fn bytes_join(sep: &[u8], args: &[Value]) -> Result<Value> {
    let iterable = args.first().ok_or_else(|| {
        PyError::named("TypeError", "bytes.join() requires 1 argument".to_string())
    })?;

    /// Extract byte items from a slice of Values, all must be bytes-like.
    fn collect_items(vals: &[Value]) -> Result<Vec<Vec<u8>>> {
        vals.iter()
            .enumerate()
            .map(|(i, v)| match v.kind() {
                ValueKind::Bytes(rc) => Ok(rc.as_slice().to_vec()),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "sequence item {i}: expected a bytes-like object, {} found",
                        pyrust_core::builtin_type_name(v),
                    ),
                )),
            })
            .collect()
    }

    let items: Vec<Vec<u8>> = match iterable.kind() {
        ValueKind::List(list_items) => collect_items(&list_items)?,
        ValueKind::Tuple(tuple_items) => collect_items(&tuple_items)?,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "can only join an iterable of bytes-like objects".to_string(),
            ));
        }
    };

    if items.is_empty() {
        return Ok(Value::bytes(vec![]));
    }
    let total_len =
        items.iter().map(|b| b.len()).sum::<usize>() + sep.len() * (items.len().saturating_sub(1));
    let mut out: Vec<u8> = Vec::with_capacity(total_len);
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(sep);
        }
        out.extend_from_slice(item);
    }
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// title / capitalize
// ---------------------------------------------------------------------------

fn bytes_title(bytes: &[u8]) -> Vec<u8> {
    // A byte is a word character if it is ASCII alphabetic.
    // Non-alphabetic bytes act as word separators.
    let mut out = Vec::with_capacity(bytes.len());
    let mut prev_was_alpha = false;
    for &b in bytes {
        if b.is_ascii_alphabetic() {
            if prev_was_alpha {
                out.push(b.to_ascii_lowercase());
            } else {
                out.push(b.to_ascii_uppercase());
            }
            prev_was_alpha = true;
        } else {
            out.push(b);
            prev_was_alpha = false;
        }
    }
    out
}

fn bytes_capitalize(bytes: &[u8]) -> Vec<u8> {
    // First byte upper-cased (if alphabetic), rest lower-cased.
    let mut out: Vec<u8> = bytes.iter().map(|b| b.to_ascii_lowercase()).collect();
    if let Some(first) = out.first_mut() {
        *first = first.to_ascii_uppercase();
    }
    out
}

// ---------------------------------------------------------------------------
// isupper / islower
// ---------------------------------------------------------------------------

fn bytes_isupper(bytes: &[u8]) -> bool {
    // Must have at least one uppercase letter and no lowercase letters.
    let has_upper = bytes.iter().any(|b| b.is_ascii_uppercase());
    let has_lower = bytes.iter().any(|b| b.is_ascii_lowercase());
    has_upper && !has_lower
}

fn bytes_islower(bytes: &[u8]) -> bool {
    // Must have at least one lowercase letter and no uppercase letters.
    let has_lower = bytes.iter().any(|b| b.is_ascii_lowercase());
    let has_upper = bytes.iter().any(|b| b.is_ascii_uppercase());
    has_lower && !has_upper
}

// ---------------------------------------------------------------------------
// center / ljust / rjust
// ---------------------------------------------------------------------------

fn extract_fill_byte(args: &[Value], arg_idx: usize, method: &str) -> Result<u8> {
    match args.get(arg_idx).map(|v| v.kind()) {
        None => Ok(b' '),
        Some(ValueKind::Bytes(rc)) => {
            let s = rc.as_slice();
            if s.len() == 1 {
                Ok(s[0])
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "{method} fillchar must be a byte string of length 1, not {}",
                        s.len()
                    ),
                ))
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("{method} fillchar must be a bytes object of length 1"),
        )),
    }
}

fn extract_width(args: &[Value], method: &str) -> Result<i64> {
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => Ok(n),
        Some(ValueKind::Bool(b)) => Ok(b as i64),
        _ => Err(PyError::named(
            "TypeError",
            format!("{method} width must be an integer"),
        )),
    }
}

fn bytes_center(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let width = extract_width(args, "center")?;
    let fill = extract_fill_byte(args, 1, "center")?;
    let len = bytes.len();
    if width <= len as i64 {
        return Ok(Value::bytes(bytes.to_vec()));
    }
    let width = width as usize;
    let marg = width - len;
    // CPython formula: left = marg/2 + (marg & width & 1)
    let left = marg / 2 + (marg & width & 1);
    let right = marg - left;
    let mut out = Vec::with_capacity(width);
    out.extend(std::iter::repeat(fill).take(left));
    out.extend_from_slice(bytes);
    out.extend(std::iter::repeat(fill).take(right));
    Ok(Value::bytes(out))
}

fn bytes_ljust(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let width = extract_width(args, "ljust")?;
    let fill = extract_fill_byte(args, 1, "ljust")?;
    let len = bytes.len();
    if width <= len as i64 {
        return Ok(Value::bytes(bytes.to_vec()));
    }
    let width = width as usize;
    let mut out = Vec::with_capacity(width);
    out.extend_from_slice(bytes);
    out.extend(std::iter::repeat(fill).take(width - len));
    Ok(Value::bytes(out))
}

fn bytes_rjust(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let width = extract_width(args, "rjust")?;
    let fill = extract_fill_byte(args, 1, "rjust")?;
    let len = bytes.len();
    if width <= len as i64 {
        return Ok(Value::bytes(bytes.to_vec()));
    }
    let width = width as usize;
    let mut out = Vec::with_capacity(width);
    out.extend(std::iter::repeat(fill).take(width - len));
    out.extend_from_slice(bytes);
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// zfill
// ---------------------------------------------------------------------------

fn bytes_zfill(bytes: &[u8], args: &[Value]) -> Result<Value> {
    let width: i64 = match args.first().map(|v| v.kind()) {
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                "zfill width must be an integer".to_string(),
            ));
        }
    };
    let len = bytes.len();
    if width <= len as i64 {
        return Ok(Value::bytes(bytes.to_vec()));
    }
    let width = width as usize;
    let pad = width - len;
    let mut out = Vec::with_capacity(width);
    // If the first byte is '+' or '-', keep it at the front and pad after it.
    if len > 0 && (bytes[0] == b'+' || bytes[0] == b'-') {
        out.push(bytes[0]);
        out.extend(std::iter::repeat(b'0').take(pad));
        out.extend_from_slice(&bytes[1..]);
    } else {
        out.extend(std::iter::repeat(b'0').take(pad));
        out.extend_from_slice(bytes);
    }
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// translate
// ---------------------------------------------------------------------------

fn bytes_translate(bytes: &[u8], args: &[Value], kwargs: &IndexMap<PyKey, Value>) -> Result<Value> {
    // CPython signature: bytes.translate(table, /, delete=b'')
    // table may be None or a 256-byte mapping (bytes).
    // When table is None: just delete bytes in the delete set.
    // When table is provided: map each byte through the table, then delete.

    // CPython counts all arguments (positional + keyword) and raises if the total
    // exceeds 2 (table + delete).  This fires before the duplicate-delete or
    // unknown-keyword checks.
    let total_args = args.len() + kwargs.len();
    if total_args > 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "translate() takes at most 2 arguments ({} given)",
                total_args
            ),
        ));
    }

    // Reject unrecognised keyword arguments.
    for key in kwargs.keys() {
        if let PyKey::Str(s) = key {
            let name = s.as_str().unwrap_or("");
            if name != "delete" {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{name}' is an invalid keyword argument for translate()"),
                ));
            }
        }
    }

    let table_val = args.first().ok_or_else(|| {
        PyError::named(
            "TypeError",
            "translate() takes at least 1 positional argument (0 given)".to_string(),
        )
    })?;
    let table: Option<&[u8]> = match table_val.kind() {
        ValueKind::None => None,
        ValueKind::Bytes(rc) => {
            let s = rc.as_slice();
            if s.len() != 256 {
                return Err(PyError::named(
                    "ValueError",
                    "translation table must be 256 characters long".to_string(),
                ));
            }
            Some(s)
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(table_val)
                ),
            ));
        }
    };

    // `delete` may come from args[1] (positional) or from the `delete=` keyword argument.
    // The total-args guard above ensures they can't both be set simultaneously.
    let kw_delete = kwargs.get(&StrKey("delete"));
    let delete_val = args.get(1).or(kw_delete);
    let delete: &[u8] = match delete_val.map(|v| v.kind()) {
        None => &[],
        Some(ValueKind::Bytes(rc)) => rc.as_slice(),
        Some(_) => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(delete_val.unwrap())
                ),
            ));
        }
    };

    // Build a 256-entry boolean table so each delete-membership check is O(1)
    // instead of O(|delete|), making the overall loop O(n) rather than O(n × |delete|).
    let mut delete_table = [false; 256];
    for &b in delete {
        delete_table[b as usize] = true;
    }

    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    for &b in bytes {
        if delete_table[b as usize] {
            continue;
        }
        let mapped = match table {
            None => b,
            Some(t) => t[b as usize],
        };
        out.push(mapped);
    }
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// partition / rpartition
// ---------------------------------------------------------------------------

fn bytes_partition(bytes: &[u8], args: &[Value], reverse: bool) -> Result<Value> {
    let sep_val = args.first().ok_or_else(|| {
        let name = if reverse { "rpartition" } else { "partition" };
        PyError::named(
            "TypeError",
            format!("bytes.{name}() requires exactly 1 argument"),
        )
    })?;
    let sep: &[u8] = match sep_val.kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(sep_val)
                ),
            ));
        }
    };
    if sep.is_empty() {
        return Err(PyError::named("ValueError", "empty separator".to_string()));
    }
    let found = if reverse {
        rfind_subsequence(bytes, sep)
    } else {
        find_subsequence(bytes, sep)
    };
    let parts = match found {
        Some(pos) => {
            let before = Value::bytes(bytes[..pos].to_vec());
            let mid = Value::bytes(sep.to_vec());
            let after = Value::bytes(bytes[pos + sep.len()..].to_vec());
            vec![before, mid, after]
        }
        None => {
            if reverse {
                // rpartition not found: (b'', b'', original)
                vec![
                    Value::bytes(vec![]),
                    Value::bytes(vec![]),
                    Value::bytes(bytes.to_vec()),
                ]
            } else {
                // partition not found: (original, b'', b'')
                vec![
                    Value::bytes(bytes.to_vec()),
                    Value::bytes(vec![]),
                    Value::bytes(vec![]),
                ]
            }
        }
    };
    Ok(Value::tuple(parts))
}

// ---------------------------------------------------------------------------
// istitle
// ---------------------------------------------------------------------------

fn bytes_istitle(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    // A byte sequence is titlecased if each word (sequence of alpha bytes) starts
    // with an uppercase letter and the rest are lowercase.  Non-alpha bytes act as
    // word separators.
    let mut prev_was_alpha = false;
    let mut has_alpha = false;
    for &b in bytes {
        if b.is_ascii_alphabetic() {
            has_alpha = true;
            if prev_was_alpha {
                // Continuation of a word — must be lowercase.
                if b.is_ascii_uppercase() {
                    return false;
                }
            } else {
                // Start of a new word — must be uppercase.
                if b.is_ascii_lowercase() {
                    return false;
                }
            }
            prev_was_alpha = true;
        } else {
            prev_was_alpha = false;
        }
    }
    has_alpha
}

// ---------------------------------------------------------------------------
// expandtabs
// ---------------------------------------------------------------------------

/// `bytes.expandtabs(tabsize=8)`
///
/// Return a copy where tab characters (`\t`, 0x09) are replaced by spaces up
/// to the next tab stop (multiples of `tabsize`).  `\n` and `\r` reset the
/// column counter.  Non-ASCII bytes are treated as single-column characters
/// (matching CPython's behaviour: bytes are opaque octets, not Unicode).
fn bytes_expandtabs(bytes: &[u8], args: &[Value]) -> Result<Value> {
    if args.len() > 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "expandtabs() takes at most 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let tabsize: i64 = match args.first().map(|v| v.kind()) {
        None => 8,
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "'{}' object cannot be interpreted as an integer",
                    pyrust_core::builtin_type_name(args.first().unwrap())
                ),
            ));
        }
    };
    let tabsize = tabsize.max(0) as usize;
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut col: usize = 0;
    for &b in bytes {
        match b {
            b'\t' => {
                if tabsize > 0 {
                    let spaces = tabsize - (col % tabsize);
                    out.extend(std::iter::repeat(b' ').take(spaces));
                    col += spaces;
                }
                // tabsize == 0: tab is silently removed (col unchanged)
            }
            b'\n' | b'\r' => {
                out.push(b);
                col = 0;
            }
            _ => {
                out.push(b);
                col += 1;
            }
        }
    }
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// fromhex
// ---------------------------------------------------------------------------

/// `bytes.fromhex(string)` — classmethod.
///
/// Decodes a hex string to bytes.  Whitespace between hex pairs is allowed
/// (including leading and trailing whitespace); whitespace within a pair is
/// not.  Raises `ValueError` for invalid hex digits or odd-length non-empty
/// tokens; raises `TypeError` for non-string input.
pub fn bytes_fromhex(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 2);
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c.is_ascii_whitespace() {
            continue;
        }
        // First hex digit of a pair.
        let hi = c.to_digit(16).ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!("non-hexadecimal number found in fromhex() arg at position {i}"),
            )
        })?;
        // Second hex digit of the pair — must follow immediately (no whitespace allowed mid-pair).
        let (j, c2) = chars.next().ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!(
                    "non-hexadecimal number found in fromhex() arg at position {}",
                    i + 1
                ),
            )
        })?;
        let lo = c2.to_digit(16).ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!("non-hexadecimal number found in fromhex() arg at position {j}"),
            )
        })?;
        out.push((hi * 16 + lo) as u8);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// maketrans
// ---------------------------------------------------------------------------

/// `bytes.maketrans(from_bytes, to_bytes)` — static method.
///
/// Builds a 256-byte translation table where each byte value `from_bytes[i]`
/// maps to `to_bytes[i]`; all other byte values map to themselves.
/// Both arguments must be bytes of the same length.
///
/// CPython implements this as `staticmethod`, so it is callable both as
/// `bytes.maketrans(...)` and on an instance (`b''.maketrans(...)`).
pub fn bytes_maketrans(args: &[Value]) -> Result<Value> {
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!("maketrans expected 2 arguments, got {}", args.len()),
        ));
    }
    let from: &[u8] = match args[0].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[0])
                ),
            ));
        }
    };
    let to: &[u8] = match args[1].kind() {
        ValueKind::Bytes(rc) => rc.as_slice(),
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    pyrust_core::builtin_type_name(&args[1])
                ),
            ));
        }
    };
    if from.len() != to.len() {
        return Err(PyError::named(
            "ValueError",
            "maketrans arguments must have same length".to_string(),
        ));
    }
    // Build the identity table then apply the mapping.
    let mut table: Vec<u8> = (0u8..=255).collect();
    for (&f, &t) in from.iter().zip(to.iter()) {
        table[f as usize] = t;
    }
    Ok(Value::bytes(table))
}
