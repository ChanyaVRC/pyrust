use indexmap::IndexMap;
use pyrust_core::{
    PyBigIntSign, PyDict, PyError, PyKey, Result, StrKey, Value, ValueKind, builtin_type_name,
    py_value_display_name,
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
    "__getnewargs__",
];

/// Returns `true` if `method` is the name of a built-in `bytes` method.
/// Used by `hasattr` / `getattr` to validate attribute names without
/// invoking the method.
pub fn has_method(method: &str) -> bool {
    METHODS.contains(&method)
}

pub fn call(method: &str, receiver: &Value, args: &[Value], kwargs: &PyDict) -> Result<Value> {
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
    // __getnewargs__ supports the pickle protocol: it returns a 1-tuple
    // containing the bytes itself, i.e. b'hi'.__getnewargs__() == (b'hi',).
    // Handled here (not in `call_on_slice`) so `bytearray` — which reuses
    // `call_on_slice` but has no `__getnewargs__` in CPython — never reaches it.
    if method == "__getnewargs__" {
        if !args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "bytes.__getnewargs__() takes no arguments ({} given)",
                    args.len()
                ),
            ));
        }
        return Ok(Value::tuple(vec![receiver.clone()]));
    }
    call_on_slice(method, bytes, args, kwargs)
}

/// Dispatch a bytes method on a raw `&[u8]` slice.  Used by `bytearray` to
/// reuse bytes read-method implementations without constructing a temporary
/// `Value::bytes`.  Results that produce new bytes values (upper, lower, etc.)
/// return `Value::bytes`; the bytearray module wraps those into bytearray.
pub fn call_on_slice(method: &str, bytes: &[u8], args: &[Value], kwargs: &PyDict) -> Result<Value> {
    match method {
        "hex" => bytes_hex(bytes, args, kwargs),
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
        "split" => {
            let merged = merge_split_kwargs("split", args, kwargs)?;
            bytes_split(bytes, &merged)
        }
        "rsplit" => {
            let merged = merge_split_kwargs("rsplit", args, kwargs)?;
            bytes_rsplit(bytes, &merged)
        }
        "splitlines" => {
            let merged = merge_single_kwarg("splitlines", "keepends", args, kwargs)?;
            bytes_splitlines(bytes, &merged)
        }
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
        "expandtabs" => {
            let merged = merge_single_kwarg("expandtabs", "tabsize", args, kwargs)?;
            bytes_expandtabs(bytes, &merged)
        }
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

fn bytes_decode(bytes: &[u8], args: &[Value], kwargs: &PyDict) -> Result<Value> {
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
    if !args.is_empty() && kw_encoding.is_some() {
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
/// Supported encodings: `utf-8` (and aliases), `utf-8-sig`, `latin-1` (and
/// aliases), `ascii`, `utf-16` (LE/BE/BOM), `utf-32` (LE/BE/BOM).
/// Supported error handlers: `strict`, `replace`, `ignore`,
/// `backslashreplace`, `surrogateescape`.
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
                Err(_) => decode_utf8_with_errors(bytes, errors, "utf-8"),
            }
        }
        // UTF-8-SIG: strip leading BOM (U+FEFF encoded as EF BB BF) if present.
        "utf8sig" => {
            let payload = if bytes.starts_with(b"\xef\xbb\xbf") {
                &bytes[3..]
            } else {
                bytes
            };
            match std::str::from_utf8(payload) {
                Ok(s) => Ok(Value::string(s)),
                Err(_) => decode_utf8_with_errors(payload, errors, "utf-8-sig"),
            }
        }
        // Latin-1 and its many aliases: byte N → Unicode code point N.
        // This encoding never fails, so the error handler is never invoked.
        "latin1" | "iso88591" | "iso8859" | "l1" | "cp819" | "latin" => {
            let s: String = bytes.iter().map(|&b| b as char).collect();
            Ok(Value::string(&s))
        }
        "ascii" => {
            // Find the first non-ASCII byte, if any.
            let has_bad = bytes.iter().any(|&b| b > 0x7F);
            if !has_bad {
                // All bytes are valid ASCII; error handler is never invoked.
                // SAFETY: all bytes validated as ASCII (≤ 0x7F, valid UTF-8).
                Ok(Value::string(unsafe {
                    std::str::from_utf8_unchecked(bytes)
                }))
            } else {
                decode_ascii_with_errors(bytes, errors)
            }
        }
        // UTF-16 with BOM detection: first two bytes are the BOM (\xff\xfe for LE,
        // \xfe\xff for BE).  If absent, default to little-endian (matches x86/x64/ARM64).
        //
        // `bytes` (the full original slice including the BOM) and the BOM byte count are
        // passed as `original_bytes`/`bom_offset` so that any UnicodeDecodeError carries
        // the full original bytes as `.object` with `start`/`end` adjusted past the BOM —
        // matching CPython's behaviour (see issues #1781, #1813).
        "utf16" => {
            if bytes.starts_with(b"\xff\xfe") {
                decode_utf16_le(&bytes[2..], bytes, 2, errors)
            } else if bytes.starts_with(b"\xfe\xff") {
                decode_utf16_be(&bytes[2..], bytes, 2, errors)
            } else {
                decode_utf16_le(bytes, bytes, 0, errors)
            }
        }
        "utf16le" => decode_utf16_le(bytes, bytes, 0, errors),
        "utf16be" => decode_utf16_be(bytes, bytes, 0, errors),
        // UTF-32 with BOM detection: first four bytes are the BOM.
        "utf32" => {
            if bytes.starts_with(b"\xff\xfe\x00\x00") {
                decode_utf32_le(&bytes[4..], bytes, 4, errors)
            } else if bytes.starts_with(b"\x00\x00\xfe\xff") {
                decode_utf32_be(&bytes[4..], bytes, 4, errors)
            } else {
                decode_utf32_le(bytes, bytes, 0, errors)
            }
        }
        "utf32le" => decode_utf32_le(bytes, bytes, 0, errors),
        "utf32be" => decode_utf32_be(bytes, bytes, 0, errors),
        "cp1252" | "windows1252" => decode_cp1252(bytes, errors),
        "unicodeescape" => decode_unicode_escape(bytes, errors),
        "rawunicodeescape" => decode_raw_unicode_escape(bytes, errors),
        "utf7" => decode_utf7(bytes, errors),
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown encoding: {encoding}"),
        )),
    }
}

/// Build a Python `str` from codepoints that may include lone surrogates,
/// using the CESU-8-aware encoder so `char::from_u32` is never called on a
/// surrogate (which would abort in debug builds).
fn string_from_codepoints(cps: &[u32]) -> Value {
    let mut s = String::new();
    for &cp in cps {
        s.push_str(&pyrust_core::cesu8_encode_codepoint(cp));
    }
    Value::string(s)
}

