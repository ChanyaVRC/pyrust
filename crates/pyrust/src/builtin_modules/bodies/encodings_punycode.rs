// `encodings.punycode` module — 100% native-Rust implementation of Python 3.12's
// RFC 3492 Punycode codec (issue #3250).
//
// Exposes all RFC 3492 helpers (`segregate`, `selective_len`, `selective_find`,
// `insertion_unsort`, `T`, `digits`, `generate_generalized_integer`, `adapt`,
// `generate_integers`, `punycode_encode`, `decode_generalized_number`,
// `insertion_sort`, `punycode_decode`) and the full Codec API (`encode`,
// `decode`, `Codec`, `IncrementalEncoder`, `IncrementalDecoder`, `StreamWriter`,
// `StreamReader`, `getregentry`).
//
// Reference: <https://datatracker.ietf.org/doc/html/rfc3492>

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use pyrust_core::{cesu8_codepoints, cesu8_encode_codepoint};
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, PyBool, PyBytesLike, PyIndex, PyStr, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyModule, Value, ValueKind};

const DIGITS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

#[inline]
fn punycode_t(j: i64, bias: i64) -> i64 {
    let res = 36 * (j + 1) - bias;
    res.clamp(1, 26)
}

fn generate_generalized_integer_rust(mut n: u64, bias: i64) -> Vec<u8> {
    let mut result = Vec::new();
    let mut j = 0i64;
    loop {
        let t = punycode_t(j, bias) as u64;
        if n < t {
            result.push(DIGITS[n as usize]);
            return result;
        }
        let base_minus_t = 36 - t;
        result.push(DIGITS[(t + ((n - t) % base_minus_t)) as usize]);
        n = (n - t) / base_minus_t;
        j += 1;
    }
}

fn adapt_rust(mut delta: i64, first: bool, numchars: i64) -> i64 {
    if first {
        delta /= 700;
    } else {
        delta /= 2;
    }
    delta += delta / numchars;
    let mut divisions = 0i64;
    while delta > 455 {
        delta /= 35;
        divisions += 36;
    }
    divisions + (36 * delta / (delta + 38))
}

fn segregate_rust(text: &str) -> (Vec<u8>, Vec<u32>) {
    let mut base = Vec::new();
    let mut extended_set = BTreeSet::new();
    for cp in cesu8_codepoints(text) {
        if cp < 128 {
            base.push(cp as u8);
        } else {
            extended_set.insert(cp);
        }
    }
    (base, extended_set.into_iter().collect())
}

fn selective_len_rust(cps: &[u32], max_cp: u32) -> usize {
    cps.iter().filter(|&&cp| cp < max_cp).count()
}

fn selective_find_rust(cps: &[u32], target: u32, mut index: i64, mut pos: i64) -> (i64, i64) {
    let len = cps.len() as i64;
    loop {
        pos += 1;
        if pos >= len {
            return (-1, -1);
        }
        let c = cps[pos as usize];
        if c == target {
            return (index + 1, pos);
        } else if c < target {
            index += 1;
        }
    }
}

fn insertion_unsort_rust(cps: &[u32], extended: &[u32]) -> Vec<u64> {
    let mut oldchar: u64 = 0x80;
    let mut result = Vec::new();
    let mut oldindex: i64 = -1;
    for &c in extended {
        let mut index: i64 = -1;
        let mut pos: i64 = -1;
        let char_val = u64::from(c);
        let curlen = selective_len_rust(cps, c) as u64;
        let mut delta: i128 = i128::from((curlen + 1) * (char_val - oldchar));
        loop {
            let (next_idx, next_pos) = selective_find_rust(cps, c, index, pos);
            index = next_idx;
            pos = next_pos;
            if index == -1 {
                break;
            }
            delta += i128::from(index - oldindex);
            result.push((delta - 1) as u64);
            oldindex = index;
            delta = 0;
        }
        oldchar = char_val;
    }
    result
}

