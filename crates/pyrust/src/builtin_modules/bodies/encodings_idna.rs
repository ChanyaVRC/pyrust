// `encodings.idna` module — 100% native-Rust implementation of Python 3.12's
// RFC 3490 (IDNA) and RFC 3491 (Nameprep) codec (issue #3250).
//
// Uses PyRust's native `stringprep` RFC 3454 tables, `unicodedata` NFKC
// normalization, and `encodings.punycode` RFC 3492 encoder/decoder.
//
// Exposes `dots`, `ace_prefix`, `sace_prefix`, `nameprep`, `ToASCII`,
// `ToUnicode`, `encode`, `decode`, `Codec`, `IncrementalEncoder`,
// `IncrementalDecoder`, `StreamWriter`, `StreamReader`, and `getregentry`.
//
// Reference: <https://datatracker.ietf.org/doc/html/rfc3490>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_core::{cesu8_codepoints, cesu8_encode_codepoint};
use pyrust_derive::pyrust_module;

use crate::builtin_modules::encodings_punycode::{
    punycode_decode_bytes_rust, punycode_encode_rust,
};
use crate::builtin_modules::macros::link_class_bases;
use crate::builtin_modules::stringprep::{
    in_table_b1_cp, in_table_c3_cp, in_table_c4_cp, in_table_c5_cp, in_table_c6_cp, in_table_c7_cp,
    in_table_c8_cp, in_table_c9_cp, in_table_c12_cp, in_table_c22_cp, in_table_d1_cp,
    in_table_d2_cp, map_table_b2_cp,
};
use crate::builtin_modules::unicodedata::normalize_nfkc;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBool, PyBytesLike, PyStr, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

const ACE_PREFIX: &[u8] = b"xn--";
const SACE_PREFIX: &str = "xn--";

#[inline]
fn is_idna_dot(ch: char) -> bool {
    matches!(ch, '\u{002E}' | '\u{3002}' | '\u{FF0E}' | '\u{FF61}')
}

fn split_idna_dots(s: &str) -> Vec<&str> {
    s.split(is_idna_dot).collect()
}

/// Pure-Rust RFC 3491 Nameprep implementation.
pub(crate) fn nameprep_rust(label: &str) -> Result<String> {
    // 1. Map (Table B.1 -> nothing, Table B.2 -> case-fold)
    let mut mapped = String::with_capacity(label.len());
    for cp in cesu8_codepoints(label) {
        if in_table_b1_cp(cp) {
            continue;
        }
        mapped.push_str(&map_table_b2_cp(cp));
    }

    // 2. Normalize (NFKC)
    let normalized = normalize_nfkc(&mapped);

    // 3. Prohibit
    let cps: Vec<u32> = cesu8_codepoints(&normalized).collect();
    for &cp in &cps {
        if in_table_c12_cp(cp)
            || in_table_c22_cp(cp)
            || in_table_c3_cp(cp)
            || in_table_c4_cp(cp)
            || in_table_c5_cp(cp)
            || in_table_c6_cp(cp)
            || in_table_c7_cp(cp)
            || in_table_c8_cp(cp)
            || in_table_c9_cp(cp)
        {
            let ch_s = cesu8_encode_codepoint(cp);
            let ch_repr = Value::string(ch_s).repr_raw();
            return Err(PyError::named(
                "UnicodeError",
                format!("Invalid character {ch_repr}"),
            ));
        }
    }

    // 4. Check BiDi
    let rand_al: Vec<bool> = cps.iter().map(|&cp| in_table_d1_cp(cp)).collect();
    if rand_al.iter().any(|&b| b) {
        if cps.iter().any(|&cp| in_table_d2_cp(cp)) {
            return Err(PyError::named(
                "UnicodeError",
                "Violation of BIDI requirement 2",
            ));
        }
        if !rand_al[0] || !rand_al[rand_al.len() - 1] {
            return Err(PyError::named(
                "UnicodeError",
                "Violation of BIDI requirement 3",
            ));
        }
    }

    Ok(normalized)
}