/// Decode CP1252 (Windows-1252) bytes, honouring the `errors` handler.
/// Undefined bytes (0x81/0x8D/0x8F/0x90/0x9D) raise with CPython's `charmap`
/// reason "character maps to <undefined>".
fn decode_cp1252(bytes: &[u8], errors: &str) -> Result<Value> {
    let mut out = String::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        let b = bytes[idx];
        if let Some(cp) = crate::string::cp1252_decode_codepoint(b) {
            // cp1252 maps only to scalar values, never surrogates.
            out.push(char::from_u32(cp).expect("cp1252 maps to a scalar value"));
            idx += 1;
            continue;
        }
        match errors {
            "ignore" => idx += 1,
            "replace" => {
                out.push('\u{FFFD}');
                idx += 1;
            }
            "strict" => {
                return Err(PyError::UnicodeDecodeError {
                    encoding: "charmap".to_string(),
                    object: bytes.to_vec(),
                    start: idx,
                    end: idx + 1,
                    reason: "character maps to <undefined>".to_string(),
                });
            }
            other => {
                return Err(PyError::named(
                    "LookupError",
                    format!("unknown error handler name '{other}'"),
                ));
            }
        }
    }
    Ok(Value::string(out))
}

/// Parse exactly `n` hex digits starting at `start`.
///
/// Returns `Ok(value)` when `n` valid hex digits are present, otherwise
/// `Err(consumed)` where `consumed` is the count of leading valid hex digits
/// (so callers can report CPython's truncated-escape end position).
fn parse_hex_escape(bytes: &[u8], start: usize, n: usize) -> std::result::Result<u32, usize> {
    let mut v: u32 = 0;
    for k in 0..n {
        match bytes.get(start + k).and_then(|b| (*b as char).to_digit(16)) {
            Some(d) => v = v * 16 + d,
            None => return Err(k),
        }
    }
    Ok(v)
}

/// Decode `raw_unicode_escape` bytes: bytes < 0x100 pass through as Latin-1,
/// `\uHHHH` / `\UHHHHHHHH` are interpreted; backslash is otherwise literal.
fn decode_raw_unicode_escape(bytes: &[u8], errors: &str) -> Result<Value> {
    let mut cps: Vec<u32> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            cps.push(bytes[i] as u32);
            i += 1;
            continue;
        }
        // Count the run of consecutive backslashes; only an odd run followed by
        // 'u'/'U' starts an escape (CPython treats '\\u0041' as literal).
        let bs_start = i;
        let mut j = i;
        while j < bytes.len() && bytes[j] == b'\\' {
            j += 1;
        }
        let bs_count = j - bs_start;
        let next = bytes.get(j).copied();
        let is_escape = bs_count % 2 == 1 && matches!(next, Some(b'u') | Some(b'U'));
        if !is_escape {
            for _ in 0..bs_count {
                cps.push(b'\\' as u32);
            }
            i = j;
            continue;
        }
        // Emit the leading (even) backslashes literally; the last one escapes.
        for _ in 0..(bs_count - 1) {
            cps.push(b'\\' as u32);
        }
        let kind = next.unwrap();
        let digits = if kind == b'u' { 4 } else { 8 };
        let esc_start = j - 1; // position of the escaping backslash
        match parse_hex_escape(bytes, j + 1, digits) {
            Ok(cp) => {
                if cp > 0x10FFFF {
                    return raw_unicode_escape_error(
                        bytes,
                        errors,
                        esc_start,
                        j + 1 + digits,
                        "\\Uxxxxxxxx out of range",
                        &mut cps,
                    );
                }
                cps.push(cp);
                i = j + 1 + digits;
            }
            Err(consumed) => {
                let reason = if kind == b'u' {
                    "truncated \\uXXXX escape"
                } else {
                    "truncated \\UXXXXXXXX escape"
                };
                return raw_unicode_escape_error(
                    bytes,
                    errors,
                    esc_start,
                    j + 1 + consumed,
                    reason,
                    &mut cps,
                );
            }
        }
    }
    Ok(string_from_codepoints(&cps))
}

fn raw_unicode_escape_error(
    bytes: &[u8],
    errors: &str,
    start: usize,
    end: usize,
    reason: &str,
    cps: &mut Vec<u32>,
) -> Result<Value> {
    match errors {
        "strict" => Err(PyError::UnicodeDecodeError {
            encoding: "rawunicodeescape".to_string(),
            object: bytes.to_vec(),
            start,
            end,
            reason: reason.to_string(),
        }),
        "ignore" => {
            let rest = decode_raw_unicode_escape(&bytes[end..], errors)?;
            if let ValueKind::Str(s) = rest.kind() {
                for cp in pyrust_core::cesu8_codepoints(s) {
                    cps.push(cp);
                }
            }
            Ok(string_from_codepoints(cps))
        }
        "replace" => {
            cps.push(0xFFFD);
            let rest = decode_raw_unicode_escape(&bytes[end..], errors)?;
            if let ValueKind::Str(s) = rest.kind() {
                for cp in pyrust_core::cesu8_codepoints(s) {
                    cps.push(cp);
                }
            }
            Ok(string_from_codepoints(cps))
        }
        other => Err(PyError::named(
            "LookupError",
            format!("unknown error handler name '{other}'"),
        )),
    }
}

/// Decode `unicode_escape` bytes: interpret Python string escapes
/// (`\n \t \r \a \b \f \v \0`, octal, `\xHH`, `\uHHHH`, `\UHHHHHHHH`, `\\`,
/// `\'`, `\"`); unknown escapes keep the backslash.
fn decode_unicode_escape(bytes: &[u8], errors: &str) -> Result<Value> {
    let mut cps: Vec<u32> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            cps.push(bytes[i] as u32);
            i += 1;
            continue;
        }
        let esc_start = i;
        match bytes.get(i + 1).copied() {
            None => {
                return unicode_escape_error(
                    bytes,
                    errors,
                    esc_start,
                    esc_start + 1,
                    "\\ at end of string",
                    &mut cps,
                );
            }
            Some(c) => match c {
                b'\n' => i += 2, // line continuation: nothing emitted
                b'\\' => {
                    cps.push(b'\\' as u32);
                    i += 2;
                }
                b'\'' => {
                    cps.push(b'\'' as u32);
                    i += 2;
                }
                b'"' => {
                    cps.push(b'"' as u32);
                    i += 2;
                }
                b'a' => {
                    cps.push(0x07);
                    i += 2;
                }
                b'b' => {
                    cps.push(0x08);
                    i += 2;
                }
                b'f' => {
                    cps.push(0x0C);
                    i += 2;
                }
                b'n' => {
                    cps.push(0x0A);
                    i += 2;
                }
                b'r' => {
                    cps.push(0x0D);
                    i += 2;
                }
                b't' => {
                    cps.push(0x09);
                    i += 2;
                }
                b'v' => {
                    cps.push(0x0B);
                    i += 2;
                }
                b'0'..=b'7' => {
                    // Octal escape: up to 3 digits.
                    let mut val: u32 = 0;
                    let mut k = i + 1;
                    let mut count = 0;
                    while k < bytes.len() && count < 3 && (b'0'..=b'7').contains(&bytes[k]) {
                        val = val * 8 + (bytes[k] - b'0') as u32;
                        k += 1;
                        count += 1;
                    }
                    cps.push(val);
                    i = k;
                }
                b'x' => match parse_hex_escape(bytes, i + 2, 2) {
                    Ok(cp) => {
                        cps.push(cp);
                        i += 4;
                    }
                    Err(consumed) => {
                        return unicode_escape_error(
                            bytes,
                            errors,
                            esc_start,
                            i + 2 + consumed,
                            "truncated \\xXX escape",
                            &mut cps,
                        );
                    }
                },
                b'u' => match parse_hex_escape(bytes, i + 2, 4) {
                    Ok(cp) => {
                        cps.push(cp);
                        i += 6;
                    }
                    Err(consumed) => {
                        return unicode_escape_error(
                            bytes,
                            errors,
                            esc_start,
                            i + 2 + consumed,
                            "truncated \\uXXXX escape",
                            &mut cps,
                        );
                    }
                },
                b'U' => match parse_hex_escape(bytes, i + 2, 8) {
                    Ok(cp) => {
                        if cp > 0x10FFFF {
                            return unicode_escape_error(
                                bytes,
                                errors,
                                esc_start,
                                esc_start + 10,
                                "illegal Unicode character",
                                &mut cps,
                            );
                        }
                        cps.push(cp);
                        i += 10;
                    }
                    Err(consumed) => {
                        return unicode_escape_error(
                            bytes,
                            errors,
                            esc_start,
                            i + 2 + consumed,
                            "truncated \\UXXXXXXXX escape",
                            &mut cps,
                        );
                    }
                },
                b'N' => {
                    // \N{NAME} — named character escape.
                    if bytes.get(i + 2) != Some(&b'{') {
                        return unicode_escape_error(
                            bytes,
                            errors,
                            esc_start,
                            esc_start + 2,
                            "malformed \\N character escape",
                            &mut cps,
                        );
                    }
                    match bytes[i + 3..].iter().position(|&b| b == b'}') {
                        None => {
                            return unicode_escape_error(
                                bytes,
                                errors,
                                esc_start,
                                bytes.len(),
                                "malformed \\N character escape",
                                &mut cps,
                            );
                        }
                        Some(rel) => {
                            let name_end = i + 3 + rel;
                            let name = std::str::from_utf8(&bytes[i + 3..name_end]).ok();
                            match name.and_then(unicode_names2::character) {
                                Some(ch) => {
                                    cps.push(ch as u32);
                                    i = name_end + 1;
                                }
                                None => {
                                    return unicode_escape_error(
                                        bytes,
                                        errors,
                                        esc_start,
                                        name_end + 1,
                                        "unknown Unicode character name",
                                        &mut cps,
                                    );
                                }
                            }
                        }
                    }
                }
                _ => {
                    // Unknown escape: keep the backslash and the char literally.
                    cps.push(b'\\' as u32);
                    cps.push(c as u32);
                    i += 2;
                }
            },
        }
    }
    Ok(string_from_codepoints(&cps))
}

