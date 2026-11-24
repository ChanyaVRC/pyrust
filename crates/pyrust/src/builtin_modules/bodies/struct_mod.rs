// `struct` module — body for the `"struct" as struct_mod @inject` entry in
// `pyrust_builtin_modules!` (issue #2807).
//
// Binary format parsing, endianness conversion, alignment, IEEE-754 binary16
// (`e`), and buffer packing/unpacking live in Rust here so performance and
// native C type sizing (`@` mode via `std::ffi::c_long`, etc.) match CPython
// 3.12 on Windows (LLP64), Linux (LP64), and macOS (LP64).
//
// The Python-level `error` exception class, `Struct` class, `iter_unpack`
// iterator wrapper, `__all__`, and public callables live in `struct_mod_py.py`
// and are injected onto the module by `inject_python_members`.  Each wrapper
// closes over its module generation's `error` class and passes it as the first
// argument to the private native primitives so `except struct.error:` matches
// even across `del sys.modules["struct"]` reloads.
//
// Reference: <https://docs.python.org/3/library/struct.html>

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

/// Python-source definitions for the public `struct` surface (`error`,
/// `Struct`, top-level wrappers, `_clearcache`, `__all__`).
const STRUCT_PY_SOURCE: &str = include_str!("struct_mod_py.py");

/// Private native helpers seeded into the injected exec namespace and removed
/// from `struct.__dict__` after injection so `dir(struct)` matches CPython.
const STRUCT_NATIVE_HELPERS: [&str; 6] = [
    "_parse_format",
    "_pack",
    "_unpack",
    "_pack_into",
    "_unpack_from",
    "_check_iter_unpack",
];

/// Public names exported from `STRUCT_PY_SOURCE` onto the `struct` module.
const STRUCT_PY_EXPORTS: [&str; 9] = [
    "error",
    "Struct",
    "calcsize",
    "pack",
    "unpack",
    "pack_into",
    "unpack_from",
    "iter_unpack",
    "_clearcache",
];

/// Execute `STRUCT_PY_SOURCE` once, wire its closures to the native primitives,
/// hide the internal helper names from `dir(struct)`, and export the public
/// CPython 3.12 `struct` surface onto `module`.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for helper in STRUCT_NATIVE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source(STRUCT_PY_SOURCE, Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::Runtime("struct: exec namespace not a dict".into()))?;
    for name in STRUCT_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                let mut class = cls_rc.borrow_mut();
                class
                    .attrs
                    .insert("__module__".to_string(), Value::string("struct"));
                if name == "Struct" {
                    class.error_name = Some("struct.Struct");
                }
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}

// ── Error helper ─────────────────────────────────────────────────────────────

fn raise_struct_error(error_cls: &Value, msg: impl Into<String>) -> PyError {
    let msg = msg.into();
    if let ValueKind::PyClass(cls_rc) = error_cls.kind() {
        return PyError::Raised(crate::interpreter::instantiate_exception(
            Rc::clone(cls_rc),
            vec![Value::string(&msg)],
        ));
    }
    PyError::Runtime(msg)
}

// ── Format representation and parser ─────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Endian {
    Native,
    Little,
    Big,
}

#[derive(Clone, Copy)]
enum FormatCode {
    Pad,
    Char,
    Int8,
    UInt8,
    Bool,
    Int16,
    UInt16,
    Int32,
    UInt32,
    Long,
    ULong,
    Int64,
    UInt64,
    SSize,
    USize,
    Float16,
    Float32,
    Float64,
    Bytes,
    PascalBytes,
    Pointer,
}

#[derive(Clone, Copy)]
struct FormatItem {
    code: FormatCode,
    repeat: usize,
    size: usize,
    align: usize,
    offset: usize,
}

struct CompiledFormat {
    format_str: String,
    endian: Endian,
    native: bool,
    size: usize,
    num_values: usize,
    items: Vec<FormatItem>,
}

