//! PEP 263 source-encoding detection (`detect_encoding`, `_get_normal_name`)
//! and `tokenize.open`, transcribed from CPython 3.12.3's `Lib/tokenize.py`.

use super::{
    ExpandedCallArg, Interpreter, PyError, Result, Value, ValueKind, builtin, call_method,
    call_pos, error_is, module_global, repr_of, syntax_error, truthy, try_swallow, type_error,
};

/// `codecs.BOM_UTF8`.
const BOM_UTF8: [u8; 3] = [0xef, 0xbb, 0xbf];

/// `cookie_re.match(line)`'s group 1, for
/// `cookie_re = re.compile(r'^[ \t\f]*#.*?coding[:=][ \t]*([-\w.]+)', re.ASCII)`.
///
/// `.` does not match `\n`, so the declaration must sit on the line's first
/// physical line; the lazy `.*?` tries every `coding` occurrence in turn
/// (a candidate that is not followed by `[:=]` and at least one name
/// character is skipped).  `\w` is ASCII-only (`re.ASCII`).
fn match_cookie(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches([' ', '\t', '\x0c']);
    let rest = rest.strip_prefix('#')?;
    let body = match rest.find('\n') {
        Some(end) => &rest[..end],
        None => rest,
    };
    let mut from = 0;
    while let Some(offset) = body[from..].find("coding") {
        let at = from + offset;
        let after = &body[at + "coding".len()..];
        if after.starts_with([':', '=']) {
            let tail = after[1..].trim_start_matches([' ', '\t']);
            let end = tail
                .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
                .unwrap_or(tail.len());
            if end > 0 {
                return Some(&tail[..end]);
            }
        }
        from = at + 1;
    }
    None
}

/// `blank_re.match(line)` for
/// `blank_re = re.compile(br'^[ \t\f]*(?:[#\r\n]|$)', re.ASCII)`.
fn is_blank_line(line: &[u8]) -> bool {
    let start = line
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t' | 0x0c))
        .unwrap_or(line.len());
    matches!(line.get(start), None | Some(b'#' | b'\r' | b'\n'))
}

/// `_get_normal_name(orig_enc)`, which imitates `get_normal_name` in
/// `tokenizer.c`.
fn normal_name(orig_enc: &str) -> String {
    // Only care about the first 12 characters.
    let head: String = orig_enc.chars().take(12).collect();
    let enc = head.to_lowercase().replace('_', "-");
    if enc == "utf-8" || enc.starts_with("utf-8-") {
        return "utf-8".to_string();
    }
    if matches!(enc.as_str(), "latin-1" | "iso-8859-1" | "iso-latin-1")
        || enc.starts_with("latin-1-")
        || enc.starts_with("iso-8859-1-")
        || enc.starts_with("iso-latin-1-")
    {
        return "iso-8859-1".to_string();
    }
    orig_enc.to_string()
}

pub(super) fn get_normal_name(_interp: &mut Interpreter, orig_enc: &Value) -> Result<Value> {
    match orig_enc.as_str() {
        Some(text) => Ok(Value::string(normal_name(text))),
        // `orig_enc[:12]` on anything but a `str` fails (or is not a `str`).
        None => Err(PyError::named(
            "AttributeError",
            format!(
                "'{}' object has no attribute 'lower'",
                pyrust_core::error_type_name(orig_enc)
            ),
        )),
    }
}

/// The bytes of an exact `bytes` object.
fn exact_bytes(value: &Value) -> Option<&[u8]> {
    match value.kind() {
        ValueKind::Bytes(bytes) => Some(bytes.as_slice()),
        _ => None,
    }
}

/// `line.decode('utf-8')` for `find_cookie`: `None` when the line is not valid
/// UTF-8.
fn decode_utf8(interp: &mut Interpreter, line: &Value) -> Result<Option<String>> {
    if let Some(bytes) = exact_bytes(line) {
        return Ok(std::str::from_utf8(bytes).ok().map(str::to_string));
    }
    let decoded = try_swallow(
        || call_method(interp, line, "decode", &[Value::string("utf-8")]),
        |err| error_is(err, "UnicodeDecodeError"),
    )?;
    match decoded {
        Ok(text) => match text.as_str() {
            Some(text) => Ok(Some(text.to_string())),
            None => Err(type_error(format!(
                "expected string or bytes-like object, got '{}'",
                pyrust_core::error_type_name(&text)
            ))),
        },
        Err(_) => Ok(None),
    }
}