fn unicode_escape_error(
    bytes: &[u8],
    errors: &str,
    start: usize,
    end: usize,
    reason: &str,
    cps: &mut Vec<u32>,
) -> Result<Value> {
    match errors {
        "strict" => Err(PyError::UnicodeDecodeError {
            encoding: "unicodeescape".to_string(),
            object: bytes.to_vec(),
            start,
            end,
            reason: reason.to_string(),
        }),
        "ignore" => {
            let rest = decode_unicode_escape(&bytes[end..], errors)?;
            if let ValueKind::Str(s) = rest.kind() {
                for cp in pyrust_core::cesu8_codepoints(s) {
                    cps.push(cp);
                }
            }
            Ok(string_from_codepoints(cps))
        }
        "replace" => {
            cps.push(0xFFFD);
            let rest = decode_unicode_escape(&bytes[end..], errors)?;
            if let ValueKind::Str(s) = rest.kind() {
                for cp in pyrust_core::cesu8_codepoints(s) {
                    cps.push(cp);
                }
            }
            Ok(string_from_codepoints(cps))
        }
        other => Err(PyError::named(
            "LookupError",
            format!("unknown error handler name '{other}'"),
        )),
    }
}

/// Modified-UTF-7 base64 alphabet value for a byte, or `None` if not a base64
/// character.
fn utf7_base64_value(b: u8) -> Option<u32> {
    match b {
        b'A'..=b'Z' => Some((b - b'A') as u32),
        b'a'..=b'z' => Some((b - b'a' + 26) as u32),
        b'0'..=b'9' => Some((b - b'0' + 52) as u32),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode a run of modified-UTF-7 base64 characters into UTF-16 code units.
///
/// Bits are accumulated 6-per-char and a 16-bit unit is emitted every 16 bits.
/// On a malformed shift sequence returns the CPython error reason:
/// - "partial character in shift sequence" when >= 6 unused bits remain (a whole
///   base64 char's worth that can't complete a unit);
/// - "non-zero padding bits in shift sequence" when the leftover (< 6) padding
///   bits are not all zero.
///
/// Always returns the complete (16-bit) units decoded; `Err(reason)` indicates a
/// malformed tail, but the already-decoded `units` are still returned so the
/// non-strict error handlers can keep them (matching CPython, e.g.
/// `b'+ABC-'.decode('utf-7','replace') == '\x10�'`).
fn utf7_base64_decode(b64: &[u8]) -> (Vec<u16>, Option<&'static str>) {
    let mut acc: u32 = 0;
    let mut nbits: u32 = 0;
    let mut units: Vec<u16> = Vec::new();
    for &c in b64 {
        // Caller only passes valid base64 chars.
        let v = match utf7_base64_value(c) {
            Some(v) => v,
            None => return (units, Some("partial character in shift sequence")),
        };
        acc = (acc << 6) | v;
        nbits += 6;
        if nbits >= 16 {
            nbits -= 16;
            units.push(((acc >> nbits) & 0xFFFF) as u16);
        }
    }
    if nbits >= 6 {
        return (units, Some("partial character in shift sequence"));
    }
    if nbits > 0 && (acc & ((1 << nbits) - 1)) != 0 {
        return (units, Some("non-zero padding bits in shift sequence"));
    }
    (units, None)
}

/// Decode modified UTF-7.  Direct bytes pass through; `+...-` sections are
/// base64-decoded to UTF-16BE code units (`+-` is a literal `+`).
fn decode_utf7(bytes: &[u8], errors: &str) -> Result<Value> {
    let mut cps: Vec<u32> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b != b'+' {
            cps.push(b as u32);
            i += 1;
            continue;
        }
        // `+` begins a shifted section.  `+-` is a literal `+`.
        if bytes.get(i + 1) == Some(&b'-') {
            cps.push('+' as u32);
            i += 2;
            continue;
        }
        let section_start = i;
        i += 1;
        let mut b64: Vec<u8> = Vec::new();
        while i < bytes.len() && utf7_base64_value(bytes[i]).is_some() {
            b64.push(bytes[i]);
            i += 1;
        }
        let (units, err) = utf7_base64_decode(&b64);
        // Combine UTF-16 units (surrogate pairs → scalar; lone surrogate kept).
        utf7_units_to_codepoints(&units, &mut cps);
        if let Some(reason) = err {
            // CPython's error span includes a terminating '-' if present.
            let end = if bytes.get(i) == Some(&b'-') {
                i + 1
            } else {
                i
            };
            match errors {
                "strict" => {
                    return Err(PyError::UnicodeDecodeError {
                        encoding: "utf7".to_string(),
                        object: bytes.to_vec(),
                        start: section_start,
                        end,
                        reason: reason.to_string(),
                    });
                }
                "ignore" => {}
                "replace" => cps.push(0xFFFD),
                other => {
                    return Err(PyError::named(
                        "LookupError",
                        format!("unknown error handler name '{other}'"),
                    ));
                }
            }
        }
        // Consume a terminating `-` if present (explicit shift-out).
        if bytes.get(i) == Some(&b'-') {
            i += 1;
        }
    }
    Ok(string_from_codepoints(&cps))
}

/// Combine decoded UTF-16 units into codepoints, joining valid surrogate pairs
/// and keeping lone surrogates as-is.
fn utf7_units_to_codepoints(units: &[u16], cps: &mut Vec<u32>) {
    let mut k = 0usize;
    while k < units.len() {
        let u = units[k];
        if (0xD800..=0xDBFF).contains(&u) && k + 1 < units.len() {
            let low = units[k + 1];
            if (0xDC00..=0xDFFF).contains(&low) {
                let cp = 0x10000 + ((u as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
                cps.push(cp);
                k += 2;
                continue;
            }
        }
        cps.push(u as u32);
        k += 1;
    }
}

/// Decode UTF-8 bytes that are known to contain at least one invalid sequence,
/// applying the specified error handler.
fn decode_utf8_with_errors(bytes: &[u8], errors: &str, codec_name: &str) -> Result<Value> {
    match errors {
        "strict" => {
            let e = std::str::from_utf8(bytes).unwrap_err();
            let start = e.valid_up_to();
            let end = start + e.error_len().unwrap_or(bytes.len() - start);
            let reason = if e.error_len().is_none() {
                "unexpected end of data"
            } else {
                let b = bytes[start];
                // CPython 3.12 reports "invalid continuation byte" when the
                // byte at `start` is a valid multi-byte sequence start
                // (0xC2..=0xF4) but the bytes that follow are not valid
                // continuation bytes.  All other cases are "invalid start byte".
                if matches!(b, 0xC2..=0xF4) {
                    "invalid continuation byte"
                } else {
                    "invalid start byte"
                }
            };
            Err(PyError::UnicodeDecodeError {
                encoding: codec_name.to_string(),
                object: bytes.to_vec(),
                start,
                end,
                reason: reason.to_string(),
            })
        }
        "ignore" => Ok(Value::string(bytes_decode_utf8_ignore(bytes))),
        "replace" => Ok(Value::string(String::from_utf8_lossy(bytes).as_ref())),
        "backslashreplace" => Ok(Value::string(bytes_decode_utf8_backslashreplace(bytes))),
        "surrogateescape" => Ok(Value::string(bytes_decode_utf8_surrogateescape(bytes))),
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown error handler name '{errors}'"),
        )),
    }
}

/// Decode ASCII bytes that contain at least one byte > 0x7F, applying the
/// specified error handler.
fn decode_ascii_with_errors(bytes: &[u8], errors: &str) -> Result<Value> {
    match errors {
        "strict" => {
            let i = bytes.iter().position(|&b| b > 0x7F).unwrap();
            Err(PyError::UnicodeDecodeError {
                encoding: "ascii".to_string(),
                object: bytes.to_vec(),
                start: i,
                end: i + 1,
                reason: "ordinal not in range(128)".to_string(),
            })
        }
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
        "backslashreplace" => {
            let mut out = String::with_capacity(bytes.len());
            for &b in bytes {
                if b <= 0x7F {
                    out.push(b as char);
                } else {
                    use std::fmt::Write as _;
                    let _ = write!(out, "\\x{:02x}", b);
                }
            }
            Ok(Value::string(&out))
        }
        "surrogateescape" => {
            // Each byte > 0x7F maps to the lone surrogate U+DC80 + byte.
            let mut out = String::with_capacity(bytes.len());
            for &b in bytes {
                if b <= 0x7F {
                    out.push(b as char);
                } else {
                    push_surrogate_escape(&mut out, b);
                }
            }
            Ok(Value::string(&out))
        }
        _ => Err(PyError::named(
            "LookupError",
            format!("unknown error handler name '{errors}'"),
        )),
    }
}

/// Push the CESU-8 encoding of the lone surrogate codepoint U+DC00 | b into
/// `out`.  Pyrust uses CESU-8 to represent surrogate codepoints throughout.
#[inline]
fn push_surrogate_escape(out: &mut String, b: u8) {
    // surrogateescape maps byte b (0x80..=0xFF) to U+DC80..=U+DCFF.
    // DC80 = 0xDC80, so codepoint = 0xDC00 | (b & 0x7F) only for 0x80..=0xFF:
    // U+DC80 + (b - 0x80) = 0xDC80 + b - 0x80 = 0xDC00 + b.
    let cp: u32 = 0xDC00u32 | (b as u32);
    // CESU-8 for a surrogate codepoint (0xD800..=0xDFFF):
    // Safety: we hold &mut String exclusively and push a valid CESU-8 triplet.
    unsafe {
        out.as_mut_vec().extend_from_slice(&[
            0xE0 | (cp >> 12) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]);
    }
}

/// Incrementally decode `bytes` as UTF-8, invoking `on_invalid` once per byte of
/// every invalid sequence. Valid UTF-8 runs pass through unchanged. This is the
/// single shared scaffold behind the `backslashreplace` / `surrogateescape` /
/// `ignore` error handlers, which differ only in `on_invalid`.
fn bytes_decode_utf8_with(bytes: &[u8], mut on_invalid: impl FnMut(&mut String, u8)) -> String {
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
                // SAFETY: `from_utf8` reports `valid_up_to` as the length of the
                // well-formed UTF-8 prefix of `bytes[i..]`, so `bytes[i..i +
                // valid_up_to]` is valid UTF-8 and `from_utf8_unchecked` is sound.
                out.push_str(unsafe { std::str::from_utf8_unchecked(&bytes[i..i + valid_up_to]) });
                // Hand each byte of the invalid run to the handler. `error_len()`
                // is `None` for a truncated trailing sequence, treated as 1 byte.
                let skip = e.error_len().unwrap_or(1);
                for j in 0..skip {
                    on_invalid(&mut out, bytes[i + valid_up_to + j]);
                }
                i += valid_up_to + skip;
            }
        }
    }
    out
}

