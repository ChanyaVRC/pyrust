// `array` module — body for the `array @inject` entry in
// `pyrust_builtin_modules!` (issue #2809).
//
// Compact native-endian element encoding/decoding, range validation, IEEE-754
// `f32`/`f64` conversion, platform-native `c_long` / `Py_UNICODE` sizing, and
// in-place `byteswap` live in Rust here so binary layouts match CPython 3.12 on
// both Windows (LLP64, 2-byte `Py_UNICODE`) and Linux/macOS (LP64, 4-byte
// `Py_UNICODE`).
//
// The `array` class (`ArrayType` alias, `typecodes`, `_array_reconstructor`)
// lives in `array_py.py` and is injected onto the module by
// `inject_python_members`.  Internal Rust helper names are removed from the
// module dict after injection so `dir(array)` matches CPython 3.12.
//
// Reference: <https://docs.python.org/3/library/array.html>

use std::cell::RefCell;
use std::ffi::{c_int, c_long, c_longlong, c_short};
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, invoke_class_method,
    lookup_value_special_method, normalize_float_slot_result, reject_keyword_args_expanded,
    value_to_float, value_type_name_str,
};
use crate::value::{PyBigInt, PyKey, PyToPrimitive, Value, ValueKind};
use pyrust_derive::pyrust_module;

/// Python-source definitions for the public `array` surface.
const ARRAY_PY_SOURCE: &str = include_str!("array_py.py");

/// Internal native helpers seeded into the injected exec namespace and removed
/// from `array.__dict__` after injection so `dir(array)` matches CPython.
const ARRAY_NATIVE_HELPERS: [&str; 9] = [
    "_typecode_itemsize",
    "_encode_item",
    "_encode_items",
    "_decode_item",
    "_decode_all",
    "_encode_unicode",
    "_decode_unicode",
    "_byteswap",
    "_as_index",
];

/// Public names exported from `ARRAY_PY_SOURCE` onto the `array` module.
const ARRAY_PY_EXPORTS: [&str; 4] = ["array", "ArrayType", "typecodes", "_array_reconstructor"];

/// Execute `ARRAY_PY_SOURCE` once, wire its closures to the native helpers,
/// remove internal helper names from `dir(array)`, and publish the CPython 3.12
/// `array` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in ARRAY_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(ARRAY_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("array: exec namespace not a dict".into()))?;
    for name in ARRAY_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("array"));
                class.error_name = Some("array.array");
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Typecode metadata ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum TypeCode {
    SignedChar,
    UnsignedChar,
    Unicode,
    SignedShort,
    UnsignedShort,
    SignedInt,
    UnsignedInt,
    SignedLong,
    UnsignedLong,
    SignedLongLong,
    UnsignedLongLong,
    Float32,
    Float64,
}

impl TypeCode {
    fn parse(val: &Value) -> Result<Self> {
        let s = match val.kind() {
            ValueKind::Str(s) => s,
            _ => {
                if let Some(backing) = builtin_data_backing(val)
                    && let ValueKind::Str(s) = backing.kind()
                {
                    return Self::parse_str(s);
                }
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "array() argument 1 must be a unicode character, not {}",
                        value_type_name_str(val)
                    ),
                ));
            }
        };
        Self::parse_str(s)
    }

    fn parse_str(s: &str) -> Result<Self> {
        let mut chars = s.chars();
        let Some(ch) = chars.next() else {
            return Err(PyError::named(
                "TypeError",
                "array() argument 1 must be a unicode character, not str".to_string(),
            ));
        };
        if chars.next().is_some() {
            return Err(PyError::named(
                "TypeError",
                "array() argument 1 must be a unicode character, not str".to_string(),
            ));
        }
        match ch {
            'b' => Ok(Self::SignedChar),
            'B' => Ok(Self::UnsignedChar),
            'u' => Ok(Self::Unicode),
            'h' => Ok(Self::SignedShort),
            'H' => Ok(Self::UnsignedShort),
            'i' => Ok(Self::SignedInt),
            'I' => Ok(Self::UnsignedInt),
            'l' => Ok(Self::SignedLong),
            'L' => Ok(Self::UnsignedLong),
            'q' => Ok(Self::SignedLongLong),
            'Q' => Ok(Self::UnsignedLongLong),
            'f' => Ok(Self::Float32),
            'd' => Ok(Self::Float64),
            _ => Err(PyError::named(
                "ValueError",
                "bad typecode (must be b, B, u, h, H, i, I, l, L, q, Q, f or d)".to_string(),
            )),
        }
    }

    fn itemsize(self) -> usize {
        match self {
            Self::SignedChar | Self::UnsignedChar => 1,
            Self::Unicode => {
                if cfg!(windows) {
                    2
                } else {
                    4
                }
            }
            Self::SignedShort | Self::UnsignedShort => std::mem::size_of::<c_short>(),
            Self::SignedInt | Self::UnsignedInt => std::mem::size_of::<c_int>(),
            Self::SignedLong | Self::UnsignedLong => std::mem::size_of::<c_long>(),
            Self::SignedLongLong | Self::UnsignedLongLong => std::mem::size_of::<c_longlong>(),
            Self::Float32 => std::mem::size_of::<f32>(),
            Self::Float64 => std::mem::size_of::<f64>(),
        }
    }
}