/// `bool(a == b)`-free `x.startswith(prefix)`.
fn starts_with_bom(interp: &mut Interpreter, first: &Value) -> Result<bool> {
    if let Some(bytes) = exact_bytes(first) {
        return Ok(bytes.starts_with(&BOM_UTF8));
    }
    let bom = module_global(interp, "BOM_UTF8")?;
    let result = call_method(interp, first, "startswith", &[bom])?;
    truthy(interp, &result)
}

/// `first[3:]`.
fn drop_bom(interp: &mut Interpreter, first: &Value) -> Result<Value> {
    if let Some(bytes) = exact_bytes(first) {
        return Ok(Value::bytes(bytes[BOM_UTF8.len()..].to_vec()));
    }
    let bounds = crate::interpreter::make_slice_value(Some(Value::int(3)), None, None);
    crate::builtin_modules::operator::getitem_value(interp, first.clone(), bounds)
}

/// `bool(blank_re.match(line))`.
fn blank_line(interp: &mut Interpreter, line: &Value) -> Result<bool> {
    if let Some(bytes) = exact_bytes(line) {
        return Ok(is_blank_line(bytes));
    }
    // A bytes-like object that is not an exact `bytes` (`bytearray`).
    let as_bytes = builtin(interp, "bytes", std::slice::from_ref(line))?;
    Ok(exact_bytes(&as_bytes).is_some_and(is_blank_line))
}

struct Detector {
    /// `readline.__self__.name`, when the stream has one.
    filename: Option<Value>,
    bom_found: bool,
}

impl Detector {
    /// `msg` or `'{} for {!r}'.format(msg, filename)`.
    fn with_filename(&self, interp: &mut Interpreter, msg: &str) -> Result<String> {
        match &self.filename {
            Some(name) => Ok(format!("{msg} for {}", repr_of(interp, name)?)),
            None => Ok(msg.to_string()),
        }
    }

    /// `find_cookie(line)`: the declared encoding, or `None`.
    fn find_cookie(&self, interp: &mut Interpreter, line: &Value) -> Result<Option<String>> {
        // Decode as UTF-8. Either the line is an encoding declaration, in
        // which case it should be pure ASCII, or it must be UTF-8 per default
        // encoding.
        let Some(line_string) = decode_utf8(interp, line)? else {
            let msg = self.with_filename(interp, "invalid or missing encoding declaration")?;
            return Err(syntax_error(msg));
        };
        let Some(cookie) = match_cookie(&line_string) else {
            return Ok(None);
        };
        let mut encoding = normal_name(cookie);
        let lookup = module_global(interp, "lookup")?;
        let found = try_swallow(
            || call_pos(interp, lookup, &[Value::string(encoding.as_str())]),
            |err| error_is(err, "LookupError"),
        )?;
        if found.is_err() {
            // This behaviour mimics the Python interpreter
            let msg = match &self.filename {
                None => format!("unknown encoding: {encoding}"),
                Some(name) => format!(
                    "unknown encoding for {}: {encoding}",
                    repr_of(interp, name)?
                ),
            };
            return Err(syntax_error(msg));
        }
        if self.bom_found {
            if encoding != "utf-8" {
                // This behaviour mimics the Python interpreter
                let msg = self.with_filename(interp, "encoding problem")?;
                return Err(syntax_error(format!("{msg}: utf-8")));
            }
            encoding.push_str("-sig");
        }
        Ok(Some(encoding))
    }
}

/// `read_or_stop()`: one call of `readline`, `b''` once it raises
/// `StopIteration`.
fn read_or_stop(interp: &mut Interpreter, readline: &Value) -> Result<Value> {
    let line = try_swallow(
        || call_pos(interp, readline.clone(), &[]),
        crate::interpreter::is_stop_iteration_error,
    )?;
    Ok(line.unwrap_or_else(|_| Value::bytes(Vec::new())))
}