fn decode_format_input(fmt_val: &Value, error_cls: &Value) -> Result<(String, Vec<u8>)> {
    let raw = match fmt_val.kind() {
        ValueKind::Str(s) => {
            if let Some((pos, ch)) = s.char_indices().find(|(_, ch)| !ch.is_ascii()) {
                return Err(PyError::named(
                    "UnicodeEncodeError",
                    format!(
                        "'ascii' codec can't encode character '\\u{:04x}' in position {pos}: ordinal not in range(128)",
                        ch as u32
                    ),
                ));
            }
            return Ok((s.to_string(), s.as_bytes().to_vec()));
        }
        ValueKind::Bytes(rc) => rc.as_ref().clone(),
        _ => {
            if let Some(backing) = builtin_data_backing(fmt_val) {
                match backing.kind() {
                    ValueKind::Str(s) => {
                        if let Some((pos, ch)) = s.char_indices().find(|(_, ch)| !ch.is_ascii()) {
                            return Err(PyError::named(
                                "UnicodeEncodeError",
                                format!(
                                    "'ascii' codec can't encode character '\\u{:04x}' in position {pos}: ordinal not in range(128)",
                                    ch as u32
                                ),
                            ));
                        }
                        return Ok((s.to_string(), s.as_bytes().to_vec()));
                    }
                    ValueKind::Bytes(rc) => rc.as_ref().clone(),
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            format!(
                                "Struct() argument 1 must be a str or bytes object, not {}",
                                value_type_name_str(fmt_val)
                            ),
                        ));
                    }
                }
            } else {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "Struct() argument 1 must be a str or bytes object, not {}",
                        value_type_name_str(fmt_val)
                    ),
                ));
            }
        }
    };
    // Bytes format: non-ASCII bytes fail during format parsing as `struct.error`
    // (matching CPython: `struct.calcsize(b'\xff')` -> `struct.error: bad char`).
    if !raw.is_ascii() {
        return Err(raise_struct_error(error_cls, "bad char in struct format"));
    }
    let decoded = String::from_utf8(raw.clone())
        .map_err(|_| raise_struct_error(error_cls, "bad char in struct format"))?;
    Ok((decoded, raw))
}

fn item_size_and_align(code: FormatCode, native: bool) -> (usize, usize) {
    if native {
        match code {
            FormatCode::Pad
            | FormatCode::Char
            | FormatCode::Int8
            | FormatCode::UInt8
            | FormatCode::Bool
            | FormatCode::Bytes
            | FormatCode::PascalBytes => (1, 1),
            FormatCode::Int16 | FormatCode::UInt16 => (
                std::mem::size_of::<c_short>(),
                std::mem::align_of::<c_short>(),
            ),
            FormatCode::Int32 | FormatCode::UInt32 => {
                (std::mem::size_of::<c_int>(), std::mem::align_of::<c_int>())
            }
            FormatCode::Long | FormatCode::ULong => (
                std::mem::size_of::<c_long>(),
                std::mem::align_of::<c_long>(),
            ),
            FormatCode::Int64 | FormatCode::UInt64 => (
                std::mem::size_of::<c_longlong>(),
                std::mem::align_of::<c_longlong>(),
            ),
            FormatCode::SSize | FormatCode::USize => {
                (std::mem::size_of::<isize>(), std::mem::align_of::<isize>())
            }
            FormatCode::Pointer => (
                std::mem::size_of::<*const ()>(),
                std::mem::align_of::<*const ()>(),
            ),
            FormatCode::Float16 => (2, 2),
            FormatCode::Float32 => (std::mem::size_of::<f32>(), std::mem::align_of::<f32>()),
            FormatCode::Float64 => (std::mem::size_of::<f64>(), std::mem::align_of::<f64>()),
        }
    } else {
        let size = match code {
            FormatCode::Pad
            | FormatCode::Char
            | FormatCode::Int8
            | FormatCode::UInt8
            | FormatCode::Bool
            | FormatCode::Bytes
            | FormatCode::PascalBytes => 1,
            FormatCode::Int16 | FormatCode::UInt16 | FormatCode::Float16 => 2,
            FormatCode::Int32
            | FormatCode::UInt32
            | FormatCode::Long
            | FormatCode::ULong
            | FormatCode::Float32 => 4,
            FormatCode::Int64 | FormatCode::UInt64 | FormatCode::Float64 => 8,
            FormatCode::SSize | FormatCode::USize | FormatCode::Pointer => {
                unreachable!("native-only format codes rejected before sizing")
            }
        };
        (size, 1)
    }
}

