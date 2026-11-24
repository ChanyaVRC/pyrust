//! Numeric coercion, buffer extraction, IEEE-754 binary16 conversion, and
//! buffer offset validation for `struct`.

use crate::error::{PyError, Result};
use crate::interpreter::{
    Interpreter, builtin_data_backing, invoke_class_method, lookup_value_special_method,
    normalize_float_slot_result, value_to_float, value_type_name_str,
};
use crate::value::{PyBigInt, PyToPrimitive, Value, ValueKind};

use super::format::raise_struct_error;

pub(super) fn value_to_struct_int(
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

pub(super) fn value_to_struct_float(
    interp: &mut Interpreter,
    val: &Value,
    error_cls: &Value,
) -> Result<f64> {
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

pub(super) fn extract_readonly_bytes(interp: &mut Interpreter, val: &Value) -> Result<Vec<u8>> {
    crate::interpreter::extract_buffer_bytes(interp, val)
}

pub(super) fn extract_bytes_for_s_or_p(
    interp: &mut Interpreter,
    val: &Value,
    fmt_char: char,
    error_cls: &Value,
) -> Result<Vec<u8>> {
    crate::interpreter::try_extract_buffer_bytes(interp, val)?.ok_or_else(|| {
        raise_struct_error(
            error_cls,
            format!("argument for '{fmt_char}' must be a bytes object"),
        )
    })
}

pub(super) fn check_signed_range(
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

pub(super) fn check_unsigned_range(
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

pub(super) fn f64_to_f16_bits(x: f64) -> Result<u16> {
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

pub(super) fn f16_bits_to_f64(bits: u16) -> f64 {
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

pub(super) fn value_from_u64(v: u64) -> Value {
    if v <= i64::MAX as u64 {
        Value::int(v as i64)
    } else {
        Value::bigint(PyBigInt::from(v))
    }
}

pub(super) fn value_from_usize(v: usize) -> Value {
    if v <= i64::MAX as usize {
        Value::int(v as i64)
    } else {
        Value::bigint(PyBigInt::from(v))
    }
}

// ── Offset parsing & bounds validation ───────────────────────────────────────

pub(super) fn parse_offset_i128(
    interp: &mut Interpreter,
    offset_val: &Value,
    packing: bool,
) -> Result<i128> {
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

pub(super) fn validate_offset_i128(
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

pub(super) fn resolve_offset(
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