fn generate_integers_rust(baselen: usize, deltas: &[u64]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut bias: i64 = 72;
    for (points, &delta) in deltas.iter().enumerate() {
        result.extend_from_slice(&generate_generalized_integer_rust(delta, bias));
        bias = adapt_rust(delta as i64, points == 0, (baselen + points + 1) as i64);
    }
    result
}

/// Pure-Rust RFC 3492 Punycode encoder (shared with `encodings.idna`).
pub(crate) fn punycode_encode_rust(text: &str) -> Vec<u8> {
    let cps: Vec<u32> = cesu8_codepoints(text).collect();
    let (base, extended_cps) = segregate_rust(text);
    let deltas = insertion_unsort_rust(&cps, &extended_cps);
    let extended = generate_integers_rust(base.len(), &deltas);
    if !base.is_empty() {
        let mut out = Vec::with_capacity(base.len() + 1 + extended.len());
        out.extend_from_slice(&base);
        out.push(b'-');
        out.extend_from_slice(&extended);
        out
    } else {
        extended
    }
}

fn decode_generalized_number_rust(
    extended: &[u32],
    mut extpos: usize,
    bias: i64,
    errors: &str,
) -> Result<(usize, Option<u64>)> {
    let mut result: u128 = 0;
    let mut w: u128 = 1;
    let mut j: i64 = 0;
    loop {
        if extpos >= extended.len() {
            if errors == "strict" {
                return Err(PyError::named("UnicodeError", "incomplete punicode string"));
            }
            return Ok((extpos + 1, None));
        }
        let ch = extended[extpos];
        extpos += 1;
        let digit: u128 = if (0x41..=0x5A).contains(&ch) {
            u128::from(ch - 0x41)
        } else if (0x30..=0x39).contains(&ch) {
            u128::from(ch - 22)
        } else if errors == "strict" {
            let rendered = cesu8_encode_codepoint(ch);
            return Err(PyError::named(
                "UnicodeError",
                format!("Invalid extended code point '{rendered}'"),
            ));
        } else {
            return Ok((extpos, None));
        };
        let t = punycode_t(j, bias) as u128;
        result = result.saturating_add(digit.saturating_mul(w));
        if digit < t {
            return Ok((extpos, Some(result.min(u64::MAX as u128) as u64)));
        }
        w = w.saturating_mul(36 - t);
        j += 1;
    }
}

fn insertion_sort_rust(base_str: &str, extended_str: &str, errors: &str) -> Result<String> {
    let mut base: Vec<u32> = cesu8_codepoints(base_str).collect();
    let extended: Vec<u32> = cesu8_codepoints(extended_str).collect();
    let mut ch: u128 = 0x80;
    let mut pos: i128 = -1;
    let mut bias: i64 = 72;
    let mut extpos: usize = 0;
    while extpos < extended.len() {
        let (newpos, delta_opt) = decode_generalized_number_rust(&extended, extpos, bias, errors)?;
        let Some(delta) = delta_opt else {
            break;
        };
        pos += i128::from(delta) + 1;
        let blen = (base.len() + 1) as i128;
        ch += (pos / blen) as u128;
        if ch > 0x10FFFF {
            if errors == "strict" {
                return Err(PyError::named(
                    "UnicodeError",
                    format!("Invalid character U+{ch:x}"),
                ));
            }
            ch = u128::from(b'?');
        }
        pos %= blen;
        base.insert(pos as usize, ch as u32);
        bias = adapt_rust(delta as i64, extpos == 0, base.len() as i64);
        extpos = newpos;
    }
    let mut out = String::with_capacity(base.len());
    for cp in base {
        out.push_str(&cesu8_encode_codepoint(cp));
    }
    Ok(out)
}