/// Decode UTF-8 with `backslashreplace`: each invalid byte `b` becomes `\xNN`.
/// Valid UTF-8 bytes pass through unchanged.
fn bytes_decode_utf8_backslashreplace(bytes: &[u8]) -> String {
    bytes_decode_utf8_with(bytes, |out, b| {
        use std::fmt::Write as _;
        let _ = write!(out, "\\x{:02x}", b);
    })
}

/// Decode UTF-8 with `surrogateescape`: each invalid byte `b` becomes the lone
/// surrogate U+DC80 + (b - 0x80) (stored as CESU-8).  Valid UTF-8 passes through.
fn bytes_decode_utf8_surrogateescape(bytes: &[u8]) -> String {
    bytes_decode_utf8_with(bytes, push_surrogate_escape)
}

/// Decode a little-endian UTF-16 byte slice (no BOM) into a Python string,
/// applying the specified error handler on invalid sequences.
///
/// See `decode_utf16` for the `original_bytes`/`bom_offset` contract.
fn decode_utf16_le(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
) -> Result<Value> {
    decode_utf16(bytes, original_bytes, bom_offset, errors, false)
}

/// Decode a big-endian UTF-16 byte slice (no BOM) into a Python string,
/// applying the specified error handler on invalid sequences.
///
/// See `decode_utf16` for the `original_bytes`/`bom_offset` contract.
fn decode_utf16_be(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
) -> Result<Value> {
    decode_utf16(bytes, original_bytes, bom_offset, errors, true)
}