fn compile_format(fmt_val: &Value, error_cls: &Value) -> Result<CompiledFormat> {
    let (format_str, bytes) = decode_format_input(fmt_val, error_cls)?;
    let mut pos = 0usize;
    let (endian, native) = match bytes.first().copied() {
        Some(b'@') => {
            pos = 1;
            (Endian::Native, true)
        }
        Some(b'=') => {
            pos = 1;
            (Endian::Native, false)
        }
        Some(b'<') => {
            pos = 1;
            (Endian::Little, false)
        }
        Some(b'>' | b'!') => {
            pos = 1;
            (Endian::Big, false)
        }
        _ => (Endian::Native, true),
    };

    let mut items = Vec::new();
    let mut offset = 0usize;
    let mut num_values = 0usize;

    while pos < bytes.len() {
        let b = bytes[pos];
        if matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
            pos += 1;
            continue;
        }
        let mut has_repeat = false;
        let mut repeat = 0usize;
        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
            has_repeat = true;
            let digit = (bytes[pos] - b'0') as usize;
            repeat = repeat
                .checked_mul(10)
                .and_then(|v| v.checked_add(digit))
                .ok_or_else(|| raise_struct_error(error_cls, "total struct size too long"))?;
            pos += 1;
        }
        if pos >= bytes.len() {
            if has_repeat {
                return Err(raise_struct_error(
                    error_cls,
                    "repeat count given without format specifier",
                ));
            }
            break;
        }
        if !has_repeat {
            repeat = 1;
        }
        let ch = bytes[pos];
        pos += 1;

        let code = match ch {
            b'x' => FormatCode::Pad,
            b'c' => FormatCode::Char,
            b'b' => FormatCode::Int8,
            b'B' => FormatCode::UInt8,
            b'?' => FormatCode::Bool,
            b'h' => FormatCode::Int16,
            b'H' => FormatCode::UInt16,
            b'i' => FormatCode::Int32,
            b'I' => FormatCode::UInt32,
            b'l' => FormatCode::Long,
            b'L' => FormatCode::ULong,
            b'q' => FormatCode::Int64,
            b'Q' => FormatCode::UInt64,
            b'n' if native => FormatCode::SSize,
            b'N' if native => FormatCode::USize,
            b'P' if native => FormatCode::Pointer,
            b'e' => FormatCode::Float16,
            b'f' => FormatCode::Float32,
            b'd' => FormatCode::Float64,
            b's' => FormatCode::Bytes,
            b'p' => FormatCode::PascalBytes,
            _ => return Err(raise_struct_error(error_cls, "bad char in struct format")),
        };

        let (item_size, align) = item_size_and_align(code, native);
        if native && align > 1 {
            let rem = offset % align;
            if rem != 0 {
                offset = offset
                    .checked_add(align - rem)
                    .ok_or_else(|| raise_struct_error(error_cls, "total struct size too long"))?;
            }
        }
        let item_offset = offset;
        let span = repeat
            .checked_mul(item_size)
            .ok_or_else(|| raise_struct_error(error_cls, "total struct size too long"))?;
        offset = offset
            .checked_add(span)
            .ok_or_else(|| raise_struct_error(error_cls, "total struct size too long"))?;

        let value_count = match code {
            FormatCode::Pad => 0,
            FormatCode::Bytes | FormatCode::PascalBytes => 1,
            _ => repeat,
        };
        num_values = num_values
            .checked_add(value_count)
            .ok_or_else(|| raise_struct_error(error_cls, "total struct size too long"))?;

        items.push(FormatItem {
            code,
            repeat,
            size: item_size,
            align,
            offset: item_offset,
        });
    }

    Ok(CompiledFormat {
        format_str,
        endian,
        native,
        size: offset,
        num_values,
        items,
    })
}

// ── Numeric & buffer coercion helpers ────────────────────────────────────────

fn value_to_struct_int(
    interp: &mut Interpreter,
    val: &Value,
    error_cls: &Value,
) -> Result<PyBigInt> {
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
                    _ => Err(raise_struct_error(
                        error_cls,
                        "required argument is not an integer",
                    )),
                }
            } else {
                Err(raise_struct_error(
                    error_cls,
                    "required argument is not an integer",
                ))
            }
        }
        _ => Err(raise_struct_error(
            error_cls,
            "required argument is not an integer",
        )),
    }
}

fn value_to_struct_float(interp: &mut Interpreter, val: &Value, error_cls: &Value) -> Result<f64> {
    if matches!(val.kind(), ValueKind::PyInstance(_) | ValueKind::PyClass(_)) {
        let backing = builtin_data_backing(val);
        if let Some(ref b) = backing
            && matches!(b.kind(), ValueKind::Float(_))
        {
            return value_to_float(b, "struct");
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
            return value_to_float(b, "struct");
        }
        if let Some(indexed) = interp.try_value_to_index(val)? {
            return value_to_float(&indexed, "struct");
        }
        return Err(raise_struct_error(
            error_cls,
            "required argument is not a float",
        ));
    }
    match val.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => {
            value_to_float(val, "struct")
        }
        _ => Err(raise_struct_error(
            error_cls,
            "required argument is not a float",
        )),
    }
}

fn extract_readonly_bytes(val: &Value) -> Result<Vec<u8>> {
    match val.kind() {
        ValueKind::Bytes(rc) => Ok(rc.as_ref().clone()),
        _ => {
            if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
                return Ok(snap);
            }
            if let Some(backing) = builtin_data_backing(val) {
                if let ValueKind::Bytes(rc) = backing.kind() {
                    return Ok(rc.as_ref().clone());
                }
                if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
                    return Ok(snap);
                }
            }
            Err(PyError::named(
                "TypeError",
                format!(
                    "a bytes-like object is required, not '{}'",
                    value_type_name_str(val)
                ),
            ))
        }
    }
}