/// Pure-Rust RFC 3492 Punycode decoder (shared with `encodings.idna`).
pub(crate) fn punycode_decode_bytes_rust(bytes: &[u8], errors: &str) -> Result<String> {
    let pos = bytes.iter().rposition(|&b| b == b'-');
    let (base, extended) = match pos {
        None => {
            let ext_val = pyrust_builtins::bytes::decode_bytes(bytes, "ascii", "strict")?;
            let ext_s = ext_val.as_str().unwrap_or("").to_ascii_uppercase();
            (String::new(), ext_s)
        }
        Some(idx) => {
            let base_val = pyrust_builtins::bytes::decode_bytes(&bytes[..idx], "ascii", errors)?;
            let ext_val =
                pyrust_builtins::bytes::decode_bytes(&bytes[idx + 1..], "ascii", "strict")?;
            let base_s = base_val.as_str().unwrap_or("").to_string();
            let ext_s = ext_val.as_str().unwrap_or("").to_ascii_uppercase();
            (base_s, ext_s)
        }
    };
    insertion_sort_rust(&base, &extended, errors)
}

fn extract_punycode_decode_input(
    interp: &mut Interpreter,
    val: &Value,
) -> Result<(Vec<u8>, usize)> {
    if let Some(s) = val.as_str() {
        let len = cesu8_codepoints(s).count();
        let encoded = pyrust_builtins::string::encode_str_to_bytes(s, "ascii", "strict")?;
        if let ValueKind::Bytes(b) = encoded.kind() {
            return Ok((b.as_ref().clone(), len));
        }
    }
    let bytes = PyBytesLike::try_from_value_with_interp(interp, val, "punycode_decode", "text")?;
    let vec = bytes.into_vec();
    let len = vec.len();
    Ok((vec, len))
}