/// Decode a UTF-16 byte slice (no BOM) into a Python string, applying the
/// specified error handler on invalid sequences. `big_endian` selects the
/// byte order, and thus the `utf-16-le`/`utf-16-be` codec name reported in
/// any `UnicodeDecodeError`.
///
/// `original_bytes` is the full original byte sequence passed by the caller
/// (may include a BOM prefix).  `bom_offset` is the number of BOM bytes that
/// were stripped before `bytes` was derived from `original_bytes`.  Both are
/// forwarded to `decode_utf16_units` so that any `UnicodeDecodeError` carries
/// the full original bytes and correct start/end offsets — matching CPython's
/// behaviour (see issues #1781, #1813).
fn decode_utf16(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
    big_endian: bool,
) -> Result<Value> {
    let codec_name = if big_endian { "utf-16-be" } else { "utf-16-le" };
    let to_u16 = if big_endian {
        u16::from_be_bytes
    } else {
        u16::from_le_bytes
    };
    if !bytes.len().is_multiple_of(2) {
        // Truncated: odd number of bytes.
        let trunc_start = bom_offset + bytes.len() - 1;
        let trunc_end = bom_offset + bytes.len();
        match errors {
            "ignore" => {
                // Drop the trailing byte and decode the rest.
                let units: Vec<u16> = bytes[..bytes.len() - 1]
                    .chunks_exact(2)
                    .map(|c| to_u16([c[0], c[1]]))
                    .collect();
                return decode_utf16_units(&units, original_bytes, bom_offset, codec_name, errors);
            }
            "replace" => {
                let units: Vec<u16> = bytes[..bytes.len() - 1]
                    .chunks_exact(2)
                    .map(|c| to_u16([c[0], c[1]]))
                    .collect();
                let mut s = decode_utf16_units_to_string(
                    &units,
                    original_bytes,
                    bom_offset,
                    codec_name,
                    errors,
                )?;
                s.push('\u{FFFD}');
                return Ok(Value::string(&s));
            }
            "backslashreplace" => {
                let units: Vec<u16> = bytes[..bytes.len() - 1]
                    .chunks_exact(2)
                    .map(|c| to_u16([c[0], c[1]]))
                    .collect();
                let mut s = decode_utf16_units_to_string(
                    &units,
                    original_bytes,
                    bom_offset,
                    codec_name,
                    errors,
                )?;
                use std::fmt::Write as _;
                let _ = write!(s, "\\x{:02x}", bytes[bytes.len() - 1]);
                return Ok(Value::string(&s));
            }
            "strict" | "surrogateescape" => {
                return Err(PyError::UnicodeDecodeError {
                    encoding: codec_name.to_string(),
                    object: original_bytes.to_vec(),
                    start: trunc_start,
                    end: trunc_end,
                    reason: "truncated data".to_string(),
                });
            }
            _ => {
                return Err(PyError::named(
                    "LookupError",
                    format!("unknown error handler name '{errors}'"),
                ));
            }
        }
    }
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| to_u16([c[0], c[1]]))
        .collect();
    decode_utf16_units(&units, original_bytes, bom_offset, codec_name, errors)
}

/// Decode a slice of UTF-16 code units into a Python string, returning a `Value`.
///
/// `raw_bytes` is the full original byte sequence (including any BOM prefix).
/// `bom_offset` is the number of BOM bytes preceding the encoded units, used to
/// adjust `start`/`end` in `UnicodeDecodeError` — matching CPython's behaviour.
fn decode_utf16_units(
    units: &[u16],
    raw_bytes: &[u8],
    bom_offset: usize,
    codec_name: &str,
    errors: &str,
) -> Result<Value> {
    let s = decode_utf16_units_to_string(units, raw_bytes, bom_offset, codec_name, errors)?;
    Ok(Value::string(&s))
}