// ── Numeric & character coercions ────────────────────────────────────────────

fn value_to_array_int(interp: &mut Interpreter, val: &Value) -> Result<PyBigInt> {
    match val.kind() {
        ValueKind::Int(n) => Ok(PyBigInt::from(n)),
        ValueKind::Bool(b) => Ok(PyBigInt::from(i64::from(b))),
        ValueKind::BigInt(bi) => Ok(bi.clone()),
        ValueKind::PyInstance(_) | ValueKind::PyClass(_) => {
            if let Some(indexed) = interp.try_value_to_index(val)? {
                match indexed.kind() {
                    ValueKind::Int(n) => Ok(PyBigInt::from(n)),
                    ValueKind::Bool(b) => Ok(PyBigInt::from(i64::from(b))),
                    ValueKind::BigInt(bi) => Ok(bi.clone()),
                    _ => Err(PyError::named(
                        "TypeError",
                        format!(
                            "'{}' object cannot be interpreted as an integer",
                            value_type_name_str(val)
                        ),
                    )),
                }
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "'{}' object cannot be interpreted as an integer",
                        value_type_name_str(val)
                    ),
                ))
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(val)
            ),
        )),
    }
}

fn value_to_array_float(interp: &mut Interpreter, val: &Value) -> Result<f64> {
    if matches!(val.kind(), ValueKind::PyInstance(_) | ValueKind::PyClass(_)) {
        let backing = builtin_data_backing(val);
        if let Some(ref b) = backing
            && matches!(b.kind(), ValueKind::Float(_))
        {
            return value_to_float(b, "array");
        }
        if let Some(method) = lookup_value_special_method(val, "__float__").transpose()? {
            let result = invoke_class_method(interp, method, val.clone(), &[])?;
            return if let Some(normalized) = normalize_float_slot_result(&result) {
                let ValueKind::Float(f) = normalized.kind() else {
                    unreachable!("normalize_float_slot_result guarantees float");
                };
                Ok(f)
            } else {
                Err(PyError::named(
                    "TypeError",
                    format!(
                        "{}.__float__ returned non-float (type {})",
                        value_type_name_str(val),
                        value_type_name_str(&result)
                    ),
                ))
            };
        }
        if let Some(ref b) = backing
            && matches!(
                b.kind(),
                ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_)
            )
        {
            return value_to_float(b, "array");
        }
        if let Some(indexed) = interp.try_value_to_index(val)? {
            return value_to_float(&indexed, "array");
        }
        return Err(PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(val)),
        ));
    }
    match val.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
            value_to_float(val, "array")
        }
        _ => Err(PyError::named(
            "TypeError",
            format!("must be real number, not {}", value_type_name_str(val)),
        )),
    }
}

fn check_signed(bi: &PyBigInt, min: i128, max: i128, label: &str) -> Result<i128> {
    if let Some(v) = bi.to_i128() {
        if v < min {
            return Err(PyError::named(
                "OverflowError",
                format!("{label} is less than minimum"),
            ));
        }
        if v > max {
            return Err(PyError::named(
                "OverflowError",
                format!("{label} is greater than maximum"),
            ));
        }
        return Ok(v);
    }
    if bi.sign() == crate::value::PyBigIntSign::Minus {
        Err(PyError::named(
            "OverflowError",
            format!("{label} is less than minimum"),
        ))
    } else {
        Err(PyError::named(
            "OverflowError",
            format!("{label} is greater than maximum"),
        ))
    }
}

fn check_unsigned(bi: &PyBigInt, max: u128, label: &str) -> Result<u128> {
    if bi.sign() == crate::value::PyBigIntSign::Minus {
        return Err(PyError::named(
            "OverflowError",
            format!("{label} is less than minimum"),
        ));
    }
    if let Some(v) = bi.to_u128()
        && v <= max
    {
        return Ok(v);
    }
    Err(PyError::named(
        "OverflowError",
        format!("{label} is greater than maximum"),
    ))
}