fn extract_writable_bytearray(val: &Value) -> Result<Rc<RefCell<Vec<u8>>>> {
    if let Some(rc) = pyrust_builtins::bytearray::as_bytearray_rc(val) {
        return Ok(rc);
    }
    if let Some(backing) = builtin_data_backing(val)
        && let Some(rc) = pyrust_builtins::bytearray::as_bytearray_rc(&backing)
    {
        return Ok(rc);
    }
    Err(PyError::named(
        "TypeError",
        format!(
            "argument must be read-write bytes-like object, not {}",
            value_type_name_str(val)
        ),
    ))
}

fn extract_bytes_for_s_or_p(val: &Value, fmt_char: char, error_cls: &Value) -> Result<Vec<u8>> {
    match val.kind() {
        ValueKind::Bytes(rc) => Ok(rc.as_ref().clone()),
        _ => {
            if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(val) {
                return Ok(snap);
            }
            if let Some(backing) = builtin_data_backing(val) {
                if let ValueKind::Bytes(rc) = backing.kind() {
                    return Ok(rc.as_ref().clone());
                }
                if let Some(snap) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
                    return Ok(snap);
                }
            }
            Err(raise_struct_error(
                error_cls,
                format!("argument for '{fmt_char}' must be a bytes object"),
            ))
        }
    }
}

fn check_signed_range(
    bi: &PyBigInt,
    min: i128,
    max: i128,
    fmt_char: char,
    error_cls: &Value,
) -> Result<i128> {
    if let Some(v) = bi.to_i128()
        && v >= min
        && v <= max
    {
        return Ok(v);
    }
    Err(raise_struct_error(
        error_cls,
        format!("'{fmt_char}' format requires {min} <= number <= {max}"),
    ))
}

fn check_unsigned_range(
    bi: &PyBigInt,
    max: u128,
    fmt_char: char,
    error_cls: &Value,
) -> Result<u128> {
    if let Some(v) = bi.to_u128()
        && v <= max
    {
        return Ok(v);
    }
    Err(raise_struct_error(
        error_cls,
        format!("'{fmt_char}' format requires 0 <= number <= {max}"),
    ))
}

// ── IEEE-754 binary16 (`e`) conversion ───────────────────────────────────────

fn f64_to_f16_bits(x: f64) -> Result<u16> {
    let bits = x.to_bits();
    let sign = ((bits >> 48) & 0x8000) as u16;
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & 0x000f_ffff_ffff_ffff;

    if exp == 0x7ff {
        if frac == 0 {
            return Ok(sign | 0x7c00);
        }
        let payload = ((frac >> 42) as u16) & 0x03ff;
        return Ok(sign
            | 0x7c00
            | if payload == 0 {
                0x0200
            } else {
                payload | 0x0200
            });
    }
    if exp == 0 && frac == 0 {
        return Ok(sign);
    }

    let e = exp - 1023;
    if e >= -14 {
        let mut h_exp = e + 15;
        let mut mant = (frac >> 42) as u32;
        let rem = frac & ((1u64 << 42) - 1);
        let half = 1u64 << 41;
        if rem > half || (rem == half && (mant & 1) != 0) {
            mant += 1;
            if mant == 0x400 {
                mant = 0;
                h_exp += 1;
            }
        }
        if h_exp >= 31 {
            return Err(PyError::named(
                "OverflowError",
                "float too large to pack with e format".to_string(),
            ));
        }
        return Ok(sign | ((h_exp as u16) << 10) | (mant as u16));
    }

    if e < -25 {
        return Ok(sign);
    }

    // Subnormal binary16 (with possible round-up to smallest normal 0x0400).
    let sig = (1u64 << 52) | frac;
    let shift = ((-14 - e) as u32) + 42;
    let mut mant = (sig >> shift) as u32;
    let rem = sig & ((1u64 << shift) - 1);
    let half = 1u64 << (shift - 1);
    if rem > half || (rem == half && (mant & 1) != 0) {
        mant += 1;
    }
    Ok(sign | (mant as u16))
}

fn f16_bits_to_f64(bits: u16) -> f64 {
    let sign = ((bits & 0x8000) as u64) << 48;
    let exp = ((bits >> 10) & 0x1f) as i32;
    let mant = (bits & 0x03ff) as u64;

    if exp == 0 {
        if mant == 0 {
            return f64::from_bits(sign);
        }
        // Normalize subnormal 10-bit mantissa into binary64.
        let lead = 63 - mant.leading_zeros() as i32; // 0..=9
        let shift = (10 - lead) as u32;
        let norm_mant = (mant << shift) & 0x03ff;
        let d_exp = ((1023 - 14 - (10 - lead)) as u64) << 52;
        return f64::from_bits(sign | d_exp | (norm_mant << 42));
    }
    if exp == 31 {
        if mant == 0 {
            return f64::from_bits(sign | (0x7ffu64 << 52));
        }
        return f64::from_bits(sign | (0x7ffu64 << 52) | (mant << 42));
    }
    let d_exp = ((exp - 15 + 1023) as u64) << 52;
    f64::from_bits(sign | d_exp | (mant << 42))
}