/// Inner helper: decode UTF-16 code units into a `String`, applying the error handler.
///
/// `raw_bytes` is the full original byte sequence (including any BOM prefix).
/// `bom_offset` adjusts `start`/`end` in `UnicodeDecodeError` and the byte slice
/// used by `backslashreplace` so that offsets are relative to `raw_bytes`.
///
/// This is factored out so the truncation-handling paths in `decode_utf16_le`/`_be`
/// can decode the valid prefix and then append their own substitution.
fn decode_utf16_units_to_string(
    units: &[u16],
    raw_bytes: &[u8],
    bom_offset: usize,
    codec_name: &str,
    errors: &str,
) -> Result<String> {
    // Validate the error handler name upfront for the non-strict paths, so that
    // an unknown handler always raises LookupError (matching CPython's behaviour
    // where the handler is validated regardless of whether any error occurs).
    // We do this check inside the error arms below; see the `_` arm.
    let mut out = String::with_capacity(units.len());
    let mut iter = units.iter().copied().enumerate();
    while let Some((i, u)) = iter.next() {
        match u {
            // High surrogate: expect a following low surrogate.
            0xD800..=0xDBFF => {
                let next = iter.next();
                match next {
                    Some((_, low)) if (0xDC00..=0xDFFF).contains(&low) => {
                        let cp = 0x10000 + ((u as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
                        out.push(char::from_u32(cp).expect("valid surrogate pair"));
                    }
                    // End of stream after a high surrogate: no low surrogate follows.
                    None => match errors {
                        "replace" => out.push('\u{FFFD}'),
                        "ignore" => {}
                        "backslashreplace" => {
                            // Emit the two raw bytes of the high surrogate unit.
                            use std::fmt::Write as _;
                            let byte_start = bom_offset + i * 2;
                            let unit_bytes = &raw_bytes[byte_start..byte_start + 2];
                            for &b in unit_bytes {
                                let _ = write!(out, "\\x{b:02x}");
                            }
                        }
                        _ => {
                            // "strict", "surrogateescape", and any unknown handler.
                            if errors != "strict" && errors != "surrogateescape" {
                                return Err(PyError::named(
                                    "LookupError",
                                    format!("unknown error handler name '{errors}'"),
                                ));
                            }
                            return Err(PyError::UnicodeDecodeError {
                                encoding: codec_name.to_string(),
                                object: raw_bytes.to_vec(),
                                start: bom_offset + i * 2,
                                end: bom_offset + i * 2 + 2,
                                reason: "unexpected end of data".to_string(),
                            });
                        }
                    },
                    // A non-low-surrogate unit follows the high surrogate.
                    Some((j, _)) => match errors {
                        "replace" => {
                            // Replace the bad high surrogate; re-process the next unit.
                            out.push('\u{FFFD}');
                            // Put j back — we can't un-advance an iterator, so decode
                            // the next unit directly from its value.
                            let next_u = units[j];
                            match next_u {
                                0xD800..=0xDBFF => {
                                    // Another high surrogate: will be handled next iteration —
                                    // but we already consumed it. Push a replacement for it too
                                    // only if it itself has no following low (which we can't
                                    // check here). This is subtle: CPython replaces only the
                                    // first bad unit and then continues, so we do the same by
                                    // pushing back via a sub-decode of the remaining slice.
                                    // Simple approach: just re-run from j onward.
                                    let sub = decode_utf16_units_to_string(
                                        &units[j..],
                                        raw_bytes,
                                        bom_offset + j * 2,
                                        codec_name,
                                        errors,
                                    )?;
                                    out.push_str(&sub);
                                    return Ok(out);
                                }
                                0xDC00..=0xDFFF => {
                                    // Lone low surrogate — replace it too.
                                    out.push('\u{FFFD}');
                                }
                                _ => {
                                    out.push(
                                        char::from_u32(next_u as u32)
                                            .expect("BMP codepoint is valid"),
                                    );
                                }
                            }
                        }
                        "ignore" => {
                            // Skip the bad high surrogate; re-process the next unit.
                            let next_u = units[j];
                            match next_u {
                                0xD800..=0xDFFF => {
                                    let sub = decode_utf16_units_to_string(
                                        &units[j..],
                                        raw_bytes,
                                        bom_offset + j * 2,
                                        codec_name,
                                        errors,
                                    )?;
                                    out.push_str(&sub);
                                    return Ok(out);
                                }
                                _ => {
                                    out.push(
                                        char::from_u32(next_u as u32)
                                            .expect("BMP codepoint is valid"),
                                    );
                                }
                            }
                        }
                        "backslashreplace" => {
                            use std::fmt::Write as _;
                            let byte_start = bom_offset + i * 2;
                            let unit_bytes = &raw_bytes[byte_start..byte_start + 2];
                            for &b in unit_bytes {
                                let _ = write!(out, "\\x{b:02x}");
                            }
                            // Re-process the unit that followed.
                            let sub = decode_utf16_units_to_string(
                                &units[j..],
                                raw_bytes,
                                bom_offset + j * 2,
                                codec_name,
                                errors,
                            )?;
                            out.push_str(&sub);
                            return Ok(out);
                        }
                        _ => {
                            if errors != "strict" && errors != "surrogateescape" {
                                return Err(PyError::named(
                                    "LookupError",
                                    format!("unknown error handler name '{errors}'"),
                                ));
                            }
                            return Err(PyError::UnicodeDecodeError {
                                encoding: codec_name.to_string(),
                                object: raw_bytes.to_vec(),
                                start: bom_offset + i * 2,
                                end: bom_offset + i * 2 + 2,
                                reason: "illegal UTF-16 surrogate".to_string(),
                            });
                        }
                    },
                }
            }
            // Lone low surrogate: invalid.
            0xDC00..=0xDFFF => match errors {
                "replace" => out.push('\u{FFFD}'),
                "ignore" => {}
                "backslashreplace" => {
                    use std::fmt::Write as _;
                    let byte_start = bom_offset + i * 2;
                    let unit_bytes = &raw_bytes[byte_start..byte_start + 2];
                    for &b in unit_bytes {
                        let _ = write!(out, "\\x{b:02x}");
                    }
                }
                _ => {
                    if errors != "strict" && errors != "surrogateescape" {
                        return Err(PyError::named(
                            "LookupError",
                            format!("unknown error handler name '{errors}'"),
                        ));
                    }
                    return Err(PyError::UnicodeDecodeError {
                        encoding: codec_name.to_string(),
                        object: raw_bytes.to_vec(),
                        start: bom_offset + i * 2,
                        end: bom_offset + i * 2 + 2,
                        reason: "illegal encoding".to_string(),
                    });
                }
            },
            // BMP character.
            _ => {
                out.push(char::from_u32(u as u32).expect("BMP codepoint is valid"));
            }
        }
    }
    Ok(out)
}

/// Decode a little-endian UTF-32 byte slice (no BOM) into a Python string,
/// applying the specified error handler on invalid sequences.
///
/// See `decode_utf32` for the chunk-first rationale and the
/// `original_bytes`/`bom_offset` contract.
fn decode_utf32_le(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
) -> Result<Value> {
    decode_utf32(bytes, original_bytes, bom_offset, errors, false)
}

/// Decode a big-endian UTF-32 byte slice (no BOM) into a Python string,
/// applying the specified error handler on invalid sequences.
///
/// See `decode_utf32` for the chunk-first rationale and the
/// `original_bytes`/`bom_offset` contract.
fn decode_utf32_be(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
) -> Result<Value> {
    decode_utf32(bytes, original_bytes, bom_offset, errors, true)
}

/// Decode a UTF-32 byte slice (no BOM) into a Python string, applying the
/// specified error handler on invalid sequences. `big_endian` selects the
/// byte order, and thus the `utf-32-le`/`utf-32-be` codec name reported in
/// any `UnicodeDecodeError`.
///
/// CPython processes complete 4-byte chunks first (reporting "code point not in
/// range" on any invalid chunk) and only then reports "truncated data" for any
/// trailing bytes that don't form a complete chunk.  The early-truncation guard
/// is therefore removed in favour of checking the remainder after all full
/// chunks have been decoded successfully.
///
/// `original_bytes` is the full original byte sequence (including any BOM prefix).
/// `bom_offset` is the number of BOM bytes preceding `bytes` so that error
/// `start`/`end` offsets index into `original_bytes` — matching CPython's behaviour
/// (see issues #1781, #1813).
fn decode_utf32(
    bytes: &[u8],
    original_bytes: &[u8],
    bom_offset: usize,
    errors: &str,
    big_endian: bool,
) -> Result<Value> {
    let codec_name = if big_endian { "utf-32-be" } else { "utf-32-le" };
    let to_u32 = if big_endian {
        u32::from_be_bytes
    } else {
        u32::from_le_bytes
    };
    let chunks = bytes.chunks_exact(4);
    let remainder = chunks.remainder();
    let mut out = String::with_capacity(bytes.len() / 4);
    for (i, chunk) in chunks.enumerate() {
        let cp = to_u32([chunk[0], chunk[1], chunk[2], chunk[3]]);
        match char::from_u32(cp) {
            Some(c) => out.push(c),
            None => match errors {
                "replace" => out.push('\u{FFFD}'),
                "ignore" => {}
                "backslashreplace" => {
                    use std::fmt::Write as _;
                    for &b in chunk {
                        let _ = write!(out, "\\x{b:02x}");
                    }
                }
                _ => {
                    if errors != "strict" && errors != "surrogateescape" {
                        return Err(PyError::named(
                            "LookupError",
                            format!("unknown error handler name '{errors}'"),
                        ));
                    }
                    return Err(PyError::UnicodeDecodeError {
                        encoding: codec_name.to_string(),
                        object: original_bytes.to_vec(),
                        start: bom_offset + i * 4,
                        end: bom_offset + i * 4 + 4,
                        reason: "code point not in range(0x110000)".to_string(),
                    });
                }
            },
        }
    }
    if !remainder.is_empty() {
        let n = bytes.len() - remainder.len();
        match errors {
            "replace" => out.push('\u{FFFD}'),
            "ignore" => {}
            "backslashreplace" => {
                use std::fmt::Write as _;
                for &b in remainder {
                    let _ = write!(out, "\\x{b:02x}");
                }
            }
            _ => {
                if errors != "strict" && errors != "surrogateescape" {
                    return Err(PyError::named(
                        "LookupError",
                        format!("unknown error handler name '{errors}'"),
                    ));
                }
                return Err(PyError::UnicodeDecodeError {
                    encoding: codec_name.to_string(),
                    object: original_bytes.to_vec(),
                    start: bom_offset + n,
                    end: bom_offset + bytes.len(),
                    reason: "truncated data".to_string(),
                });
            }
        }
    }
    Ok(Value::string(&out))
}

/// Decode UTF-8 bytes, skipping any invalid byte sequences (errors='ignore').
fn bytes_decode_utf8_ignore(bytes: &[u8]) -> String {
    bytes_decode_utf8_with(bytes, |_out, _b| {})
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
        // A BigInt is by definition outside 0..=255; CPython reports the same
        // ValueError as an out-of-range plain int rather than a TypeError.
        ValueKind::BigInt(_) => Err(PyError::named(
            "ValueError",
            "byte must be in range(0, 256)".to_string(),
        )),
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
                    // Non-bytes tuple element: CPython names the offending
                    // element's type in the canonical message (issue #2044).
                    // bytearray / bytes-subclass elements are pre-coerced to
                    // `Bytes` by the interpreter, so only genuinely-wrong types
                    // reach here.
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "a bytes-like object is required, not '{}'",
                                pyrust_core::builtin_type_name(pv)
                            ),
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
                    // Non-bytes tuple element: CPython names the offending
                    // element's type in the canonical message (issue #2044).
                    // bytearray / bytes-subclass elements are pre-coerced to
                    // `Bytes` by the interpreter, so only genuinely-wrong types
                    // reach here.
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "a bytes-like object is required, not '{}'",
                                pyrust_core::builtin_type_name(sv)
                            ),
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
        Some(ValueKind::BigInt(n)) => bigint_start_idx(n, len),
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
        Some(ValueKind::BigInt(n)) => bigint_end_idx(n, len),
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

/// Normalise a `BigInt` `start` bound for a search window. A BigInt never fits in
/// an index range, so CPython clamps it: a negative one to the start (`0`), a
/// positive one to just past the end (`len + 1`) so the inverted-window check in
/// the caller reports "not found" rather than a zero-length window (#2688).
fn bigint_start_idx(n: &pyrust_core::PyBigInt, len: usize) -> usize {
    match n.sign() {
        PyBigIntSign::Minus => 0,
        _ => len + 1,
    }
}

/// Normalise a `BigInt` `end` bound for a search window: a negative one clamps to
/// the start (`0`), a positive one to the end (`len`) (#2688).
fn bigint_end_idx(n: &pyrust_core::PyBigInt, len: usize) -> usize {
    match n.sign() {
        PyBigIntSign::Minus => 0,
        _ => len,
    }
}

/// Naive substring search: find first occurrence of `sub` in `haystack`,
/// returning the starting byte index, or `None` if not found.
pub fn find_subsequence(haystack: &[u8], sub: &[u8]) -> Option<usize> {
    if sub.len() > haystack.len() {
        return None;
    }
    haystack.windows(sub.len()).position(|w| w == sub)
}

/// Naive substring search: find last occurrence of `sub` in `haystack`,
/// returning the starting byte index, or `None` if not found.
pub fn rfind_subsequence(haystack: &[u8], sub: &[u8]) -> Option<usize> {
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
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            ));
        }
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

