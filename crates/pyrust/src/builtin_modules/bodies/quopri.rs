// `quopri` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/quopri.py` (issues #3230, #3454).
//
// Like the Python source, `encode` / `encodestring` / `decode` /
// `decodestring` delegate to `binascii.b2a_qp` / `binascii.a2b_qp` (looked up
// as module attributes at call time, so `quopri.b2a_qp = None` selects the
// line-by-line fallback exactly as in CPython), and the fallback paths, the
// RFC 1521 helpers (`needsquoting`, `quote`, `ishex`, `unhex`) and the
// command-line `main()` are ported statement by statement.
//
// Reference: <https://docs.python.org/3.12/library/quopri.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBytesLike};
use crate::interpreter::{ConsumerIterator, ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyBigInt, PyModule, Value, ValueKind};

const MODULE_DOC: &str = "Conversions to/from quoted-printable transport encoding as per RFC 1521.";
const HEX: &[u8; 16] = b"0123456789ABCDEF";
const MAXLINESIZE: usize = 76;

// `def`s of `Lib/quopri.py`: CPython reports arity errors for these in the
// interpreter's `initialize_locals` wording, not the macro's.
const SIG_NEEDSQUOTING: Sig = Sig {
    qualname: "needsquoting",
    params: &["c", "quotetabs", "header"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_QUOTE: Sig = Sig {
    qualname: "quote",
    params: &["c"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_ENCODE: Sig = Sig {
    qualname: "encode",
    params: &["input", "output", "quotetabs", "header"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_ENCODESTRING: Sig = Sig {
    qualname: "encodestring",
    params: &["s", "quotetabs", "header"],
    defaults: 2,
    kwonly: &[],
    var_kw: false,
};
const SIG_DECODE: Sig = Sig {
    qualname: "decode",
    params: &["input", "output", "header"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_DECODESTRING: Sig = Sig {
    qualname: "decodestring",
    params: &["s", "header"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_ISHEX: Sig = Sig {
    qualname: "ishex",
    params: &["c"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_UNHEX: Sig = Sig {
    qualname: "unhex",
    params: &["s"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_MAIN: Sig = Sig {
    qualname: "main",
    params: &[],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

// ---------------------------------------------------------------------------
// Call helpers
// ---------------------------------------------------------------------------

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn call_with(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    expanded.extend(kw.iter().map(|(name, value)| ExpandedCallArg {
        name: Some((*name).to_string()),
        value: value.clone(),
    }));
    interp.call_function_expanded(callee, &expanded)
}

fn call_pos(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    call_with(interp, callee, pos, &[])
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, pos)
}

/// A module global of `quopri` (`b2a_qp`, `encode`, ...), resolved at call
/// time so reassigning it on the module is observed like in CPython.
fn module_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("quopri")?;
    interp.get_attr(&module, name)
}

/// A binary-mode `bytes` result (`read()` / `readline()`): `bytes`, `bytearray`
/// or `memoryview` content.
fn buffer_bytes(interp: &mut Interpreter, value: &Value) -> Result<Vec<u8>> {
    if !PyBytesLike::matches(value) {
        return Err(type_error(format!(
            "a bytes-like object is required, not '{}'",
            value_type_name_str(value)
        )));
    }
    Ok(PyBytesLike::try_from_value_with_interp(interp, value, "quopri", "data")?.into_vec())
}

/// `isinstance(c, bytes)` content (`bytes` or a subclass).
fn exact_bytes(value: &Value) -> Option<Vec<u8>> {
    if let ValueKind::Bytes(rc) = value.kind() {
        return Some(rc.as_ref().clone());
    }
    let backing = crate::interpreter::builtin_data_backing(value)?;
    match backing.kind() {
        ValueKind::Bytes(rc) => Some(rc.as_ref().clone()),
        _ => None,
    }
}

/// `assert isinstance(c, bytes)`.
fn assert_bytes(value: &Value) -> Result<Vec<u8>> {
    exact_bytes(value).ok_or_else(|| PyError::named("AssertionError", ""))
}

fn all_list() -> Value {
    Value::list(
        ["encode", "decode", "encodestring", "decodestring"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// RFC 1521 helpers
// ---------------------------------------------------------------------------

/// What `needsquoting` returns: it hands back the `quotetabs` / `header`
/// objects themselves for tabs, spaces and `_`.
enum Needs {
    Quotetabs,
    Header,
    Flag(bool),
}

fn needsquoting_core(c: &[u8]) -> Needs {
    // `c in b' \t'` is a substring test.
    if matches!(c, b"" | b" " | b"\t" | b" \t") {
        return Needs::Quotetabs;
    }
    if c == b"_" {
        return Needs::Header;
    }
    // `c == ESCAPE or not (b' ' <= c <= b'~')`.
    Needs::Flag(c == b"=" || !(c >= &b" "[..] && c <= &b"~"[..]))
}

fn quote_byte(b: u8) -> [u8; 3] {
    [b'=', HEX[(b / 16) as usize], HEX[(b % 16) as usize]]
}

fn is_hex_slice(c: &[u8]) -> bool {
    (c >= &b"0"[..] && c <= &b"9"[..])
        || (c >= &b"a"[..] && c <= &b"f"[..])
        || (c >= &b"A"[..] && c <= &b"F"[..])
}

fn hex_digit_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `repr(bytes((b,)))`.
fn single_byte_repr(b: u8) -> String {
    let body = match b {
        b'\'' => "'".to_string(),
        b'\\' => "\\\\".to_string(),
        b'\t' => "\\t".to_string(),
        b'\n' => "\\n".to_string(),
        b'\r' => "\\r".to_string(),
        0x20..=0x7e => (b as char).to_string(),
        _ => format!("\\x{b:02x}"),
    };
    // A lone `'` makes CPython pick double quotes.
    if b == b'\'' {
        format!("b\"{body}\"")
    } else {
        format!("b'{body}'")
    }
}

/// `bytes((c,))` for one item of an iterable of ints.
fn item_to_byte(item: &Value) -> Result<u8> {
    match item.as_int() {
        Some(n) if (0..=255).contains(&n) => Ok(n as u8),
        Some(_) => Err(PyError::named(
            "ValueError",
            "bytes must be in range(0, 256)",
        )),
        None => Err(type_error(format!(
            "'{}' object cannot be interpreted as an integer",
            value_type_name_str(item)
        ))),
    }
}

// ---------------------------------------------------------------------------
// Fallback (no `binascii`) encoder / decoder
// ---------------------------------------------------------------------------

/// The nested `write(s, output, lineEnd)` helper of `encode`.
fn fallback_write(
    interp: &mut Interpreter,
    output: &Value,
    s: &[u8],
    line_end: &[u8],
) -> Result<()> {
    let mut data = Vec::with_capacity(s.len() + 3 + line_end.len());
    match s.last() {
        // RFC 1521 requires that the line ending in a space or tab must have
        // that trailing character encoded.
        Some(&last) if last == b' ' || last == b'\t' => {
            data.extend_from_slice(&s[..s.len() - 1]);
            data.extend_from_slice(&quote_byte(last));
        }
        _ if s == b"." => data.extend_from_slice(&quote_byte(b'.')),
        _ => data.extend_from_slice(s),
    }
    data.extend_from_slice(line_end);
    call_method(interp, output, "write", &[Value::bytes(data)])?;
    Ok(())
}

fn fallback_encode(
    interp: &mut Interpreter,
    input: &Value,
    output: &Value,
    quotetabs: &Value,
    header: &Value,
) -> Result<()> {
    let mut prevline: Option<Vec<u8>> = None;
    let mut last_stripped: &[u8] = b"";
    loop {
        let line = call_method(interp, input, "readline", &[])?;
        if !interp.truthy_value(&line)? {
            break;
        }
        let mut line = buffer_bytes(interp, &line)?;
        // Strip off any readline induced trailing newline.
        let mut stripped: &[u8] = b"";
        if line.last() == Some(&b'\n') {
            line.pop();
            stripped = b"\n";
        }
        // Calculate the un-length-limited encoded line.
        let mut thisline: Vec<u8> = Vec::with_capacity(line.len());
        for &c in &line {
            let quoted = match needsquoting_core(&[c]) {
                Needs::Quotetabs => interp.truthy_value(quotetabs)?,
                Needs::Header => interp.truthy_value(header)?,
                Needs::Flag(f) => f,
            };
            if quoted {
                thisline.extend_from_slice(&quote_byte(c));
            } else if c == b' ' && interp.truthy_value(header)? {
                thisline.push(b'_');
            } else {
                thisline.push(c);
            }
        }
        // First, write out the previous line.
        if let Some(prev) = prevline.take() {
            fallback_write(interp, output, &prev, b"\n")?;
        }
        // Now see if we need any soft line breaks because of RFC-imposed
        // length limitations.
        while thisline.len() > MAXLINESIZE {
            // Don't forget to include the soft line break `=' sign in the
            // length calculation!
            fallback_write(interp, output, &thisline[..MAXLINESIZE - 1], b"=\n")?;
            thisline.drain(..MAXLINESIZE - 1);
        }
        prevline = Some(thisline);
        last_stripped = stripped;
    }
    // Write out the last line, without a trailing newline.
    if let Some(prev) = prevline {
        fallback_write(interp, output, &prev, last_stripped)?;
    }
    Ok(())
}

fn fallback_decode(
    interp: &mut Interpreter,
    input: &Value,
    output: &Value,
    header: &Value,
) -> Result<()> {
    let mut new: Vec<u8> = Vec::new();
    loop {
        let line = call_method(interp, input, "readline", &[])?;
        if !interp.truthy_value(&line)? {
            break;
        }
        let line = buffer_bytes(interp, &line)?;
        let mut i = 0usize;
        let mut n = line.len();
        let mut partial;
        if n > 0 && line[n - 1] == b'\n' {
            partial = false;
            n -= 1;
            // Strip trailing whitespace.
            while n > 0 && matches!(line[n - 1], b' ' | b'\t' | b'\r') {
                n -= 1;
            }
        } else {
            partial = true;
        }
        while i < n {
            let c = line[i];
            if c == b'_' && interp.truthy_value(header)? {
                new.push(b' ');
                i += 1;
            } else if c != b'=' {
                new.push(c);
                i += 1;
            } else if i + 1 == n && !partial {
                partial = true;
                break;
            } else if i + 1 < n && line[i + 1] == b'=' {
                new.push(b'=');
                i += 2;
            } else if i + 2 < n
                && is_hex_slice(&line[i + 1..i + 2])
                && is_hex_slice(&line[i + 2..i + 3])
            {
                let hi = hex_digit_value(line[i + 1]).unwrap_or(0);
                let lo = hex_digit_value(line[i + 2]).unwrap_or(0);
                new.push(hi * 16 + lo);
                i += 3;
            } else {
                // Bad escape sequence -- leave it in.
                new.push(c);
                i += 1;
            }
        }
        if !partial {
            new.push(b'\n');
            call_method(
                interp,
                output,
                "write",
                &[Value::bytes(std::mem::take(&mut new))],
            )?;
        }
    }
    if !new.is_empty() {
        call_method(interp, output, "write", &[Value::bytes(new)])?;
    }
    Ok(())
}

/// `encodestring` / `decodestring` fallback: round-trip through `BytesIO`.
fn bytesio_roundtrip(
    interp: &mut Interpreter,
    s: &Value,
    run: impl FnOnce(&mut Interpreter, &Value, &Value) -> Result<()>,
) -> Result<Value> {
    let io = interp.load_module("io")?;
    let bytesio = interp.get_attr(&io, "BytesIO")?;
    let infp = call_pos(interp, bytesio.clone(), std::slice::from_ref(s))?;
    let outfp = call_pos(interp, bytesio, &[])?;
    run(interp, &infp, &outfp)?;
    call_method(interp, &outfp, "getvalue", &[])
}

// ---------------------------------------------------------------------------
// main()
// ---------------------------------------------------------------------------

fn builtin_call(interp: &mut Interpreter, name: &'static str, pos: &[Value]) -> Result<Value> {
    call_pos(interp, Value::builtin_function(name), pos)
}

fn is_instance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> Result<bool> {
    let r = builtin_call(interp, "isinstance", &[obj.clone(), cls.clone()])?;
    interp.truthy_value(&r)
}

/// `sys.stdout = sys.stderr; print(*lines); sys.exit(2)`.
fn usage_exit(interp: &mut Interpreter, sys: &Value, lines: &[Value]) -> Result<Value> {
    let stderr = interp.get_attr(sys, "stderr")?;
    interp.assign_attr(sys.clone(), "stdout", stderr)?;
    for line in lines {
        builtin_call(interp, "print", std::slice::from_ref(line))?;
    }
    call_method(interp, sys, "exit", &[Value::int(2)])
}

fn run_main(interp: &mut Interpreter) -> Result<Value> {
    let getopt = interp.load_module("getopt")?;
    let sys = interp.load_module("sys")?;

    let argv = interp.get_attr(&sys, "argv")?;
    // `sys.argv[1:]`
    let slice = crate::interpreter::make_slice_value(Some(Value::int(1)), None, None);
    let argv_tail = crate::builtin_modules::operator::getitem_value(interp, argv, slice)?;

    let getopt_fn = interp.get_attr(&getopt, "getopt")?;
    let parsed = match call_pos(interp, getopt_fn, &[argv_tail, Value::string("td")]) {
        Ok(v) => v,
        Err(err) => {
            let exc = interp.materialize_pyerror(err.clone())?;
            let error_cls = interp.get_attr(&getopt, "error")?;
            if !is_instance(interp, &exc, &error_cls)? {
                return Err(err);
            }
            return usage_exit(
                interp,
                &sys,
                &[
                    exc,
                    Value::string("usage: quopri [-t | -d] [file] ..."),
                    Value::string("-t: quote tabs"),
                    Value::string("-d: decode; default encode"),
                ],
            );
        }
    };
    let opts = parsed
        .as_tuple()
        .and_then(|items| items.first().cloned())
        .unwrap_or_else(Value::none);
    let mut args = parsed
        .as_tuple()
        .and_then(|items| items.get(1).cloned())
        .unwrap_or_else(Value::none);

    let mut deco = false;
    let mut tabs = false;
    let mut opts_iter = ConsumerIterator::new(interp, &opts)?;
    while let Some(pair) = opts_iter.next(interp)? {
        let o = pair
            .as_tuple()
            .and_then(|items| items.first().cloned())
            .unwrap_or_else(Value::none);
        match o.as_str() {
            Some("-t") => tabs = true,
            Some("-d") => deco = true,
            _ => {}
        }
    }
    if tabs && deco {
        return usage_exit(
            interp,
            &sys,
            &[Value::string("-t and -d are mutually exclusive")],
        );
    }
    if !interp.truthy_value(&args)? {
        args = Value::list(vec![Value::string("-")]);
    }

    let mut sts = 0i64;
    let mut files = ConsumerIterator::new(interp, &args)?;
    while let Some(file) = files.next(interp)? {
        let is_stdin = file.as_str() == Some("-");
        let fp = if is_stdin {
            let stdin = interp.get_attr(&sys, "stdin")?;
            interp.get_attr(&stdin, "buffer")?
        } else {
            match builtin_call(interp, "open", &[file.clone(), Value::string("rb")]) {
                Ok(fp) => fp,
                Err(err) => {
                    let exc = interp.materialize_pyerror(err.clone())?;
                    let oserror = crate::interpreter::lookup_exc_class("OSError")
                        .map(Value::py_class)
                        .ok_or_else(|| PyError::runtime("quopri: OSError missing".into()))?;
                    if !is_instance(interp, &exc, &oserror)? {
                        return Err(err);
                    }
                    let msg = interp.eval_binary(
                        Value::string("%s: can't open (%s)\n"),
                        BinaryOp::Mod,
                        Value::tuple(vec![file.clone(), exc]),
                    )?;
                    let stderr = interp.get_attr(&sys, "stderr")?;
                    call_method(interp, &stderr, "write", &[msg])?;
                    sts = 1;
                    continue;
                }
            }
        };
        let stdout = interp.get_attr(&sys, "stdout")?;
        let outbuf = interp.get_attr(&stdout, "buffer");
        let result = outbuf.and_then(|outbuf| {
            if deco {
                let decode = module_attr(interp, "decode")?;
                call_pos(interp, decode, &[fp.clone(), outbuf])
            } else {
                let encode = module_attr(interp, "encode")?;
                call_pos(interp, encode, &[fp.clone(), outbuf, Value::bool_(tabs)])
            }
        });
        if !is_stdin {
            let closed = call_method(interp, &fp, "close", &[]);
            result?;
            closed?;
        } else {
            result?;
        }
    }
    if sts != 0 {
        call_method(interp, &sys, "exit", &[Value::int(sts)])?;
    }
    Ok(Value::none())
}

// ---------------------------------------------------------------------------
// Module
// ---------------------------------------------------------------------------

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
        "ESCAPE" => Value::bytes(b"=".to_vec()),
        "MAXLINESIZE" => Value::int(MAXLINESIZE as i64),
        "HEX" => Value::bytes(HEX.to_vec()),
        "EMPTYSTRING" => Value::bytes(Vec::new()),
    }

    // Decide whether a particular byte ordinal needs to be quoted.
    //
    // The 'quotetabs' flag indicates whether embedded tabs and spaces should be
    // quoted.  Note that line-ending tabs and spaces are always encoded, as per
    // RFC 1521.
    fn needsquoting(args) -> Result<Value> {
        let mut b = args::bind(&SIG_NEEDSQUOTING, args)?;
        let c = b.take_or_none(0);
        let quotetabs = b.take_or_none(1);
        let header = b.take_or_none(2);
        let c = assert_bytes(&c)?;
        Ok(match needsquoting_core(&c) {
            Needs::Quotetabs => quotetabs,
            Needs::Header => header,
            Needs::Flag(f) => Value::bool_(f),
        })
    }

    // Quote a single character.
    fn quote(args) -> Result<Value> {
        let mut b = args::bind(&SIG_QUOTE, args)?;
        let c = b.take_or_none(0);
        let c = assert_bytes(&c)?;
        if c.len() != 1 {
            return Err(PyError::named("AssertionError", ""));
        }
        Ok(Value::bytes(quote_byte(c[0]).to_vec()))
    }

    // Read 'input', apply quoted-printable encoding, and write to 'output'.
    //
    // 'input' and 'output' are binary file objects. The 'quotetabs' flag
    // indicates whether embedded tabs and spaces should be quoted. Note that
    // line-ending tabs and spaces are always encoded, as per RFC 1521.
    // The 'header' flag indicates whether we are encoding spaces as _ as per RFC
    // 1522.
    fn encode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_ENCODE, args)?;
        let input = b.take_or_none(0);
        let output = b.take_or_none(1);
        let quotetabs = b.take_or_none(2);
        let header = b.take_or(3, || Value::bool_(false));
        let b2a_qp = module_attr(_interp, "b2a_qp")?;
        if !b2a_qp.is_none() {
            let data = call_method(_interp, &input, "read", &[])?;
            let odata = call_with(
                _interp,
                b2a_qp,
                &[data],
                &[("quotetabs", quotetabs), ("header", header)],
            )?;
            call_method(_interp, &output, "write", &[odata])?;
            return Ok(Value::none());
        }
        fallback_encode(_interp, &input, &output, &quotetabs, &header)?;
        Ok(Value::none())
    }

    fn encodestring(args) -> Result<Value> {
        let mut b = args::bind(&SIG_ENCODESTRING, args)?;
        let s = b.take_or_none(0);
        let quotetabs = b.take_or(1, || Value::bool_(false));
        let header = b.take_or(2, || Value::bool_(false));
        let b2a_qp = module_attr(_interp, "b2a_qp")?;
        if !b2a_qp.is_none() {
            return call_with(
                _interp,
                b2a_qp,
                &[s],
                &[("quotetabs", quotetabs), ("header", header)],
            );
        }
        bytesio_roundtrip(_interp, &s, |interp, infp, outfp| {
            let encode = module_attr(interp, "encode")?;
            call_pos(interp, encode, &[infp.clone(), outfp.clone(), quotetabs, header])?;
            Ok(())
        })
    }

    // Read 'input', apply quoted-printable decoding, and write to 'output'.
    // 'input' and 'output' are binary file objects.
    // If 'header' is true, decode underscore as space (per RFC 1522).
    fn decode(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DECODE, args)?;
        let input = b.take_or_none(0);
        let output = b.take_or_none(1);
        let header = b.take_or(2, || Value::bool_(false));
        let a2b_qp = module_attr(_interp, "a2b_qp")?;
        if !a2b_qp.is_none() {
            let data = call_method(_interp, &input, "read", &[])?;
            let odata = call_with(_interp, a2b_qp, &[data], &[("header", header)])?;
            call_method(_interp, &output, "write", &[odata])?;
            return Ok(Value::none());
        }
        fallback_decode(_interp, &input, &output, &header)?;
        Ok(Value::none())
    }

    fn decodestring(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DECODESTRING, args)?;
        let s = b.take_or_none(0);
        let header = b.take_or(1, || Value::bool_(false));
        let a2b_qp = module_attr(_interp, "a2b_qp")?;
        if !a2b_qp.is_none() {
            return call_with(_interp, a2b_qp, &[s], &[("header", header)]);
        }
        bytesio_roundtrip(_interp, &s, |interp, infp, outfp| {
            let decode = module_attr(interp, "decode")?;
            call_with(
                interp,
                decode,
                &[infp.clone(), outfp.clone()],
                &[("header", header)],
            )?;
            Ok(())
        })
    }

    // Return true if the byte ordinal 'c' is a hexadecimal digit in ASCII.
    fn ishex(args) -> Result<Value> {
        let mut b = args::bind(&SIG_ISHEX, args)?;
        let c = b.take_or_none(0);
        let c = assert_bytes(&c)?;
        Ok(Value::bool_(is_hex_slice(&c)))
    }

    // Get the integer value of a hexadecimal number.
    fn unhex(args) -> Result<Value> {
        let mut b = args::bind(&SIG_UNHEX, args)?;
        let s = b.take_or_none(0);
        let items: Vec<u8> = if PyBytesLike::matches(&s) {
            buffer_bytes(_interp, &s)?
        } else {
            let mut bytes = Vec::new();
            let mut iter = ConsumerIterator::new(_interp, &s)?;
            while let Some(item) = iter.next(_interp)? {
                bytes.push(item_to_byte(&item)?);
            }
            bytes
        };
        let mut small: Option<i64> = Some(0);
        let mut big = PyBigInt::from(0);
        for &c in &items {
            let digit = match hex_digit_value(c) {
                Some(d) => i64::from(d),
                None => {
                    return Err(PyError::named(
                        "AssertionError",
                        format!("non-hex digit {}", single_byte_repr(c)),
                    ));
                }
            };
            if let Some(bits) = small {
                match bits.checked_mul(16).and_then(|v| v.checked_add(digit)) {
                    Some(v) => small = Some(v),
                    None => {
                        big = PyBigInt::from(bits) * 16 + digit;
                        small = None;
                    }
                }
            } else {
                big = big * 16 + digit;
            }
        }
        Ok(match small {
            Some(v) => Value::int(v),
            None => Value::bigint(big),
        })
    }

    fn main(args) -> Result<Value> {
        args::bind(&SIG_MAIN, args)?;
        run_main(_interp)
    }
}

/// Post-import wiring: `from binascii import a2b_qp, b2a_qp`.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let binascii = interp.load_module("binascii")?;
    let a2b_qp = interp.get_attr(&binascii, "a2b_qp")?;
    let b2a_qp = interp.get_attr(&binascii, "b2a_qp")?;
    let mut module = module.borrow_mut();
    module.insert_attr("a2b_qp".to_string(), a2b_qp);
    module.insert_attr("b2a_qp".to_string(), b2a_qp);
    Ok(())
}
