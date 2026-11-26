// `email.charset` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Character set registry and RFC 2045/2047 encoding rules (`Charset`,
// `add_charset`, `add_alias`, `add_codec`, `QP`, `BASE64`, `SHORTEST`,
// `CHARSETS`, `ALIASES`, `CODEC_MAP`).
//
// Reference: <https://docs.python.org/3/library/email.charset.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBytesLike, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

const QP: i64 = 1;
const BASE64: i64 = 2;
const SHORTEST: i64 = 3;
const RFC2047_CHROME_LEN: usize = 7;
const DEFAULT_CHARSET: &str = "us-ascii";
const UNKNOWN8BIT: &str = "unknown-8bit";

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

type CharsetSpec = (&'static str, Option<i64>, Option<i64>, Option<&'static str>);

fn default_charsets_dict() -> Value {
    let mut d = PyDict::default();
    let entries: &[CharsetSpec] = &[
        ("iso-8859-1", Some(QP), Some(QP), None),
        ("iso-8859-2", Some(QP), Some(QP), None),
        ("iso-8859-3", Some(QP), Some(QP), None),
        ("iso-8859-4", Some(QP), Some(QP), None),
        ("iso-8859-9", Some(QP), Some(QP), None),
        ("iso-8859-10", Some(QP), Some(QP), None),
        ("iso-8859-13", Some(QP), Some(QP), None),
        ("iso-8859-14", Some(QP), Some(QP), None),
        ("iso-8859-15", Some(QP), Some(QP), None),
        ("iso-8859-16", Some(QP), Some(QP), None),
        ("windows-1252", Some(QP), Some(QP), None),
        ("viscii", Some(QP), Some(QP), None),
        ("us-ascii", None, None, None),
        ("big5", Some(BASE64), Some(BASE64), None),
        ("gb2312", Some(BASE64), Some(BASE64), None),
        ("euc-jp", Some(BASE64), None, Some("iso-2022-jp")),
        ("shift_jis", Some(BASE64), None, Some("iso-2022-jp")),
        ("iso-2022-jp", Some(BASE64), None, None),
        ("koi8-r", Some(BASE64), Some(BASE64), None),
        ("utf-8", Some(SHORTEST), Some(BASE64), Some("utf-8")),
    ];
    for &(name, henc, benc, conv) in entries {
        let tup = Value::tuple(vec![
            henc.map(Value::int).unwrap_or_else(Value::none),
            benc.map(Value::int).unwrap_or_else(Value::none),
            conv.map(Value::string).unwrap_or_else(Value::none),
        ]);
        d.insert(PyKey::str_from(name), tup);
    }
    Value::dict(d)
}

fn default_aliases_dict() -> Value {
    let mut d = PyDict::default();
    let entries: &[(&str, &str)] = &[
        ("latin_1", "iso-8859-1"),
        ("latin-1", "iso-8859-1"),
        ("latin_2", "iso-8859-2"),
        ("latin-2", "iso-8859-2"),
        ("latin_3", "iso-8859-3"),
        ("latin-3", "iso-8859-3"),
        ("latin_4", "iso-8859-4"),
        ("latin-4", "iso-8859-4"),
        ("latin_5", "iso-8859-9"),
        ("latin-5", "iso-8859-9"),
        ("latin_6", "iso-8859-10"),
        ("latin-6", "iso-8859-10"),
        ("latin_7", "iso-8859-13"),
        ("latin-7", "iso-8859-13"),
        ("latin_8", "iso-8859-14"),
        ("latin-8", "iso-8859-14"),
        ("latin_9", "iso-8859-15"),
        ("latin-9", "iso-8859-15"),
        ("latin_10", "iso-8859-16"),
        ("latin-10", "iso-8859-16"),
        ("cp949", "ks_c_5601-1987"),
        ("euc_jp", "euc-jp"),
        ("euc_kr", "euc-kr"),
        ("ascii", "us-ascii"),
    ];
    for &(k, v) in entries {
        d.insert(PyKey::str_from(k), Value::string(v));
    }
    Value::dict(d)
}