/// Linear membership avoids building a bitmap for short `chars`.
const STRIP_LINEAR_SCAN_MAX: usize = 8;

fn bytes_strip(bytes: &[u8], args: &[Value], left: bool, right: bool) -> Result<Value> {
    let chars_owned: Option<std::borrow::Cow<'_, [u8]>> = match args.first() {
        None => None,
        Some(v) => match v.kind() {
            ValueKind::None => None,
            ValueKind::Bytes(rc) => Some(std::borrow::Cow::Borrowed(rc.as_slice())),
            // Accept any bytes-like object (bytearray, ...) per CPython; reject
            // genuinely-wrong types (str, int, ...) with CPython's message.
            _ => match crate::bytearray::as_bytearray_snapshot(v) {
                Some(data) => Some(std::borrow::Cow::Owned(data)),
                None => {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "a bytes-like object is required, not '{}'",
                            pyrust_core::builtin_type_name(v)
                        ),
                    ));
                }
            },
        },
    };
    let chars_arg: Option<&[u8]> = chars_owned.as_deref();
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
            if chars.is_empty() {
                // Nothing can be stripped.
            } else if chars.len() == 1 {
                let needle = chars[0];
                if left {
                    while start < end && bytes[start] == needle {
                        start += 1;
                    }
                }
                if right {
                    while end > start && bytes[end - 1] == needle {
                        end -= 1;
                    }
                }
            } else if chars.len() <= STRIP_LINEAR_SCAN_MAX {
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
            } else {
                let mut bitmap = [0u64; 4];
                for &byte in chars {
                    bitmap[(byte >> 6) as usize] |= 1u64 << (byte & 63);
                }
                let contains = |byte: u8| bitmap[(byte >> 6) as usize] & (1u64 << (byte & 63)) != 0;
                if left {
                    while start < end && contains(bytes[start]) {
                        start += 1;
                    }
                }
                if right {
                    while end > start && contains(bytes[end - 1]) {
                        end -= 1;
                    }
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

/// Normalise `split`/`rsplit` keyword arguments (`sep`, `maxsplit`) into the
/// positional slots `[sep, maxsplit]` that `bytes_split_args` expects.
///
/// CPython 3.12 accepts both `sep` and `maxsplit` by keyword for
/// `bytes`/`bytearray` `split`/`rsplit`; passing the same value by both name
/// and position, an unknown keyword, or more than two positionals all raise
/// `TypeError`.  Error messages use the bare method name (`split()` /
/// `rsplit()`), matching CPython's `Objects/bytesobject.c`.
fn merge_split_kwargs(method: &str, args: &[Value], kwargs: &PyDict) -> Result<Vec<Value>> {
    merge_split_kwargs_iter(
        method,
        args,
        kwargs.len(),
        kwargs.iter().map(|(k, v)| {
            let key = match k {
                PyKey::Str(s) => s.as_str().unwrap_or(""),
                _ => "",
            };
            (key, v)
        }),
    )
}

/// `bytearray.split`/`rsplit` keep their kwargs in a `String`-keyed map; this
/// shim threads them through the same merge logic as `bytes`.
pub fn merge_split_kwargs_str(
    method: &str,
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Vec<Value>> {
    merge_split_kwargs_iter(
        method,
        args,
        kwargs.len(),
        kwargs.iter().map(|(k, v)| (k.as_str(), v)),
    )
}

/// Shared core: normalise `split`/`rsplit` `sep`/`maxsplit` keywords into the
/// positional slots `[sep, maxsplit]`, generic over the keyword key type.
fn merge_split_kwargs_iter<'a>(
    method: &str,
    args: &[Value],
    kwargs_len: usize,
    kwargs: impl Iterator<Item = (&'a str, &'a Value)>,
) -> Result<Vec<Value>> {
    if kwargs_len == 0 {
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{method}() takes at most 2 arguments ({} given)",
                    args.len()
                ),
            ));
        }
        return Ok(args.to_vec());
    }

    // CPython's argument parser checks the total argument count against the
    // two-positional limit before resolving per-keyword position conflicts, so
    // `split(b" ", 1, maxsplit=1)` reports "takes at most 2 arguments" rather
    // than a name/position clash.
    let total = args.len() + kwargs_len;
    if total > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{method}() takes at most 2 arguments ({total} given)"),
        ));
    }

    let mut pos = args.to_vec();
    let mut sep: Option<Value> = None;
    let mut maxsplit: Option<Value> = None;
    for (key_str, v) in kwargs {
        match key_str {
            "sep" => {
                if !pos.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("argument for {method}() given by name ('sep') and position (1)"),
                    ));
                }
                sep = Some(v.clone());
            }
            "maxsplit" => {
                maxsplit = Some(v.clone());
            }
            other => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{other}' is an invalid keyword argument for {method}()"),
                ));
            }
        }
    }

    // Fill positional slots: pos[0] = sep, pos[1] = maxsplit.
    if let Some(ms) = maxsplit {
        if pos.is_empty() {
            pos.push(sep.unwrap_or_else(Value::none));
        } else if let Some(sep_val) = sep {
            pos[0] = sep_val;
        }
        if pos.len() < 2 {
            pos.push(ms);
        }
    } else if let Some(sep_val) = sep {
        if pos.is_empty() {
            pos.push(sep_val);
        } else {
            pos[0] = sep_val;
        }
    }
    Ok(pos)
}