/// Pure-Rust RFC 3490 ToASCII.
pub(crate) fn to_ascii_rust(label: &str) -> Result<Vec<u8>> {
    // Step 1: try ASCII
    if label.is_ascii() {
        let b = label.as_bytes();
        if !b.is_empty() && b.len() < 64 {
            return Ok(b.to_vec());
        }
        return Err(PyError::named("UnicodeError", "label empty or too long"));
    }

    // Step 2: nameprep
    let prepped = nameprep_rust(label)?;

    // Step 4: try ASCII
    if prepped.is_ascii() {
        let b = prepped.as_bytes();
        if !b.is_empty() && b.len() < 64 {
            return Ok(b.to_vec());
        }
        return Err(PyError::named("UnicodeError", "label empty or too long"));
    }

    // Step 5: check ACE prefix
    if prepped.starts_with(SACE_PREFIX) {
        return Err(PyError::named(
            "UnicodeError",
            "Label starts with ACE prefix",
        ));
    }

    // Step 6 & 7: Punycode + ACE prefix
    let encoded = punycode_encode_rust(&prepped);
    let mut out = Vec::with_capacity(ACE_PREFIX.len() + encoded.len());
    out.extend_from_slice(ACE_PREFIX);
    out.extend_from_slice(&encoded);

    // Step 8: check size
    if !out.is_empty() && out.len() < 64 {
        Ok(out)
    } else {
        Err(PyError::named("UnicodeError", "label empty or too long"))
    }
}

/// Pure-Rust RFC 3490 ToUnicode accepting either `bytes` or `str`.
fn to_unicode_bytes_rust(label_bytes: &[u8]) -> Result<String> {
    if label_bytes.len() > 1024 {
        return Err(PyError::named("UnicodeError", "label way too long"));
    }
    if !label_bytes.starts_with(ACE_PREFIX) {
        let decoded = pyrust_builtins::bytes::decode_bytes(label_bytes, "ascii", "strict")?;
        return Ok(decoded.as_str().unwrap_or("").to_string());
    }
    let label1 = &label_bytes[ACE_PREFIX.len()..];
    let result = punycode_decode_bytes_rust(label1, "strict")?;
    let label2 = to_ascii_rust(&result)?;
    let label_str = pyrust_builtins::bytes::decode_bytes(label_bytes, "ascii", "strict")?;
    let l1 = label_str.as_str().unwrap_or("").to_ascii_lowercase();
    let l2 = std::str::from_utf8(&label2).unwrap_or("");
    if l1 != l2 {
        return Err(PyError::named("UnicodeError", "IDNA does not round-trip"));
    }
    Ok(result)
}

fn to_unicode_val(interp: &mut Interpreter, val: &Value) -> Result<String> {
    if let Some(s) = val.as_str() {
        let cp_len = cesu8_codepoints(s).count();
        if cp_len > 1024 {
            return Err(PyError::named("UnicodeError", "label way too long"));
        }
        if s.is_ascii() {
            return to_unicode_bytes_rust(s.as_bytes());
        }
        let prepped = nameprep_rust(s)?;
        if !prepped.is_ascii() {
            return Err(PyError::named(
                "UnicodeError",
                "Invalid character in IDN label",
            ));
        }
        return to_unicode_bytes_rust(prepped.as_bytes());
    }
    let bytes = PyBytesLike::try_from_value_with_interp(interp, val, "ToUnicode", "label")?;
    to_unicode_bytes_rust(bytes.as_bytes())
}

fn idna_encode_rust(input: &str, errors: &str) -> Result<(Vec<u8>, usize)> {
    if errors != "strict" {
        return Err(PyError::named(
            "UnicodeError",
            format!("unsupported error handling {errors}"),
        ));
    }
    if input.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let input_len = cesu8_codepoints(input).count();
    if input.is_ascii() {
        let raw = input.as_bytes();
        let labels: Vec<&[u8]> = raw.split(|&b| b == b'.').collect();
        for label in &labels[..labels.len() - 1] {
            if label.is_empty() || label.len() >= 64 {
                return Err(PyError::named("UnicodeError", "label empty or too long"));
            }
        }
        if labels[labels.len() - 1].len() >= 64 {
            return Err(PyError::named("UnicodeError", "label too long"));
        }
        return Ok((raw.to_vec(), input_len));
    }

    let mut labels = split_idna_dots(input);
    let trailing_dot = if let Some(&"") = labels.last() {
        labels.pop();
        true
    } else {
        false
    };
    let mut result = Vec::new();
    for label in labels {
        if !result.is_empty() {
            result.push(b'.');
        }
        result.extend_from_slice(&to_ascii_rust(label)?);
    }
    if trailing_dot {
        result.push(b'.');
    }
    Ok((result, input_len))
}

