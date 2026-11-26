// `codecs` module — body for the `codecs @inject` entry in
// `pyrust_builtin_modules!` (issue #3203).
//
// Provides minimal native helpers for bytes-like buffer extraction,
// surrogate-safe codepoint iteration, and Unicode 15.0.0 character name lookup
// (`namereplace_errors`); the complete Python 3.12 `codecs` public surface
// lives in `codecs_py.py` and is injected by `inject_python_members`.
//
// Reference: <https://docs.python.org/3/library/codecs.html>

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, reject_keyword_args_expanded,
    value_type_name_str,
};
use crate::value::{PyKey, Value, ValueKind};
use pyrust_core::{cesu8_codepoints, cesu8_encode_codepoint};
use pyrust_derive::pyrust_module;

const CODECS_PY_SOURCE: &str = include_str!("codecs_py.py");

const CODECS_NATIVE_HELPERS: [&str; 6] = [
    "_to_bytes",
    "_unicode_name",
    "_str_codepoints",
    "_codepoints_to_str",
    "_utf8_encode_surrogate",
    "_native_encode",
];

const CODECS_PY_EXPORTS: &[&str] = &[
    "BOM",
    "BOM_UTF8",
    "BOM_LE",
    "BOM_BE",
    "BOM_UTF16",
    "BOM_UTF16_LE",
    "BOM_UTF16_BE",
    "BOM_UTF32",
    "BOM_UTF32_LE",
    "BOM_UTF32_BE",
    "BOM32_LE",
    "BOM32_BE",
    "BOM64_LE",
    "BOM64_BE",
    "CodecInfo",
    "Codec",
    "IncrementalEncoder",
    "BufferedIncrementalEncoder",
    "IncrementalDecoder",
    "BufferedIncrementalDecoder",
    "StreamWriter",
    "StreamReader",
    "StreamReaderWriter",
    "StreamRecoder",
    "register",
    "unregister",
    "lookup",
    "encode",
    "decode",
    "getencoder",
    "getdecoder",
    "getincrementalencoder",
    "getincrementaldecoder",
    "getreader",
    "getwriter",
    "iterencode",
    "iterdecode",
    "open",
    "EncodedFile",
    "register_error",
    "lookup_error",
    "strict_errors",
    "ignore_errors",
    "replace_errors",
    "xmlcharrefreplace_errors",
    "backslashreplace_errors",
    "namereplace_errors",
    "make_identity_dict",
    "make_encoding_map",
    "charmap_build",
    "charmap_encode",
    "charmap_decode",
    "escape_encode",
    "escape_decode",
    "readbuffer_encode",
    "ascii_encode",
    "ascii_decode",
    "latin_1_encode",
    "latin_1_decode",
    "utf_8_encode",
    "utf_8_decode",
    "utf_16_encode",
    "utf_16_decode",
    "utf_16_ex_decode",
    "utf_16_le_encode",
    "utf_16_le_decode",
    "utf_16_be_encode",
    "utf_16_be_decode",
    "utf_32_encode",
    "utf_32_decode",
    "utf_32_ex_decode",
    "utf_32_le_encode",
    "utf_32_le_decode",
    "utf_32_be_encode",
    "utf_32_be_decode",
    "raw_unicode_escape_encode",
    "raw_unicode_escape_decode",
    "unicode_escape_encode",
    "unicode_escape_decode",
    "utf_7_encode",
    "utf_7_decode",
];

pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in CODECS_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(CODECS_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("codecs: exec namespace not a dict".into()))?;
    for name in CODECS_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("codecs"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

fn try_extract_bytes_like(val: &Value) -> Option<Vec<u8>> {
    if let ValueKind::Bytes(rc) = val.kind() {
        return Some(rc.as_ref().clone());
    }
    if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
        return Some(snap);
    }
    if pyrust_builtins::memoryview::is_memoryview(val) {
        return pyrust_builtins::memoryview::to_bytes_checked(val).ok();
    }
    if let Some(backing) = builtin_data_backing(val) {
        if let ValueKind::Bytes(rc) = backing.kind() {
            return Some(rc.as_ref().clone());
        }
        if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
            return Some(snap);
        }
    }
    if let Some(snap) = super::array::buffer_snapshot(val) {
        return Some(snap);
    }
    None
}

fn extract_str(val: &Value) -> Option<String> {
    if let Some(s) = val.as_str() {
        return Some(s.to_owned());
    }
    if let Some(backing) = builtin_data_backing(val)
        && let Some(s) = backing.as_str()
    {
        return Some(s.to_owned());
    }
    None
}

pyrust_module! {
    fn _to_bytes(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_to_bytes expects 1 arg".into()));
        }
        let bytes = try_extract_bytes_like(&args[0].value).ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    value_type_name_str(&args[0].value)
                ),
            )
        })?;
        Ok(Value::bytes(bytes))
    }

    fn _unicode_name(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_unicode_name expects 1 arg".into()));
        }
        let cp = args[0]
            .value
            .as_int()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| PyError::named("TypeError", "expected int codepoint".to_string()))?;
        match crate::builtin_modules::unicodedata::unicode15_name(cp) {
            Some(name) => Ok(Value::string(name)),
            None => Ok(Value::none()),
        }
    }

    fn _str_codepoints(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_str_codepoints expects 1 arg".into()));
        }
        let s = extract_str(&args[0].value).ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "expected str, not '{}'",
                    value_type_name_str(&args[0].value)
                ),
            )
        })?;
        let cps: Vec<Value> = cesu8_codepoints(&s).map(|cp| Value::int(i64::from(cp))).collect();
        Ok(Value::list(cps))
    }

    fn _codepoints_to_str(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_codepoints_to_str expects 1 arg".into()));
        }
        let list = args[0]
            .value
            .as_list()
            .ok_or_else(|| PyError::named("TypeError", "expected list of ints".to_string()))?;
        let mut out = String::with_capacity(list.len());
        for item in list.iter() {
            let cp = item
                .as_int()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| PyError::named("ValueError", "invalid codepoint".to_string()))?;
            if let Some(ch) = char::from_u32(cp) {
                out.push(ch);
            } else {
                out.push_str(&cesu8_encode_codepoint(cp));
            }
        }
        Ok(Value::string(out))
    }

    fn _utf8_encode_surrogate(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::runtime("_utf8_encode_surrogate expects 1 arg".into()));
        }
        let cp = args[0]
            .value
            .as_int()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| PyError::named("TypeError", "expected int codepoint".to_string()))?;
        Ok(Value::bytes(cesu8_encode_codepoint(cp).into_bytes()))
    }

    fn _native_encode(args) -> Result<Value> {
        let _ = _interp;
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 3 {
            return Err(PyError::runtime("_native_encode expects 3 args".into()));
        }
        let s = extract_str(&args[0].value).ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "expected str, not '{}'",
                    value_type_name_str(&args[0].value)
                ),
            )
        })?;
        let enc = extract_str(&args[1].value)
            .ok_or_else(|| PyError::named("TypeError", "encoding must be str".to_string()))?;
        let errors = extract_str(&args[2].value)
            .ok_or_else(|| PyError::named("TypeError", "errors must be str".to_string()))?;
        pyrust_builtins::string::encode_str_to_bytes(&s, &enc, &errors)
    }
}