// ── Endianness write/read helpers ────────────────────────────────────────────

macro_rules! write_bytes {
    ($endian:expr, $dst:expr, $val:expr) => {
        match $endian {
            Endian::Native => $dst.copy_from_slice(&$val.to_ne_bytes()),
            Endian::Little => $dst.copy_from_slice(&$val.to_le_bytes()),
            Endian::Big => $dst.copy_from_slice(&$val.to_be_bytes()),
        }
    };
}

macro_rules! read_bytes {
    ($endian:expr, $ty:ty, $src:expr) => {{
        let arr: [u8; std::mem::size_of::<$ty>()] = $src
            .try_into()
            .expect("slice length matches format item size");
        match $endian {
            Endian::Native => <$ty>::from_ne_bytes(arr),
            Endian::Little => <$ty>::from_le_bytes(arr),
            Endian::Big => <$ty>::from_be_bytes(arr),
        }
    }};
}

fn value_from_u64(v: u64) -> Value {
    if v <= i64::MAX as u64 {
        Value::int(v as i64)
    } else {
        Value::bigint(PyBigInt::from(v))
    }
}

fn value_from_usize(v: usize) -> Value {
    if v <= i64::MAX as usize {
        Value::int(v as i64)
    } else {
        Value::bigint(PyBigInt::from(v))
    }
}

// ── Packing and unpacking engines ────────────────────────────────────────────