fn encode_single_item(
    interp: &mut Interpreter,
    tc: TypeCode,
    val: &Value,
    out: &mut Vec<u8>,
) -> Result<()> {
    match tc {
        TypeCode::SignedChar => {
            let bi = value_to_array_int(interp, val)?;
            let n =
                check_signed(&bi, i128::from(i8::MIN), i128::from(i8::MAX), "signed char")? as i8;
            out.push(n as u8);
        }
        TypeCode::UnsignedChar => {
            let bi = value_to_array_int(interp, val)?;
            let n = check_unsigned(&bi, u128::from(u8::MAX), "unsigned byte integer")? as u8;
            out.push(n);
        }
        TypeCode::Unicode => {
            let s = match val.kind() {
                ValueKind::Str(s) => s,
                _ => {
                    if let Some(backing) = builtin_data_backing(val)
                        && let ValueKind::Str(s) = backing.kind()
                    {
                        return encode_unicode_char_str(s, out);
                    }
                    return Err(PyError::named(
                        "TypeError",
                        "array item must be unicode character".to_string(),
                    ));
                }
            };
            encode_unicode_char_str(s, out)?;
        }
        TypeCode::SignedShort => {
            let bi = value_to_array_int(interp, val)?;
            let n = check_signed(
                &bi,
                i128::from(i16::MIN),
                i128::from(i16::MAX),
                "signed short integer",
            )? as i16;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        TypeCode::UnsignedShort => {
            let bi = value_to_array_int(interp, val)?;
            let n = check_unsigned(&bi, u128::from(u16::MAX), "unsigned short integer")? as u16;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        TypeCode::SignedInt => {
            let bi = value_to_array_int(interp, val)?;
            let n = check_signed(
                &bi,
                i128::from(i32::MIN),
                i128::from(i32::MAX),
                "signed integer",
            )? as i32;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        TypeCode::UnsignedInt => {
            let bi = value_to_array_int(interp, val)?;
            let n = check_unsigned(&bi, u128::from(u32::MAX), "unsigned int")? as u32;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        TypeCode::SignedLong => {
            let bi = value_to_array_int(interp, val)?;
            if std::mem::size_of::<c_long>() == 8 {
                let n = check_signed(
                    &bi,
                    i128::from(i64::MIN),
                    i128::from(i64::MAX),
                    "signed long integer",
                )? as i64;
                out.extend_from_slice(&n.to_ne_bytes());
            } else {
                let n = check_signed(
                    &bi,
                    i128::from(i32::MIN),
                    i128::from(i32::MAX),
                    "signed integer",
                )? as i32;
                out.extend_from_slice(&n.to_ne_bytes());
            }
        }
        TypeCode::UnsignedLong => {
            let bi = value_to_array_int(interp, val)?;
            if std::mem::size_of::<c_long>() == 8 {
                let n = check_unsigned(&bi, u128::from(u64::MAX), "unsigned long integer")? as u64;
                out.extend_from_slice(&n.to_ne_bytes());
            } else {
                let n = check_unsigned(&bi, u128::from(u32::MAX), "unsigned int")? as u32;
                out.extend_from_slice(&n.to_ne_bytes());
            }
        }
        TypeCode::SignedLongLong => {
            let bi = value_to_array_int(interp, val)?;
            let n = check_signed(
                &bi,
                i128::from(i64::MIN),
                i128::from(i64::MAX),
                "signed long long integer",
            )? as i64;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        TypeCode::UnsignedLongLong => {
            let bi = value_to_array_int(interp, val)?;
            let n = check_unsigned(&bi, u128::from(u64::MAX), "unsigned long long integer")? as u64;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        TypeCode::Float32 => {
            let x = value_to_array_float(interp, val)?;
            out.extend_from_slice(&(x as f32).to_ne_bytes());
        }
        TypeCode::Float64 => {
            let x = value_to_array_float(interp, val)?;
            out.extend_from_slice(&x.to_ne_bytes());
        }
    }
    Ok(())
}

fn encode_unicode_char_str(s: &str, out: &mut Vec<u8>) -> Result<()> {
    let Some((cp, next_pos)) = pyrust_core::cesu8_next_codepoint(s, 0) else {
        return Err(PyError::named(
            "TypeError",
            "array item must be unicode character".to_string(),
        ));
    };
    if next_pos != s.len() {
        return Err(PyError::named(
            "TypeError",
            "array item must be unicode character".to_string(),
        ));
    }
    if cfg!(windows) {
        if cp > 0xFFFF {
            return Err(PyError::named(
                "TypeError",
                "array item must be unicode character".to_string(),
            ));
        }
        out.extend_from_slice(&(cp as u16).to_ne_bytes());
    } else {
        out.extend_from_slice(&cp.to_ne_bytes());
    }
    Ok(())
}

fn encode_unicode_str(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some((cp, next_pos)) = pyrust_core::cesu8_next_codepoint(s, pos) {
        pos = next_pos;
        if cfg!(windows) {
            if cp <= 0xFFFF {
                out.extend_from_slice(&(cp as u16).to_ne_bytes());
            } else {
                let adj = cp - 0x10000;
                let high = (0xD800 + (adj >> 10)) as u16;
                let low = (0xDC00 + (adj & 0x3FF)) as u16;
                out.extend_from_slice(&high.to_ne_bytes());
                out.extend_from_slice(&low.to_ne_bytes());
            }
        } else {
            out.extend_from_slice(&cp.to_ne_bytes());
        }
    }
    out
}

fn decode_unicode_bytes(bytes: &[u8]) -> String {
    let mut out = String::new();
    if cfg!(windows) {
        let mut i = 0usize;
        while i + 2 <= bytes.len() {
            let u = u16::from_ne_bytes([bytes[i], bytes[i + 1]]);
            i += 2;
            if (0xD800..=0xDBFF).contains(&u) && i + 2 <= bytes.len() {
                let next_u = u16::from_ne_bytes([bytes[i], bytes[i + 1]]);
                if (0xDC00..=0xDFFF).contains(&next_u) {
                    i += 2;
                    let cp =
                        0x10000 + (((u32::from(u) - 0xD800) << 10) | (u32::from(next_u) - 0xDC00));
                    out.push_str(&pyrust_core::cesu8_encode_codepoint(cp));
                    continue;
                }
            }
            out.push_str(&pyrust_core::cesu8_encode_codepoint(u32::from(u)));
        }
    } else {
        for chunk in bytes.as_chunks::<4>().0 {
            let cp = u32::from_ne_bytes(*chunk);
            out.push_str(&pyrust_core::cesu8_encode_codepoint(cp));
        }
    }
    out
}

fn value_from_u64(v: u64) -> Value {
    if v <= i64::MAX as u64 {
        Value::int(v as i64)
    } else {
        Value::bigint(PyBigInt::from(v))
    }
}

fn decode_single_item(tc: TypeCode, slot: &[u8]) -> Value {
    match tc {
        TypeCode::SignedChar => Value::int(i64::from(slot[0] as i8)),
        TypeCode::UnsignedChar => Value::int(i64::from(slot[0])),
        TypeCode::Unicode => {
            let cp = if cfg!(windows) {
                u32::from(u16::from_ne_bytes([slot[0], slot[1]]))
            } else {
                u32::from_ne_bytes([slot[0], slot[1], slot[2], slot[3]])
            };
            Value::string(pyrust_core::cesu8_encode_codepoint(cp))
        }
        TypeCode::SignedShort => Value::int(i64::from(i16::from_ne_bytes([slot[0], slot[1]]))),
        TypeCode::UnsignedShort => Value::int(i64::from(u16::from_ne_bytes([slot[0], slot[1]]))),
        TypeCode::SignedInt => Value::int(i64::from(i32::from_ne_bytes([
            slot[0], slot[1], slot[2], slot[3],
        ]))),
        TypeCode::UnsignedInt => Value::int(i64::from(u32::from_ne_bytes([
            slot[0], slot[1], slot[2], slot[3],
        ]))),
        TypeCode::SignedLong => {
            if std::mem::size_of::<c_long>() == 8 {
                Value::int(i64::from_ne_bytes(slot.try_into().unwrap()))
            } else {
                Value::int(i64::from(i32::from_ne_bytes(slot.try_into().unwrap())))
            }
        }
        TypeCode::UnsignedLong => {
            if std::mem::size_of::<c_long>() == 8 {
                value_from_u64(u64::from_ne_bytes(slot.try_into().unwrap()))
            } else {
                Value::int(i64::from(u32::from_ne_bytes(slot.try_into().unwrap())))
            }
        }
        TypeCode::SignedLongLong => Value::int(i64::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::UnsignedLongLong => value_from_u64(u64::from_ne_bytes(slot.try_into().unwrap())),
        TypeCode::Float32 => Value::float(f64::from(f32::from_ne_bytes(slot.try_into().unwrap()))),
        TypeCode::Float64 => Value::float(f64::from_ne_bytes(slot.try_into().unwrap())),
    }
}

fn extract_bytearray_rc(val: &Value) -> Result<Rc<RefCell<Vec<u8>>>> {
    pyrust_builtins::bytearray::as_bytearray_rc(val)
        .ok_or_else(|| PyError::Runtime("array: internal buffer must be bytearray".into()))
}

pyrust_module! {
    /// `_typecode_itemsize(typecode)` -> `itemsize` (int).
    fn _typecode_itemsize(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let tc = TypeCode::parse(&args[0].value)?;
        Ok(Value::int(tc.itemsize() as i64))
    }

    /// `_encode_item(typecode, value)` -> `bytes` (length `itemsize`).
    fn _encode_item(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let tc = TypeCode::parse(&args[0].value)?;
        let mut out = Vec::with_capacity(tc.itemsize());
        encode_single_item(_interp, tc, &args[1].value, &mut out)?;
        Ok(Value::bytes(out))
    }

    /// `_encode_items(typecode, list_val)` -> `bytes` (atomic encoding for `fromlist`).
    fn _encode_items(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let tc = TypeCode::parse(&args[0].value)?;
        let items: Vec<Value> = match args[1].value.kind() {
            ValueKind::List(slice) => slice.to_vec(),
            _ => {
                return Err(PyError::named("TypeError", "arg must be list".to_string()));
            }
        };
        let mut out = Vec::with_capacity(items.len() * tc.itemsize());
        for item in &items {
            encode_single_item(_interp, tc, item, &mut out)?;
        }
        Ok(Value::bytes(out))
    }

    /// `_decode_item(typecode, buf_bytearray, index)` -> decoded Python element.
    fn _decode_item(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let tc = TypeCode::parse(&args[0].value)?;
        let rc = extract_bytearray_rc(&args[1].value)?;
        let idx = args[2]
            .value
            .as_int()
            .ok_or_else(|| PyError::Runtime("array: internal index must be int".into()))?
            as usize;
        let itemsize = tc.itemsize();
        let start = idx * itemsize;
        let borrowed = rc.borrow();
        Ok(decode_single_item(tc, &borrowed[start..start + itemsize]))
    }

    /// `_decode_all(typecode, buf_bytearray)` -> `list` of decoded Python elements.
    fn _decode_all(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let tc = TypeCode::parse(&args[0].value)?;
        let rc = extract_bytearray_rc(&args[1].value)?;
        let itemsize = tc.itemsize();
        let borrowed = rc.borrow();
        let mut out = Vec::with_capacity(borrowed.len() / itemsize);
        for chunk in borrowed.chunks_exact(itemsize) {
            out.push(decode_single_item(tc, chunk));
        }
        Ok(Value::list(out))
    }

    /// `_encode_unicode(s)` -> `bytes` encoded in platform `Py_UNICODE`.
    fn _encode_unicode(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let val = &args[0].value;
        let bytes = match val.kind() {
            ValueKind::Str(s) => encode_unicode_str(s),
            _ => {
                if let Some(backing) = builtin_data_backing(val)
                    && let ValueKind::Str(s) = backing.kind()
                {
                    encode_unicode_str(s)
                } else {
                    return Err(PyError::named(
                        "TypeError",
                        format!(
                            "fromunicode() argument must be str, not {}",
                            value_type_name_str(val)
                        ),
                    ));
                }
            }
        };
        Ok(Value::bytes(bytes))
    }

    /// `_decode_unicode(buf_bytearray)` -> `str` decoded from platform `Py_UNICODE`.
    fn _decode_unicode(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let rc = extract_bytearray_rc(&args[0].value)?;
        let s = decode_unicode_bytes(&rc.borrow());
        Ok(Value::string(&s))
    }

    /// `_byteswap(buf_bytearray, itemsize)` -> `None` (swaps each element's bytes in-place).
    fn _byteswap(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let rc = extract_bytearray_rc(&args[0].value)?;
        let itemsize = args[1]
            .value
            .as_int()
            .ok_or_else(|| PyError::Runtime("array: internal itemsize must be int".into()))?
            as usize;
        if itemsize > 1 {
            let mut borrowed = rc.borrow_mut();
            for chunk in borrowed.chunks_exact_mut(itemsize) {
                chunk.reverse();
            }
        }
        Ok(Value::none())
    }

    /// `_as_index(val)` -> `int` via Python's `__index__` protocol.
    fn _as_index(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        _interp.call_index_protocol(&args[0].value, "array")
    }
}