fn default_codec_map_dict() -> Value {
    let mut d = PyDict::default();
    d.insert(PyKey::str_from("gb2312"), Value::string("eucgb2312_cn"));
    d.insert(PyKey::str_from("big5"), Value::string("big5_tw"));
    d.insert(PyKey::str_from("us-ascii"), Value::none());
    Value::dict(d)
}

fn normalize_python_codec(codec: &str) -> &str {
    if codec.eq_ignore_ascii_case("us-ascii") || codec.eq_ignore_ascii_case("us_ascii") {
        "ascii"
    } else {
        codec
    }
}

fn encode_string_with_codec(interp: &mut Interpreter, s: &str, codec: &str) -> Result<Vec<u8>> {
    let s_val = Value::string(s);
    let enc_fn = interp.get_attr(&s_val, "encode")?;
    let b_val = if codec == UNKNOWN8BIT {
        call_pos(
            interp,
            enc_fn,
            &[Value::string("ascii"), Value::string("surrogateescape")],
        )?
    } else {
        call_pos(
            interp,
            enc_fn,
            &[Value::string(normalize_python_codec(codec))],
        )?
    };
    let bl = PyBytesLike::try_from_value_with_interp(interp, &b_val, "_encode", "string")?;
    Ok(bl.as_bytes().to_vec())
}