fn idna_decode_rust(
    interp: &mut Interpreter,
    input_val: &Value,
    errors: &str,
) -> Result<(String, usize)> {
    if errors != "strict" {
        return Err(PyError::named(
            "UnicodeError",
            format!("Unsupported error handling {errors}"),
        ));
    }
    let bytes_owned =
        PyBytesLike::try_from_value_with_interp(interp, input_val, "decode", "input")?.into_vec();
    let input_len = bytes_owned.len();
    if bytes_owned.is_empty() {
        return Ok((String::new(), 0));
    }
    let has_ace = bytes_owned
        .windows(ACE_PREFIX.len())
        .any(|w| w == ACE_PREFIX);
    if !has_ace
        && let Ok(decoded) = pyrust_builtins::bytes::decode_bytes(&bytes_owned, "ascii", "strict")
    {
        return Ok((decoded.as_str().unwrap_or("").to_string(), input_len));
    }

    let mut labels: Vec<&[u8]> = bytes_owned.split(|&b| b == b'.').collect();
    let trailing_dot = if let Some(last) = labels.last()
        && last.is_empty()
    {
        labels.pop();
        true
    } else {
        false
    };

    let mut parts = Vec::with_capacity(labels.len());
    for label in labels {
        parts.push(to_unicode_bytes_rust(label)?);
    }
    let mut joined = parts.join(".");
    if trailing_dot {
        joined.push('.');
    }
    Ok((joined, input_len))
}

fn idna_buffer_encode_rust(
    input: &str,
    errors: &str,
    final_flag: bool,
) -> Result<(Vec<u8>, usize)> {
    if errors != "strict" {
        return Err(PyError::named(
            "UnicodeError",
            format!("unsupported error handling {errors}"),
        ));
    }
    if input.is_empty() {
        return Ok((Vec::new(), 0));
    }
    let mut labels = split_idna_dots(input);
    let mut trailing_dot = false;
    if let Some(last) = labels.last() {
        if last.is_empty() {
            trailing_dot = true;
            labels.pop();
        } else if !final_flag {
            labels.pop();
            if !labels.is_empty() {
                trailing_dot = true;
            }
        }
    }
    let mut result = Vec::new();
    let mut size: usize = 0;
    for label in labels {
        if size > 0 {
            result.push(b'.');
            size += 1;
        }
        result.extend_from_slice(&to_ascii_rust(label)?);
        size += cesu8_codepoints(label).count();
    }
    if trailing_dot {
        result.push(b'.');
        size += 1;
    }
    Ok((result, size))
}

fn idna_buffer_decode_rust(
    interp: &mut Interpreter,
    input_val: &Value,
    errors: &str,
    final_flag: bool,
) -> Result<(String, usize)> {
    if errors != "strict" {
        return Err(PyError::named(
            "UnicodeError",
            format!("Unsupported error handling {errors}"),
        ));
    }
    let (owned_str, is_str_input) = if let Some(s) = input_val.as_str() {
        (s.to_string(), true)
    } else {
        let b =
            PyBytesLike::try_from_value_with_interp(interp, input_val, "_buffer_decode", "input")?;
        if b.as_bytes().is_empty() {
            return Ok((String::new(), 0));
        }
        let decoded = pyrust_builtins::bytes::decode_bytes(b.as_bytes(), "ascii", "strict")?;
        (decoded.as_str().unwrap_or("").to_string(), false)
    };
    if owned_str.is_empty() {
        return Ok((String::new(), 0));
    }
    let mut labels: Vec<&str> = if is_str_input {
        split_idna_dots(&owned_str)
    } else {
        owned_str.split('.').collect()
    };
    let mut trailing_dot = false;
    if let Some(last) = labels.last() {
        if last.is_empty() {
            trailing_dot = true;
            labels.pop();
        } else if !final_flag {
            labels.pop();
            if !labels.is_empty() {
                trailing_dot = true;
            }
        }
    }
    let mut parts = Vec::with_capacity(labels.len());
    let mut size: usize = 0;
    for label in labels {
        let dec = if label.is_ascii() {
            to_unicode_bytes_rust(label.as_bytes())?
        } else {
            let prepped = nameprep_rust(label)?;
            if !prepped.is_ascii() {
                return Err(PyError::named(
                    "UnicodeError",
                    "Invalid character in IDN label",
                ));
            }
            to_unicode_bytes_rust(prepped.as_bytes())?
        };
        parts.push(dec);
        if size > 0 {
            size += 1;
        }
        size += cesu8_codepoints(label).count();
    }
    let mut joined = parts.join(".");
    if trailing_dot {
        joined.push('.');
        size += 1;
    }
    Ok((joined, size))
}