/// Normalise a single-keyword method (`expandtabs(tabsize=…)`,
/// `splitlines(keepends=…)`) into a `[value]` positional slot.
///
/// CPython 3.12 accepts the sole argument by keyword as well as position for
/// these `bytes`/`bytearray` methods. Passing it both ways, supplying more
/// than one positional, or using an unknown keyword all raise `TypeError`.
/// CPython's arg parser checks the total argument count against the
/// one-positional limit before resolving the name/position clash, so both
/// `m(x, kw=y)` and `m(x, y)` report `takes at most 1 argument (2 given)`.
pub fn merge_single_kwarg(
    method: &str,
    keyword: &str,
    args: &[Value],
    kwargs: &PyDict,
) -> Result<Vec<Value>> {
    merge_single_kwarg_iter(
        method,
        keyword,
        args,
        kwargs.len(),
        kwargs.iter().map(|(k, v)| {
            let key = match k {
                PyKey::Str(s) => s.as_str().unwrap_or(""),
                _ => "",
            };
            (key, v)
        }),
    )
}

/// `bytearray` keeps its kwargs in a `String`-keyed map; this shim threads them
/// through the same single-keyword merge logic as `bytes`.
pub fn merge_single_kwarg_str(
    method: &str,
    keyword: &str,
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Vec<Value>> {
    merge_single_kwarg_iter(
        method,
        keyword,
        args,
        kwargs.len(),
        kwargs.iter().map(|(k, v)| (k.as_str(), v)),
    )
}

/// Shared core for [`merge_single_kwarg`] / [`merge_single_kwarg_str`].
fn merge_single_kwarg_iter<'a>(
    method: &str,
    keyword: &str,
    args: &[Value],
    kwargs_len: usize,
    kwargs: impl Iterator<Item = (&'a str, &'a Value)>,
) -> Result<Vec<Value>> {
    if kwargs_len == 0 {
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{method}() takes at most 1 argument ({} given)", args.len()),
            ));
        }
        return Ok(args.to_vec());
    }

    let total = args.len() + kwargs_len;
    if total > 1 {
        // CPython distinguishes the all-keyword overflow ("keyword argument")
        // from the mixed/positional overflow ("argument").
        let noun = if args.is_empty() {
            "keyword argument"
        } else {
            "argument"
        };
        return Err(PyError::named(
            "TypeError",
            format!("{method}() takes at most 1 {noun} ({total} given)"),
        ));
    }

    // total == 1 here, so args is empty and there is exactly one keyword.
    let (key_str, v) = kwargs.into_iter().next().expect("one keyword");
    if key_str != keyword {
        return Err(PyError::named(
            "TypeError",
            format!("'{key_str}' is an invalid keyword argument for {method}()"),
        ));
    }
    Ok(vec![v.clone()])
}

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
    let sep = match args.first() {
        Some(v) => match v.kind() {
            ValueKind::Bytes(rc) => Some(rc.as_slice()),
            ValueKind::None => None,
            // CPython 3.12 names the offending separator type (issue #2044).
            // bytearray / bytes-subclass separators are pre-coerced to `Bytes`
            // by the interpreter, so only genuinely-wrong types reach here.
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "a bytes-like object is required, not '{}'",
                        pyrust_core::builtin_type_name(v)
                    ),
                ));
            }
        },
        None => None,
    };
    let maxsplit: i64 = match args.get(1).map(|v| v.kind()) {
        None => -1,
        Some(ValueKind::Int(n)) => n,
        Some(ValueKind::Bool(b)) => b as i64,
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C ssize_t",
            ));
        }
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
        ValueKind::Tuple(tuple_items) => collect_items(tuple_items)?,
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
        Some(ValueKind::BigInt(_)) => Err(PyError::named(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )),
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
    out.extend(std::iter::repeat_n(fill, left));
    out.extend_from_slice(bytes);
    out.extend(std::iter::repeat_n(fill, right));
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
    out.extend(std::iter::repeat_n(fill, width - len));
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
    out.extend(std::iter::repeat_n(fill, width - len));
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
        out.extend(std::iter::repeat_n(b'0', pad));
        out.extend_from_slice(&bytes[1..]);
    } else {
        out.extend(std::iter::repeat_n(b'0', pad));
        out.extend_from_slice(bytes);
    }
    Ok(Value::bytes(out))
}

// ---------------------------------------------------------------------------
// translate
// ---------------------------------------------------------------------------

fn bytes_translate(bytes: &[u8], args: &[Value], kwargs: &PyDict) -> Result<Value> {
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
    let name = if reverse { "rpartition" } else { "partition" };
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "bytes.{name}() takes exactly one argument ({} given)",
                args.len()
            ),
        ));
    }
    let sep_val = &args[0];
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
        Some(ValueKind::BigInt(_)) => {
            return Err(PyError::named(
                "OverflowError",
                "Python int too large to convert to C int",
            ));
        }
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
                    out.extend(std::iter::repeat_n(b' ', spaces));
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
    // Accept any bytes-like object (bytes, bytearray, subclasses) per CPython.
    let from_owned: std::borrow::Cow<'_, [u8]> = match args[0].kind() {
        ValueKind::Bytes(rc) => std::borrow::Cow::Borrowed(rc.as_slice()),
        _ => match crate::bytearray::as_bytearray_snapshot(&args[0]) {
            Some(data) => std::borrow::Cow::Owned(data),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "a bytes-like object is required, not '{}'",
                        pyrust_core::builtin_type_name(&args[0])
                    ),
                ));
            }
        },
    };
    let to_owned: std::borrow::Cow<'_, [u8]> = match args[1].kind() {
        ValueKind::Bytes(rc) => std::borrow::Cow::Borrowed(rc.as_slice()),
        _ => match crate::bytearray::as_bytearray_snapshot(&args[1]) {
            Some(data) => std::borrow::Cow::Owned(data),
            None => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "a bytes-like object is required, not '{}'",
                        pyrust_core::builtin_type_name(&args[1])
                    ),
                ));
            }
        },
    };
    let from: &[u8] = from_owned.as_ref();
    let to: &[u8] = to_owned.as_ref();
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