fn pack_into_vec(
    interp: &mut Interpreter,
    compiled: &CompiledFormat,
    values: &[Value],
    error_cls: &Value,
) -> Result<Vec<u8>> {
    if values.len() != compiled.num_values {
        return Err(raise_struct_error(
            error_cls,
            format!(
                "pack expected {} items for packing (got {})",
                compiled.num_values,
                values.len()
            ),
        ));
    }
    let mut out = vec![0u8; compiled.size];
    let mut arg_idx = 0usize;

    for item in &compiled.items {
        let _ = item.align;
        let mut cursor = item.offset;
        match item.code {
            FormatCode::Pad => {}
            FormatCode::Bytes => {
                let raw = extract_bytes_for_s_or_p(&values[arg_idx], 's', error_cls)?;
                arg_idx += 1;
                let copy_len = raw.len().min(item.repeat);
                out[cursor..cursor + copy_len].copy_from_slice(&raw[..copy_len]);
            }
            FormatCode::PascalBytes => {
                let raw = extract_bytes_for_s_or_p(&values[arg_idx], 'p', error_cls)?;
                arg_idx += 1;
                if item.repeat > 0 {
                    let max_data = item.repeat - 1;
                    let copy_len = raw.len().min(max_data);
                    let stored_len = copy_len.min(255) as u8;
                    out[cursor] = stored_len;
                    out[cursor + 1..cursor + 1 + copy_len].copy_from_slice(&raw[..copy_len]);
                }
            }
            _ => {
                for _ in 0..item.repeat {
                    let val = &values[arg_idx];
                    arg_idx += 1;
                    let slot = &mut out[cursor..cursor + item.size];
                    match item.code {
                        FormatCode::Char => {
                            let byte = match val.kind() {
                                ValueKind::Bytes(rc) if rc.len() == 1 => rc[0],
                                _ => {
                                    if let Some(backing) = builtin_data_backing(val)
                                        && let ValueKind::Bytes(rc) = backing.kind()
                                        && rc.len() == 1
                                    {
                                        rc[0]
                                    } else {
                                        return Err(raise_struct_error(
                                            error_cls,
                                            "char format requires a bytes object of length 1",
                                        ));
                                    }
                                }
                            };
                            slot[0] = byte;
                        }
                        FormatCode::Bool => {
                            slot[0] = u8::from(interp.truthy_value(val)?);
                        }
                        FormatCode::Int8 => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_signed_range(
                                &bi,
                                i128::from(i8::MIN),
                                i128::from(i8::MAX),
                                'b',
                                error_cls,
                            )? as i8;
                            slot[0] = n as u8;
                        }
                        FormatCode::UInt8 => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_unsigned_range(&bi, u128::from(u8::MAX), 'B', error_cls)?
                                as u8;
                            slot[0] = n;
                        }
                        FormatCode::Int16 => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_signed_range(
                                &bi,
                                i128::from(i16::MIN),
                                i128::from(i16::MAX),
                                'h',
                                error_cls,
                            )? as i16;
                            write_bytes!(compiled.endian, slot, n);
                        }
                        FormatCode::UInt16 => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_unsigned_range(&bi, u128::from(u16::MAX), 'H', error_cls)?
                                as u16;
                            write_bytes!(compiled.endian, slot, n);
                        }
                        FormatCode::Int32 => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_signed_range(
                                &bi,
                                i128::from(i32::MIN),
                                i128::from(i32::MAX),
                                'i',
                                error_cls,
                            )? as i32;
                            write_bytes!(compiled.endian, slot, n);
                        }
                        FormatCode::UInt32 => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_unsigned_range(&bi, u128::from(u32::MAX), 'I', error_cls)?
                                as u32;
                            write_bytes!(compiled.endian, slot, n);
                        }
                        FormatCode::Long => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            if compiled.native && std::mem::size_of::<c_long>() == 8 {
                                let n = check_signed_range(
                                    &bi,
                                    i128::from(i64::MIN),
                                    i128::from(i64::MAX),
                                    'l',
                                    error_cls,
                                )? as i64;
                                write_bytes!(compiled.endian, slot, n);
                            } else {
                                let n = check_signed_range(
                                    &bi,
                                    i128::from(i32::MIN),
                                    i128::from(i32::MAX),
                                    'l',
                                    error_cls,
                                )? as i32;
                                write_bytes!(compiled.endian, slot, n);
                            }
                        }
                        FormatCode::ULong => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            if compiled.native && std::mem::size_of::<c_long>() == 8 {
                                let n = check_unsigned_range(
                                    &bi,
                                    u128::from(u64::MAX),
                                    'L',
                                    error_cls,
                                )? as u64;
                                write_bytes!(compiled.endian, slot, n);
                            } else {
                                let n = check_unsigned_range(
                                    &bi,
                                    u128::from(u32::MAX),
                                    'L',
                                    error_cls,
                                )? as u32;
                                write_bytes!(compiled.endian, slot, n);
                            }
                        }
                        FormatCode::Int64 => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_signed_range(
                                &bi,
                                i128::from(i64::MIN),
                                i128::from(i64::MAX),
                                'q',
                                error_cls,
                            )? as i64;
                            write_bytes!(compiled.endian, slot, n);
                        }
                        FormatCode::UInt64 => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_unsigned_range(&bi, u128::from(u64::MAX), 'Q', error_cls)?
                                as u64;
                            write_bytes!(compiled.endian, slot, n);
                        }
                        FormatCode::SSize => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_signed_range(
                                &bi,
                                isize::MIN as i128,
                                isize::MAX as i128,
                                'n',
                                error_cls,
                            )? as isize;
                            write_bytes!(compiled.endian, slot, n);
                        }
                        FormatCode::USize => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_unsigned_range(&bi, usize::MAX as u128, 'N', error_cls)?
                                as usize;
                            write_bytes!(compiled.endian, slot, n);
                        }
                        FormatCode::Pointer => {
                            let bi = value_to_struct_int(interp, val, error_cls)?;
                            let n = check_unsigned_range(&bi, usize::MAX as u128, 'P', error_cls)?
                                as usize;
                            write_bytes!(compiled.endian, slot, n);
                        }
                        FormatCode::Float16 => {
                            let x = value_to_struct_float(interp, val, error_cls)?;
                            let bits = f64_to_f16_bits(x)?;
                            write_bytes!(compiled.endian, slot, bits);
                        }
                        FormatCode::Float32 => {
                            let x = value_to_struct_float(interp, val, error_cls)?;
                            let f = x as f32;
                            if x.is_finite() && f.is_infinite() {
                                return Err(PyError::named(
                                    "OverflowError",
                                    "float too large to pack with f format".to_string(),
                                ));
                            }
                            write_bytes!(compiled.endian, slot, f);
                        }
                        FormatCode::Float64 => {
                            let x = value_to_struct_float(interp, val, error_cls)?;
                            write_bytes!(compiled.endian, slot, x);
                        }
                        FormatCode::Pad | FormatCode::Bytes | FormatCode::PascalBytes => {
                            unreachable!()
                        }
                    }
                    cursor += item.size;
                }
            }
        }
    }

    Ok(out)
}