fn check_punycode_errors(errors: &str) -> Result<()> {
    if !matches!(errors, "strict" | "replace" | "ignore") {
        return Err(PyError::named(
            "UnicodeError",
            format!("Unsupported error handling {errors}"),
        ));
    }
    Ok(())
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
    let self_mod = interp.load_module("encodings.punycode")?;
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
                value: Value::string("punycode"),
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
    let codecs_mod = interp.load_module("codecs")?;
    let codecs_codec = interp.get_attr(&codecs_mod, "Codec")?;
    let codecs_inc_enc = interp.get_attr(&codecs_mod, "IncrementalEncoder")?;
    let codecs_inc_dec = interp.get_attr(&codecs_mod, "IncrementalDecoder")?;
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
        "digits" => Value::bytes(DIGITS.to_vec()),
    }

    fn segregate(str_arg: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        let (base, ext) = segregate_rust(&str_arg);
        let ext_list: Vec<Value> = ext
            .into_iter()
            .map(|cp| Value::string(cesu8_encode_codepoint(cp)))
            .collect();
        Ok(Value::tuple(vec![Value::bytes(base), Value::list(ext_list)]))
    }

    fn selective_len(str_arg: PyStr<'_>, max_val: PyIndex) -> Result<Value> {
        let _ = _interp;
        let max_cp = max_val.expect_i64(FN_NAME, "max")?;
        let count = cesu8_codepoints(&str_arg)
            .filter(|&cp| i64::from(cp) < max_cp)
            .count();
        Ok(Value::int(count as i64))
    }

    fn selective_find(
        str_arg: PyStr<'_>,
        ch: PyStr<'_>,
        index: PyIndex,
        pos: PyIndex,
    ) -> Result<Value> {
        let _ = _interp;
        let cps: Vec<u32> = cesu8_codepoints(&str_arg).collect();
        let target = cesu8_codepoints(&ch)
            .next()
            .ok_or_else(|| PyError::named("TypeError", "expected single character"))?;
        let idx = index.expect_i64(FN_NAME, "index")?;
        let p = pos.expect_i64(FN_NAME, "pos")?;
        let (r_idx, r_pos) = selective_find_rust(&cps, target, idx, p);
        Ok(Value::tuple(vec![Value::int(r_idx), Value::int(r_pos)]))
    }

    fn insertion_unsort(str_arg: PyStr<'_>, extended: PyValueRef<'_>) -> Result<Value> {
        let cps: Vec<u32> = cesu8_codepoints(&str_arg).collect();
        let ext_items = _interp.collect_iterable(extended.0)?;
        let mut ext_cps = Vec::with_capacity(ext_items.len());
        for item in ext_items {
            let s = item.as_str().ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    format!("expected str element, got {}", value_type_name_str(&item)),
                )
            })?;
            let cp = cesu8_codepoints(s)
                .next()
                .ok_or_else(|| PyError::named("TypeError", "expected non-empty char"))?;
            ext_cps.push(cp);
        }
        let deltas = insertion_unsort_rust(&cps, &ext_cps);
        Ok(Value::list(
            deltas.into_iter().map(|d| Value::int(d as i64)).collect(),
        ))
    }

    #[py_name = "T"]
    fn t_threshold(j: PyIndex, bias: PyIndex) -> Result<Value> {
        let _ = _interp;
        let j_val = j.expect_i64(FN_NAME, "j")?;
        let bias_val = bias.expect_i64(FN_NAME, "bias")?;
        Ok(Value::int(punycode_t(j_val, bias_val)))
    }

    #[allow(non_snake_case)]
    fn generate_generalized_integer(N: PyIndex, bias: PyIndex) -> Result<Value> {
        let _ = _interp;
        let n_val = N.expect_i64(FN_NAME, "N")?;
        let bias_val = bias.expect_i64(FN_NAME, "bias")?;
        Ok(Value::bytes(generate_generalized_integer_rust(
            n_val.max(0) as u64,
            bias_val,
        )))
    }

    fn adapt(delta: PyIndex, first: PyBool, numchars: PyIndex) -> Result<Value> {
        let _ = _interp;
        let d = delta.expect_i64(FN_NAME, "delta")?;
        let nc = numchars.expect_i64(FN_NAME, "numchars")?;
        Ok(Value::int(adapt_rust(d, first.0, nc)))
    }

    fn generate_integers(baselen: PyIndex, deltas: PyValueRef<'_>) -> Result<Value> {
        let bl = baselen.expect_i64(FN_NAME, "baselen")?.max(0) as usize;
        let items = _interp.collect_iterable(deltas.0)?;
        let mut ds = Vec::with_capacity(items.len());
        for item in items {
            let d = item
                .as_int()
                .ok_or_else(|| PyError::named("TypeError", "delta must be int"))?;
            ds.push(d.max(0) as u64);
        }
        Ok(Value::bytes(generate_integers_rust(bl, &ds)))
    }

    fn punycode_encode(text: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        Ok(Value::bytes(punycode_encode_rust(&text)))
    }

    fn decode_generalized_number(
        extended: PyStr<'_>,
        extpos: PyIndex,
        bias: PyIndex,
        errors: PyStr<'_>,
    ) -> Result<Value> {
        let _ = _interp;
        let cps: Vec<u32> = cesu8_codepoints(&extended).collect();
        let pos = extpos.expect_i64(FN_NAME, "extpos")?.max(0) as usize;
        let b = bias.expect_i64(FN_NAME, "bias")?;
        let (newpos, val) = decode_generalized_number_rust(&cps, pos, b, &errors)?;
        Ok(Value::tuple(vec![
            Value::int(newpos as i64),
            match val {
                Some(v) => Value::int(v as i64),
                None => Value::none(),
            },
        ]))
    }

    fn insertion_sort(base: PyStr<'_>, extended: PyStr<'_>, errors: PyStr<'_>) -> Result<Value> {
        let _ = _interp;
        let res = insertion_sort_rust(&base, &extended, &errors)?;
        Ok(Value::string(res))
    }

    fn punycode_decode(text: PyValueRef<'_>, errors: PyStr<'_>) -> Result<Value> {
        let (bytes, _) = extract_punycode_decode_input(_interp, text.0)?;
        let res = punycode_decode_bytes_rust(&bytes, &errors)?;
        Ok(Value::string(res))
    }

    fn encode(
        input: PyStr<'_>,
        #[default("strict".into())] errors: PyStr<'_>,
    ) -> Result<Value> {
        let _ = (_interp, errors);
        let s: &str = &input;
        let res = punycode_encode_rust(s);
        let len = cesu8_codepoints(s).count() as i64;
        Ok(Value::tuple(vec![Value::bytes(res), Value::int(len)]))
    }

    fn decode(
        input: PyValueRef<'_>,
        #[default("strict".into())] errors: PyStr<'_>,
    ) -> Result<Value> {
        check_punycode_errors(&errors)?;
        let (bytes, len) = extract_punycode_decode_input(_interp, input.0)?;
        let res = punycode_decode_bytes_rust(&bytes, &errors)?;
        Ok(Value::tuple(vec![Value::string(res), Value::int(len as i64)]))
    }

    class Codec {
        fn encode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyStr<'_>,
            #[default("strict".into())] errors: PyStr<'_>,
        ) -> Result<Value> {
            let _ = (_interp, errors);
            let s: &str = &input;
            let res = punycode_encode_rust(s);
            let len = cesu8_codepoints(s).count() as i64;
            Ok(Value::tuple(vec![Value::bytes(res), Value::int(len)]))
        }

        fn decode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyValueRef<'_>,
            #[default("strict".into())] errors: PyStr<'_>,
        ) -> Result<Value> {
            check_punycode_errors(&errors)?;
            let (bytes, len) = extract_punycode_decode_input(_interp, input.0)?;
            let res = punycode_decode_bytes_rust(&bytes, &errors)?;
            Ok(Value::tuple(vec![Value::string(res), Value::int(len as i64)]))
        }
    }

    class IncrementalEncoder {
        fn encode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyStr<'_>,
            #[default(false.into())] final_flag: PyBool,
        ) -> Result<Value> {
            let _ = (_interp, final_flag);
            Ok(Value::bytes(punycode_encode_rust(&input)))
        }
    }

    class IncrementalDecoder {
        fn decode(
            #[positional_only] self_val: PyValueRef<'_>,
            input: PyValueRef<'_>,
            #[default(false.into())] final_flag: PyBool,
        ) -> Result<Value> {
            let _ = final_flag;
            let err_val = _interp.get_attr(self_val.0, "errors")?;
            let errors = err_val.as_str().unwrap_or("strict").to_string();
            check_punycode_errors(&errors)?;
            let (bytes, _) = extract_punycode_decode_input(_interp, input.0)?;
            let res = punycode_decode_bytes_rust(&bytes, &errors)?;
            Ok(Value::string(res))
        }
    }

    class StreamWriter {
        fn encode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyStr<'_>,
            #[default("strict".into())] errors: PyStr<'_>,
        ) -> Result<Value> {
            let _ = (_interp, errors);
            let s: &str = &input;
            let res = punycode_encode_rust(s);
            let len = cesu8_codepoints(s).count() as i64;
            Ok(Value::tuple(vec![Value::bytes(res), Value::int(len)]))
        }
    }

    class StreamReader {
        fn decode(
            #[positional_only] _self: PyValueRef<'_>,
            input: PyValueRef<'_>,
            #[default("strict".into())] errors: PyStr<'_>,
        ) -> Result<Value> {
            check_punycode_errors(&errors)?;
            let (bytes, len) = extract_punycode_decode_input(_interp, input.0)?;
            let res = punycode_decode_bytes_rust(&bytes, &errors)?;
            Ok(Value::tuple(vec![Value::string(res), Value::int(len as i64)]))
        }
    }

    fn getregentry() -> Result<Value> {
        build_codec_info(_interp)
    }
}