/// `(encoding, lines)`.
fn detected(encoding: &str, lines: Vec<Value>) -> Value {
    Value::tuple(vec![Value::string(encoding), Value::list(lines)])
}

/// `detect_encoding(readline)`.
pub(super) fn detect_encoding(interp: &mut Interpreter, readline: &Value) -> Result<Value> {
    // try: filename = readline.__self__.name
    // except AttributeError: filename = None
    let filename = try_swallow(
        || {
            let owner = interp.get_attr(readline, "__self__")?;
            interp.get_attr(&owner, "name")
        },
        |err| error_is(err, "AttributeError"),
    )?
    .ok();
    let mut detector = Detector {
        filename,
        bom_found: false,
    };
    let mut default = "utf-8";

    let mut first = read_or_stop(interp, readline)?;
    if starts_with_bom(interp, &first)? {
        detector.bom_found = true;
        first = drop_bom(interp, &first)?;
        default = "utf-8-sig";
    }
    if !truthy(interp, &first)? {
        return Ok(detected(default, Vec::new()));
    }

    if let Some(encoding) = detector.find_cookie(interp, &first)? {
        return Ok(detected(&encoding, vec![first]));
    }
    if !blank_line(interp, &first)? {
        return Ok(detected(default, vec![first]));
    }

    let second = read_or_stop(interp, readline)?;
    if !truthy(interp, &second)? {
        return Ok(detected(default, vec![first]));
    }

    if let Some(encoding) = detector.find_cookie(interp, &second)? {
        return Ok(detected(&encoding, vec![first, second]));
    }

    Ok(detected(default, vec![first, second]))
}

/// `open(filename)`: open a file in read only mode using the encoding detected
/// by `detect_encoding()`.
pub(super) fn open_file(interp: &mut Interpreter, filename: &Value) -> Result<Value> {
    let builtin_open = module_global(interp, "_builtin_open")?;
    let buffer = call_pos(
        interp,
        builtin_open,
        &[filename.clone(), Value::string("rb")],
    )?;
    match open_text(interp, filename, &buffer) {
        Ok(text) => Ok(text),
        Err(err) => {
            // except: buffer.close(); raise
            call_method(interp, &buffer, "close", &[])?;
            Err(err)
        }
    }
}

fn open_text(interp: &mut Interpreter, filename: &Value, buffer: &Value) -> Result<Value> {
    let readline = interp.get_attr(buffer, "readline")?;
    let detect = module_global(interp, "detect_encoding")?;
    let result = call_pos(interp, detect, &[readline])?;
    let pair = interp.unpack_exact_values(&result, 2)?;
    let encoding = pair[0].clone();
    let wrapper = try_swallow(
        || module_global(interp, "TextIOWrapper"),
        |err| error_is(err, "AttributeError"),
    )?;
    let Ok(wrapper) = wrapper else {
        // No `io.TextIOWrapper` class in this runtime: the builtin `open`
        // builds the text file object itself.
        call_method(interp, buffer, "close", &[])?;
        let builtin_open = module_global(interp, "_builtin_open")?;
        return interp.call_function_expanded(
            builtin_open,
            &[
                ExpandedCallArg {
                    name: None,
                    value: filename.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string("r"),
                },
                ExpandedCallArg {
                    name: Some("encoding".to_string()),
                    value: encoding,
                },
            ],
        );
    };
    call_method(interp, buffer, "seek", &[Value::int(0)])?;
    let text = interp.call_function_expanded(
        wrapper,
        &[
            ExpandedCallArg {
                name: None,
                value: buffer.clone(),
            },
            ExpandedCallArg {
                name: None,
                value: encoding,
            },
            ExpandedCallArg {
                name: Some("line_buffering".to_string()),
                value: Value::bool_(true),
            },
        ],
    )?;
    interp.assign_attr(text.clone(), "mode", Value::string("r"))?;
    Ok(text)
}