fn unpack_from_slice(compiled: &CompiledFormat, buf: &[u8]) -> Value {
    let mut results = Vec::with_capacity(compiled.num_values);

    for item in &compiled.items {
        let mut cursor = item.offset;
        match item.code {
            FormatCode::Pad => {}
            FormatCode::Bytes => {
                results.push(Value::bytes(buf[cursor..cursor + item.repeat].to_vec()));
            }
            FormatCode::PascalBytes => {
                if item.repeat == 0 {
                    results.push(Value::bytes(Vec::new()));
                } else {
                    let stored_len = (buf[cursor] as usize).min(item.repeat - 1);
                    results.push(Value::bytes(
                        buf[cursor + 1..cursor + 1 + stored_len].to_vec(),
                    ));
                }
            }
            _ => {
                for _ in 0..item.repeat {
                    let slot = &buf[cursor..cursor + item.size];
                    let val = match item.code {
                        FormatCode::Char => Value::bytes(vec![slot[0]]),
                        FormatCode::Bool => Value::bool_(slot[0] != 0),
                        FormatCode::Int8 => Value::int(i64::from(slot[0] as i8)),
                        FormatCode::UInt8 => Value::int(i64::from(slot[0])),
                        FormatCode::Int16 => {
                            Value::int(i64::from(read_bytes!(compiled.endian, i16, slot)))
                        }
                        FormatCode::UInt16 => {
                            Value::int(i64::from(read_bytes!(compiled.endian, u16, slot)))
                        }
                        FormatCode::Int32 => {
                            Value::int(i64::from(read_bytes!(compiled.endian, i32, slot)))
                        }
                        FormatCode::UInt32 => {
                            Value::int(i64::from(read_bytes!(compiled.endian, u32, slot)))
                        }
                        FormatCode::Long => {
                            if compiled.native && std::mem::size_of::<c_long>() == 8 {
                                Value::int(read_bytes!(compiled.endian, i64, slot))
                            } else {
                                Value::int(i64::from(read_bytes!(compiled.endian, i32, slot)))
                            }
                        }
                        FormatCode::ULong => {
                            if compiled.native && std::mem::size_of::<c_long>() == 8 {
                                value_from_u64(read_bytes!(compiled.endian, u64, slot))
                            } else {
                                Value::int(i64::from(read_bytes!(compiled.endian, u32, slot)))
                            }
                        }
                        FormatCode::Int64 => Value::int(read_bytes!(compiled.endian, i64, slot)),
                        FormatCode::UInt64 => {
                            value_from_u64(read_bytes!(compiled.endian, u64, slot))
                        }
                        FormatCode::SSize => {
                            Value::int(read_bytes!(compiled.endian, isize, slot) as i64)
                        }
                        FormatCode::USize | FormatCode::Pointer => {
                            value_from_usize(read_bytes!(compiled.endian, usize, slot))
                        }
                        FormatCode::Float16 => {
                            let bits = read_bytes!(compiled.endian, u16, slot);
                            Value::float(f16_bits_to_f64(bits))
                        }
                        FormatCode::Float32 => {
                            Value::float(f64::from(read_bytes!(compiled.endian, f32, slot)))
                        }
                        FormatCode::Float64 => {
                            Value::float(read_bytes!(compiled.endian, f64, slot))
                        }
                        FormatCode::Pad | FormatCode::Bytes | FormatCode::PascalBytes => {
                            unreachable!()
                        }
                    };
                    results.push(val);
                    cursor += item.size;
                }
            }
        }
    }

    Value::tuple(results)
}

fn parse_offset_i128(interp: &mut Interpreter, offset_val: &Value, packing: bool) -> Result<i128> {
    let idx_val = interp.call_index_protocol(
        offset_val,
        if packing { "pack_into" } else { "unpack_from" },
    )?;
    match idx_val.kind() {
        ValueKind::Int(n) => Ok(i128::from(n)),
        ValueKind::Bool(b) => Ok(i128::from(i64::from(b))),
        ValueKind::BigInt(bi) => bi.to_i128().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "cannot fit 'int' into an index-sized integer".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                value_type_name_str(offset_val)
            ),
        )),
    }
}

fn validate_offset_i128(
    offset_i128: i128,
    buf_len: usize,
    struct_size: usize,
    packing: bool,
    error_cls: &Value,
) -> Result<usize> {
    let buf_len_i = buf_len as i128;
    let size_i = struct_size as i128;
    if offset_i128 < 0 {
        let adj = buf_len_i + offset_i128;
        if adj < 0 {
            return Err(raise_struct_error(
                error_cls,
                format!("offset {offset_i128} out of range for {buf_len}-byte buffer"),
            ));
        }
        if adj + size_i > buf_len_i {
            let verb = if packing {
                "no space to pack"
            } else {
                "not enough data to unpack"
            };
            return Err(raise_struct_error(
                error_cls,
                format!("{verb} {struct_size} bytes at offset {offset_i128}"),
            ));
        }
        Ok(adj as usize)
    } else {
        if offset_i128 + size_i > buf_len_i {
            let (fn_name, gerund) = if packing {
                ("pack_into", "packing")
            } else {
                ("unpack_from", "unpacking")
            };
            return Err(raise_struct_error(
                error_cls,
                format!(
                    "{fn_name} requires a buffer of at least {} bytes for {gerund} {struct_size} bytes at offset {offset_i128} (actual buffer size is {buf_len})",
                    offset_i128 + size_i
                ),
            ));
        }
        Ok(offset_i128 as usize)
    }
}