fn call_positional(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn build_codec_info(interp: &mut Interpreter) -> Result<Value> {
    let codecs_mod = interp.load_module("codecs")?;
    let codec_info_cls = interp.get_attr(&codecs_mod, "CodecInfo")?;
    let self_mod = interp.load_module("encodings.idna")?;
    let codec_cls = interp.get_attr(&self_mod, "Codec")?;
    let codec_inst = call_positional(interp, codec_cls, &[])?;
    let enc_fn = interp.get_attr(&codec_inst, "encode")?;
    let dec_fn = interp.get_attr(&codec_inst, "decode")?;
    let inc_enc = interp.get_attr(&self_mod, "IncrementalEncoder")?;
    let inc_dec = interp.get_attr(&self_mod, "IncrementalDecoder")?;
    let stream_writer = interp.get_attr(&self_mod, "StreamWriter")?;
    let stream_reader = interp.get_attr(&self_mod, "StreamReader")?;
    interp.call_function_expanded(
        codec_info_cls,
        &[
            ExpandedCallArg {
                name: Some("name".to_string()),
                value: Value::string("idna"),
            },
            ExpandedCallArg {
                name: Some("encode".to_string()),
                value: enc_fn,
            },
            ExpandedCallArg {
                name: Some("decode".to_string()),
                value: dec_fn,
            },
            ExpandedCallArg {
                name: Some("incrementalencoder".to_string()),
                value: inc_enc,
            },
            ExpandedCallArg {
                name: Some("incrementaldecoder".to_string()),
                value: inc_dec,
            },
            ExpandedCallArg {
                name: Some("streamwriter".to_string()),
                value: stream_writer,
            },
            ExpandedCallArg {
                name: Some("streamreader".to_string()),
                value: stream_reader,
            },
        ],
    )
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // Install `dots` regex pattern matching CPython 3.12 (`re.compile("[\u002E\u3002\uFF0E\uFF61]")`).
    let re_mod = interp.load_module("re")?;
    let compile_fn = interp.get_attr(&re_mod, "compile")?;
    let dots = call_positional(
        interp,
        compile_fn,
        &[Value::string("[\u{002E}\u{3002}\u{FF0E}\u{FF61}]")],
    )?;
    module.borrow_mut().insert_attr("dots".to_string(), dots);

    // Link class bases against `codecs` base classes.
    let codecs_mod = interp.load_module("codecs")?;
    let codecs_codec = interp.get_attr(&codecs_mod, "Codec")?;
    let codecs_inc_enc = interp.get_attr(&codecs_mod, "BufferedIncrementalEncoder")?;
    let codecs_inc_dec = interp.get_attr(&codecs_mod, "BufferedIncrementalDecoder")?;
    let codecs_sw = interp.get_attr(&codecs_mod, "StreamWriter")?;
    let codecs_sr = interp.get_attr(&codecs_mod, "StreamReader")?;

    let local_codec = module.borrow().get_attr_value("Codec");
    let local_inc_enc = module.borrow().get_attr_value("IncrementalEncoder");
    let local_inc_dec = module.borrow().get_attr_value("IncrementalDecoder");
    let local_sw = module.borrow().get_attr_value("StreamWriter");
    let local_sr = module.borrow().get_attr_value("StreamReader");

    let as_cls = |v: &Value| match v.kind() {
        ValueKind::PyClass(c) => Some(Rc::clone(c)),
        _ => None,
    };

    if let (Some(lc), Some(bc)) = (local_codec.as_ref().and_then(as_cls), as_cls(&codecs_codec)) {
        link_class_bases(&lc, vec![bc]);
    }
    if let (Some(lc), Some(bc)) = (
        local_inc_enc.as_ref().and_then(as_cls),
        as_cls(&codecs_inc_enc),
    ) {
        link_class_bases(&lc, vec![bc]);
    }
    if let (Some(lc), Some(bc)) = (
        local_inc_dec.as_ref().and_then(as_cls),
        as_cls(&codecs_inc_dec),
    ) {
        link_class_bases(&lc, vec![bc]);
    }
    if let (Some(lc), Some(c_cls), Some(sw_cls)) = (
        local_sw.as_ref().and_then(as_cls),
        local_codec.as_ref().and_then(as_cls),
        as_cls(&codecs_sw),
    ) {
        link_class_bases(&lc, vec![c_cls, sw_cls]);
    }
    if let (Some(lc), Some(c_cls), Some(sr_cls)) = (
        local_sr.as_ref().and_then(as_cls),
        local_codec.as_ref().and_then(as_cls),
        as_cls(&codecs_sr),
    ) {
        link_class_bases(&lc, vec![c_cls, sr_cls]);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "ace_prefix" => Value::bytes(ACE_PREFIX.to_vec()),
        "sace_prefix" => Value::string(SACE_PREFIX),
    }

    fn nameprep(label: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::string(nameprep_rust(&label)?))
    }

    #[py_name = "ToASCII"]
    fn to_ascii(label: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::bytes(to_ascii_rust(&label)?))
    }

    #[py_name = "ToUnicode"]
    fn to_unicode(label: PyValueRef<'_>) -> Result<Value> {
        Ok(Value::string(to_unicode_val(_interp, label.0)?))
    }

    fn encode(
        input: PyStr<'_>,
        #[default("strict".into())] errors: PyStr<'_>,
    ) -> Result<Value> {
        let _ = _interp;
        let (out, len) = idna_encode_rust(&input, &errors)?;
        Ok(Value::tuple(vec![Value::bytes(out), Value::int(len as i64)]))
    }

    fn decode(
        input: PyValueRef<'_>,
        #[default("strict".into())] errors: PyStr<'_>,
    ) -> Result<Value> {
        let (out, len) = idna_decode_rust(_interp, input.0, &errors)?;
        Ok(Value::tuple(vec![Value::string(out), Value::int(len as i64)]))
    }

    class Codec {
        fn encode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyStr<'_>,
            #[default("strict".into())] errors: PyStr<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            let (out, len) = idna_encode_rust(&input, &errors)?;
            Ok(Value::tuple(vec![Value::bytes(out), Value::int(len as i64)]))
        }

        fn decode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyValueRef<'_>,
            #[default("strict".into())] errors: PyStr<'_>,
        ) -> Result<Value> {
            let (out, len) = idna_decode_rust(_interp, input.0, &errors)?;
            Ok(Value::tuple(vec![Value::string(out), Value::int(len as i64)]))
        }
    }

    class IncrementalEncoder {
        fn _buffer_encode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyStr<'_>,
            errors: PyStr<'_>,
            final_flag: PyBool,
        ) -> Result<Value> {
            let _ = _interp;
            let (out, consumed) = idna_buffer_encode_rust(&input, &errors, final_flag.0)?;
            Ok(Value::tuple(vec![
                Value::bytes(out),
                Value::int(consumed as i64),
            ]))
        }
    }

    class IncrementalDecoder {
        fn _buffer_decode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyValueRef<'_>,
            errors: PyStr<'_>,
            final_flag: PyBool,
        ) -> Result<Value> {
            let (out, consumed) = idna_buffer_decode_rust(_interp, input.0, &errors, final_flag.0)?;
            Ok(Value::tuple(vec![
                Value::string(out),
                Value::int(consumed as i64),
            ]))
        }
    }

    class StreamWriter {
        fn encode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyStr<'_>,
            #[default("strict".into())] errors: PyStr<'_>,
        ) -> Result<Value> {
            let _ = _interp;
            let (out, len) = idna_encode_rust(&input, &errors)?;
            Ok(Value::tuple(vec![Value::bytes(out), Value::int(len as i64)]))
        }
    }

    class StreamReader {
        fn decode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyValueRef<'_>,
            #[default("strict".into())] errors: PyStr<'_>,
        ) -> Result<Value> {
            let (out, len) = idna_decode_rust(_interp, input.0, &errors)?;
            Ok(Value::tuple(vec![Value::string(out), Value::int(len as i64)]))
        }
    }

    fn getregentry() -> Result<Value> {
        build_codec_info(_interp)
    }
}