fn get_module_dict(interp: &mut Interpreter, attr: &str) -> Result<Value> {
    let m = interp.load_module("email.charset")?;
    interp.get_attr(&m, attr)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderEncoderKind {
    Base64,
    Qp,
    None,
}

fn pick_header_encoder(header_encoding: &Value, header_bytes: &[u8]) -> HeaderEncoderKind {
    match header_encoding.as_int() {
        Some(BASE64) => HeaderEncoderKind::Base64,
        Some(QP) => HeaderEncoderKind::Qp,
        Some(SHORTEST) => {
            let len64 = super::email_base64mime::b64_header_length_usize(header_bytes.len());
            let lenqp = super::email_quoprimime::qp_header_length_bytes(header_bytes);
            if len64 < lenqp {
                HeaderEncoderKind::Base64
            } else {
                HeaderEncoderKind::Qp
            }
        }
        _ => HeaderEncoderKind::None,
    }
}

fn all_list() -> Value {
    Value::list(vec![
        Value::string("Charset"),
        Value::string("add_alias"),
        Value::string("add_charset"),
        Value::string("add_codec"),
    ])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
        "__all__" => all_list(),
        "QP" => Value::int(QP),
        "BASE64" => Value::int(BASE64),
        "SHORTEST" => Value::int(SHORTEST),
        "RFC2047_CHROME_LEN" => Value::int(RFC2047_CHROME_LEN as i64),
        "DEFAULT_CHARSET" => Value::string(DEFAULT_CHARSET),
        "UNKNOWN8BIT" => Value::string(UNKNOWN8BIT),
        "EMPTYSTRING" => Value::string(""),
        "CHARSETS" => default_charsets_dict(),
        "ALIASES" => default_aliases_dict(),
        "CODEC_MAP" => default_codec_map_dict(),
    }

    fn add_charset(
        charset: PyStr<'_>,
        #[default(OptionalArg::Omitted)] header_enc: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] body_enc: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] output_charset: OptValueRef<'_>,
    ) -> Result<Value> {
        let henc = header_enc.as_value().cloned().unwrap_or_else(Value::none);
        let benc = body_enc.as_value().cloned().unwrap_or_else(Value::none);
        let ocs = output_charset
            .as_value()
            .cloned()
            .unwrap_or_else(Value::none);
        if benc.as_int() == Some(SHORTEST) {
            return Err(PyError::named(
                "ValueError",
                "SHORTEST not allowed for body_enc".to_string(),
            ));
        }
        let charsets = get_module_dict(_interp, "CHARSETS")?;
        charsets.dict_insert(
            PyKey::str_from(charset.0.as_ref()),
            Value::tuple(vec![henc, benc, ocs]),
        )?;
        Ok(Value::none())
    }

    fn add_alias(alias: PyStr<'_>, canonical: PyStr<'_>) -> Result<Value> {
        let aliases = get_module_dict(_interp, "ALIASES")?;
        aliases.dict_insert(
            PyKey::str_from(alias.0.as_ref()),
            Value::string(canonical.0.as_ref()),
        )?;
        Ok(Value::none())
    }

    fn add_codec(charset: PyStr<'_>, codecname: PyValue) -> Result<Value> {
        let codec_map = get_module_dict(_interp, "CODEC_MAP")?;
        codec_map.dict_insert(PyKey::str_from(charset.0.as_ref()), codecname.0)?;
        Ok(Value::none())
    }

    fn _encode(string: PyStr<'_>, codec: PyStr<'_>) -> Result<Value> {
        let bytes = encode_string_with_codec(_interp, string.0.as_ref(), codec.0.as_ref())?;
        Ok(Value::bytes(bytes))
    }

    class Charset {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] input_charset: OptValueRef<'_>,
        ) -> Result<Value> {
            let raw_cs = input_charset
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::string(DEFAULT_CHARSET));
            let cs_str = if let Some(s) = raw_cs.as_str() {
                if !s.is_ascii() {
                    return Err(super::email_errors::raise_error_val(
                        _interp,
                        "CharsetError",
                        raw_cs,
                    ));
                }
                s.to_ascii_lowercase()
            } else if let Ok(bl) =
                PyBytesLike::try_from_value_with_interp(_interp, &raw_cs, FN_NAME, "input_charset")
            {
                match std::str::from_utf8(bl.as_bytes()) {
                    Ok(s) if s.is_ascii() => s.to_ascii_lowercase(),
                    _ => {
                        return Err(super::email_errors::raise_error_val(
                            _interp,
                            "CharsetError",
                            raw_cs,
                        ));
                    }
                }
            } else {
                // Coerce via str(input_charset, 'ascii')
                let str_cls = Value::py_class(
                    crate::interpreter::primitive_class_by_name("str")
                        .ok_or_else(|| PyError::runtime("str class not found".to_string()))?,
                );
                match call_pos(_interp, str_cls, &[raw_cs.clone(), Value::string("ascii")]) {
                    Ok(v) => v.as_str().unwrap_or("").to_ascii_lowercase(),
                    Err(e) if e.class_name_is("UnicodeDecodeError") || e.class_name_is("UnicodeEncodeError") => {
                        return Err(super::email_errors::raise_error_val(
                            _interp,
                            "CharsetError",
                            raw_cs,
                        ));
                    }
                    Err(e) => return Err(e),
                }
            };

            let aliases = get_module_dict(_interp, "ALIASES")?;
            let charsets = get_module_dict(_interp, "CHARSETS")?;
            let codec_map = get_module_dict(_interp, "CODEC_MAP")?;

            let canonical_input = aliases
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&cs_str)).cloned())
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or(cs_str);

            let (henc, benc, conv) = if let Some(tup) = charsets
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&canonical_input)).cloned())
            {
                let items: Vec<Value> = match tup.kind() {
                    ValueKind::Tuple(t) => t.to_vec(),
                    _ => Vec::new(),
                };
                let h = items.first().cloned().unwrap_or_else(Value::none);
                let b = items.get(1).cloned().unwrap_or_else(Value::none);
                let c = items.get(2).cloned().unwrap_or_else(Value::none);
                (h, b, c)
            } else {
                (Value::int(SHORTEST), Value::int(BASE64), Value::none())
            };

            let conv_str = if conv.is_none() || conv.as_str() == Some("") {
                canonical_input.clone()
            } else {
                conv.as_str().unwrap_or(&canonical_input).to_string()
            };

            let output_charset = aliases
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&conv_str)).cloned())
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or(conv_str);

            let input_codec = codec_map
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&canonical_input)).cloned())
                .unwrap_or_else(|| Value::string(&canonical_input));

            let output_codec = codec_map
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&output_charset)).cloned())
                .unwrap_or_else(|| Value::string(&output_charset));

            _interp.assign_attr(
                self_val.0.clone(),
                "input_charset",
                Value::string(&canonical_input),
            )?;
            _interp.assign_attr(self_val.0.clone(), "header_encoding", henc)?;
            _interp.assign_attr(self_val.0.clone(), "body_encoding", benc)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "output_charset",
                Value::string(&output_charset),
            )?;
            _interp.assign_attr(self_val.0.clone(), "input_codec", input_codec)?;
            _interp.assign_attr(self_val.0.clone(), "output_codec", output_codec)?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ic = _interp.get_attr(self_val.0, "input_charset")?;
            let s = ic.as_str().unwrap_or("").to_ascii_lowercase();
            Ok(Value::string(s))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ic = _interp.get_attr(self_val.0, "input_charset")?;
            let s = ic.as_str().unwrap_or("").to_ascii_lowercase();
            Ok(Value::string(s))
        }

        fn __eq__(
            #[positional_only] self_val: PyValueRef<'_>,
            other: PyValueRef<'_>,
        ) -> Result<Value> {
            let lhs = super::email_errors::py_str(_interp, self_val.0)?;
            let rhs = super::email_errors::py_str(_interp, other.0)?.to_lowercase();
            Ok(Value::bool_(lhs == rhs))
        }

        fn get_body_encoding(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let benc = _interp.get_attr(self_val.0, "body_encoding")?;
            if benc.as_int() == Some(SHORTEST) {
                return Err(PyError::named(
                    "AssertionError",
                    "SHORTEST not allowed for body_encoding".to_string(),
                ));
            }
            match benc.as_int() {
                Some(QP) => Ok(Value::string("quoted-printable")),
                Some(BASE64) => Ok(Value::string("base64")),
                _ => {
                    let enc_mod = _interp.load_module("email.encoders")?;
                    _interp.get_attr(&enc_mod, "encode_7or8bit")
                }
            }
        }

        fn get_output_charset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ocs = _interp.get_attr(self_val.0, "output_charset")?;
            if _interp.truthy_value(&ocs)? {
                Ok(ocs)
            } else {
                _interp.get_attr(self_val.0, "input_charset")
            }
        }

        fn header_encode(
            #[positional_only] self_val: PyValueRef<'_>,
            string: PyStr<'_>,
        ) -> Result<Value> {
            let oc = _interp.get_attr(self_val.0, "output_codec")?;
            let codec = oc
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("us-ascii")
                .to_string();
            let header_bytes = encode_string_with_codec(_interp, string.0.as_ref(), &codec)?;
            let henc = _interp.get_attr(self_val.0, "header_encoding")?;
            match pick_header_encoder(&henc, &header_bytes) {
                HeaderEncoderKind::None => Ok(Value::string(string.0.as_ref())),
                HeaderEncoderKind::Base64 => {
                    if header_bytes.is_empty() {
                        Ok(Value::string(""))
                    } else {
                        let enc = super::email_base64mime::b64_encode_bytes(&header_bytes);
                        Ok(Value::string(format!("=?{codec}?b?{enc}?=")))
                    }
                }
                HeaderEncoderKind::Qp => Ok(Value::string(
                    super::email_quoprimime::qp_header_encode_bytes(&header_bytes, &codec),
                )),
            }
        }

        fn header_encode_lines(
            #[positional_only] self_val: PyValueRef<'_>,
            string: PyStr<'_>,
            maxlengths: PyValue,
        ) -> Result<Value> {
            let oc = _interp.get_attr(self_val.0, "output_codec")?;
            let codec = oc
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("us-ascii")
                .to_string();
            let header_bytes = encode_string_with_codec(_interp, string.0.as_ref(), &codec)?;
            let henc = _interp.get_attr(self_val.0, "header_encoding")?;
            let kind = pick_header_encoder(&henc, &header_bytes);

            let get_ocs = _interp.get_attr(self_val.0, "get_output_charset")?;
            let ocs_val = call_pos(_interp, get_ocs, &[])?;
            let charset = ocs_val.as_str().unwrap_or("us-ascii").to_string();
            let extra = (charset.len() + RFC2047_CHROME_LEN) as i64;

            let next_fn = Value::builtin_function("next");
            let mut maxlen = call_pos(_interp, next_fn.clone(), std::slice::from_ref(&maxlengths.0))?
                .as_int()
                .unwrap_or(76)
                - extra;

            let mut lines: Vec<Value> = Vec::new();
            let mut current_line: Vec<char> = Vec::new();

            let encode_chunk = |bytes: &[u8]| -> String {
                match kind {
                    HeaderEncoderKind::Base64 => {
                        if bytes.is_empty() {
                            String::new()
                        } else {
                            let enc = super::email_base64mime::b64_encode_bytes(bytes);
                            format!("=?{codec}?b?{enc}?=")
                        }
                    }
                    _ => super::email_quoprimime::qp_header_encode_bytes(bytes, &codec),
                }
            };

            let measure_chunk = |bytes: &[u8]| -> i64 {
                match kind {
                    HeaderEncoderKind::Base64 => {
                        super::email_base64mime::b64_header_length_usize(bytes.len()) as i64
                    }
                    _ => super::email_quoprimime::qp_header_length_bytes(bytes) as i64,
                }
            };

            for ch in string.0.chars() {
                current_line.push(ch);
                let this_line: String = current_line.iter().collect();
                let this_bytes = encode_string_with_codec(_interp, &this_line, &charset)?;
                let length = measure_chunk(&this_bytes);
                if length > maxlen {
                    current_line.pop();
                    if lines.is_empty() && current_line.is_empty() {
                        lines.push(Value::none());
                    } else {
                        let joined: String = current_line.iter().collect();
                        let hb = encode_string_with_codec(_interp, &joined, &codec)?;
                        lines.push(Value::string(encode_chunk(&hb)));
                    }
                    current_line = vec![ch];
                    maxlen = call_pos(_interp, next_fn.clone(), std::slice::from_ref(&maxlengths.0))?
                        .as_int()
                        .unwrap_or(76)
                        - extra;
                }
            }
            let joined: String = current_line.iter().collect();
            let hb = encode_string_with_codec(_interp, &joined, &codec)?;
            lines.push(Value::string(encode_chunk(&hb)));
            Ok(Value::list(lines))
        }

        fn body_encode(
            #[positional_only] self_val: PyValueRef<'_>,
            string: PyValue,
        ) -> Result<Value> {
            if !_interp.truthy_value(&string.0)? {
                return Ok(string.0);
            }
            let benc = _interp.get_attr(self_val.0, "body_encoding")?;
            let ocs = _interp.get_attr(self_val.0, "output_charset")?;
            let out_cs = ocs.as_str().unwrap_or("us-ascii").to_string();
            match benc.as_int() {
                Some(BASE64) => {
                    let raw = if let Some(s) = string.0.as_str() {
                        encode_string_with_codec(_interp, s, &out_cs)?
                    } else {
                        let bl =
                            PyBytesLike::try_from_value_with_interp(_interp, &string.0, FN_NAME, "string")?;
                        bl.as_bytes().to_vec()
                    };
                    let enc = super::email_base64mime::b64_body_encode_bytes(&raw, 76, "\n")?;
                    Ok(Value::string(enc))
                }
                Some(QP) => {
                    let raw = if let Some(s) = string.0.as_str() {
                        encode_string_with_codec(_interp, s, &out_cs)?
                    } else {
                        let bl =
                            PyBytesLike::try_from_value_with_interp(_interp, &string.0, FN_NAME, "string")?;
                        bl.as_bytes().to_vec()
                    };
                    let latin1: String = raw.into_iter().map(|b| b as char).collect();
                    let enc = super::email_quoprimime::qp_body_encode_str(&latin1, 76, "\n")?;
                    Ok(Value::string(enc))
                }
                _ => {
                    if let Some(s) = string.0.as_str() {
                        let raw = encode_string_with_codec(_interp, s, &out_cs)?;
                        match std::str::from_utf8(&raw) {
                            Ok(ascii) if ascii.is_ascii() => Ok(Value::string(ascii)),
                            _ => Err(PyError::named(
                                "UnicodeDecodeError",
                                "'ascii' codec can't decode byte".to_string(),
                            )),
                        }
                    } else {
                        Ok(string.0)
                    }
                }
            }
        }
    }
}