fn resolve_offset(
    interp: &mut Interpreter,
    offset_val: &Value,
    buf_len: usize,
    struct_size: usize,
    packing: bool,
    error_cls: &Value,
) -> Result<usize> {
    let offset_i128 = parse_offset_i128(interp, offset_val, packing)?;
    validate_offset_i128(offset_i128, buf_len, struct_size, packing, error_cls)
}

pyrust_module! {
    /// `_parse_format(error_cls, format)` -> `(normalized_str, size)`.
    fn _parse_format(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let compiled = compile_format(&args[1].value, error_cls)?;
        Ok(Value::tuple(vec![
            Value::string(&compiled.format_str),
            Value::int(compiled.size as i64),
        ]))
    }

    /// `_pack(error_cls, format, values_tuple)` -> `bytes`.
    fn _pack(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let compiled = compile_format(&args[1].value, error_cls)?;
        let values: Vec<Value> = match args[2].value.kind() {
            ValueKind::Tuple(items) => items.to_vec(),
            _ => Vec::new(),
        };
        let bytes = pack_into_vec(_interp, &compiled, &values, error_cls)?;
        Ok(Value::bytes(bytes))
    }

    /// `_unpack(error_cls, format, buffer)` -> `tuple`.
    fn _unpack(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let compiled = compile_format(&args[1].value, error_cls)?;
        let buf = extract_readonly_bytes(&args[2].value)?;
        if buf.len() != compiled.size {
            return Err(raise_struct_error(
                error_cls,
                format!("unpack requires a buffer of {} bytes", compiled.size),
            ));
        }
        Ok(unpack_from_slice(&compiled, &buf))
    }

    /// `_pack_into(error_cls, format, buffer, offset, values_tuple)` -> `None`.
    fn _pack_into(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let compiled = compile_format(&args[1].value, error_cls)?;
        let target = extract_writable_bytearray(&args[2].value)?;
        let values: Vec<Value> = match args[4].value.kind() {
            ValueKind::Tuple(items) => items.to_vec(),
            _ => Vec::new(),
        };
        if values.len() != compiled.num_values {
            return Err(raise_struct_error(
                error_cls,
                format!(
                    "pack_into expected {} items for packing (got {})",
                    compiled.num_values,
                    values.len()
                ),
            ));
        }
        let offset_i128 = parse_offset_i128(_interp, &args[3].value, true)?;
        let initial_len = target.borrow().len();
        let _ = validate_offset_i128(offset_i128, initial_len, compiled.size, true, error_cls)?;
        // Pack into a scratch Vec before borrowing the bytearray mutably so
        // user `__index__`/`__bool__`/`__float__` cannot alias the borrow or
        // leave a partially written buffer on error.
        let packed = pack_into_vec(_interp, &compiled, &values, error_cls)?;
        let final_len = target.borrow().len();
        let start = validate_offset_i128(offset_i128, final_len, compiled.size, true, error_cls)?;
        let mut dst = target.borrow_mut();
        dst[start..start + compiled.size].copy_from_slice(&packed);
        Ok(Value::none())
    }

    /// `_unpack_from(error_cls, format, buffer, offset)` -> `tuple`.
    fn _unpack_from(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let compiled = compile_format(&args[1].value, error_cls)?;
        let buf = extract_readonly_bytes(&args[2].value)?;
        let start = resolve_offset(
            _interp,
            &args[3].value,
            buf.len(),
            compiled.size,
            false,
            error_cls,
        )?;
        Ok(unpack_from_slice(&compiled, &buf[start..start + compiled.size]))
    }

    /// `_check_iter_unpack(error_cls, format, buffer)` -> `(size, buf_len)`.
    fn _check_iter_unpack(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        let error_cls = &args[0].value;
        let compiled = compile_format(&args[1].value, error_cls)?;
        let buf = extract_readonly_bytes(&args[2].value)?;
        if compiled.size == 0 {
            return Err(raise_struct_error(
                error_cls,
                "cannot iteratively unpack with a struct of length 0",
            ));
        }
        if buf.len() % compiled.size != 0 {
            return Err(raise_struct_error(
                error_cls,
                format!(
                    "iterative unpacking requires a buffer of a multiple of {} bytes",
                    compiled.size
                ),
            ));
        }
        Ok(Value::tuple(vec![
            Value::int(compiled.size as i64),
            Value::int(buf.len() as i64),
        ]))
    }
}
