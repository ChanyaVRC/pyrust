//! Native struct format parsing, element packing, and element unpacking for
//! `memoryview`.
//!
//! Matches CPython 3.12's `Objects/memoryobject.c` native format table:
//! single-character native formats (`b`, `B`, `c`, `h`, `H`, `i`, `I`, `l`,
//! `L`, `q`, `Q`, `n`, `N`, `e`, `f`, `d`, `?`, `P`, plus `u` / `w` for
//! `array.array('u')`), optionally prefixed with `'@'`.

use std::ffi::{c_int, c_long, c_longlong, c_short};

use pyrust_core::{PyBigInt, PyBigIntSign, PyError, PyToPrimitive, Result, Value, ValueKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NativeFormat {
    SignedChar,       // 'b'
    UnsignedChar,     // 'B'
    Char,             // 'c'
    Bool,             // '?'
    SignedShort,      // 'h'
    UnsignedShort,    // 'H'
    SignedInt,        // 'i'
    UnsignedInt,      // 'I'
    SignedLong,       // 'l'
    UnsignedLong,     // 'L'
    SignedLongLong,   // 'q'
    UnsignedLongLong, // 'Q'
    SSize,            // 'n'
    USize,            // 'N'
    Pointer,          // 'P'
    Float16,          // 'e'
    Float32,          // 'f'
    Float64,          // 'd'
    Unicode,          // 'u' (from array.array('u'))
}

impl NativeFormat {
    /// Parse any supported memoryview format string (including `array.array('u')`).
    pub(crate) fn parse(fmt: &str) -> Result<Self> {
        let stripped = fmt.strip_prefix('@').unwrap_or(fmt);
        let mut chars = stripped.chars();
        let Some(ch) = chars.next() else {
            return Err(invalid_cast_format_error());
        };
        if chars.next().is_some() {
            return Err(invalid_cast_format_error());
        }
        match ch {
            'b' => Ok(Self::SignedChar),
            'B' => Ok(Self::UnsignedChar),
            'c' => Ok(Self::Char),
            '?' => Ok(Self::Bool),
            'h' => Ok(Self::SignedShort),
            'H' => Ok(Self::UnsignedShort),
            'i' => Ok(Self::SignedInt),
            'I' => Ok(Self::UnsignedInt),
            'l' => Ok(Self::SignedLong),
            'L' => Ok(Self::UnsignedLong),
            'q' => Ok(Self::SignedLongLong),
            'Q' => Ok(Self::UnsignedLongLong),
            'n' => Ok(Self::SSize),
            'N' => Ok(Self::USize),
            'P' => Ok(Self::Pointer),
            'e' => Ok(Self::Float16),
            'f' => Ok(Self::Float32),
            'd' => Ok(Self::Float64),
            // `array.array('u')` exports `'w'` where `wchar_t` is 4 bytes
            // (CPython's `Py_UNICODE_WIDE` builds).
            'u' | 'w' => Ok(Self::Unicode),
            _ => Err(invalid_cast_format_error()),
        }
    }

    /// Parse a destination format for `memoryview.cast(format[, shape])`.
    /// CPython 3.12 restricts `cast()` to native struct formats (no `'u'`).
    pub(crate) fn parse_for_cast(fmt: &str) -> Result<Self> {
        let parsed = Self::parse(fmt)?;
        if parsed == Self::Unicode {
            return Err(invalid_cast_format_error());
        }
        Ok(parsed)
    }

    pub(crate) fn itemsize(self) -> usize {
        match self {
            Self::SignedChar | Self::UnsignedChar | Self::Char | Self::Bool => 1,
            Self::Float16 => 2,
            Self::SignedShort | Self::UnsignedShort => std::mem::size_of::<c_short>(),
            Self::SignedInt | Self::UnsignedInt => std::mem::size_of::<c_int>(),
            Self::SignedLong | Self::UnsignedLong => std::mem::size_of::<c_long>(),
            Self::SignedLongLong | Self::UnsignedLongLong => std::mem::size_of::<c_longlong>(),
            Self::SSize | Self::USize | Self::Pointer => std::mem::size_of::<usize>(),
            Self::Float32 => std::mem::size_of::<f32>(),
            Self::Float64 => std::mem::size_of::<f64>(),
            Self::Unicode => {
                if cfg!(windows) {
                    2
                } else {
                    4
                }
            }
        }
    }

    pub(crate) fn is_byte_format(self) -> bool {
        matches!(self, Self::SignedChar | Self::UnsignedChar | Self::Char)
    }

    pub(crate) fn is_unsigned_byte(self) -> bool {
        self == Self::UnsignedChar
    }
}

pub(crate) fn invalid_cast_format_error() -> PyError {
    PyError::named(
        "ValueError",
        "memoryview: destination format must be a native single character format prefixed with an optional '@'".to_string(),
    )
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

/// Unpack a single element of `fmt` from a byte slice of length `fmt.itemsize()`.
pub(crate) fn unpack_element(fmt: NativeFormat, slot: &[u8]) -> Value {
    match fmt {
        NativeFormat::SignedChar => Value::int(i64::from(slot[0] as i8)),
        NativeFormat::UnsignedChar => Value::int(i64::from(slot[0])),
        NativeFormat::Char => Value::bytes(vec![slot[0]]),
        NativeFormat::Bool => Value::bool_(slot[0] != 0),
        NativeFormat::SignedShort => {
            Value::int(i64::from(i16::from_ne_bytes(slot.try_into().unwrap())))
        }
        NativeFormat::UnsignedShort => {
            Value::int(i64::from(u16::from_ne_bytes(slot.try_into().unwrap())))
        }
        NativeFormat::SignedInt => {
            Value::int(i64::from(i32::from_ne_bytes(slot.try_into().unwrap())))
        }
        NativeFormat::UnsignedInt => {
            Value::int(i64::from(u32::from_ne_bytes(slot.try_into().unwrap())))
        }
        NativeFormat::SignedLong => {
            if std::mem::size_of::<c_long>() == 8 {
                Value::int(i64::from_ne_bytes(slot.try_into().unwrap()))
            } else {
                Value::int(i64::from(i32::from_ne_bytes(slot.try_into().unwrap())))
            }
        }
        NativeFormat::UnsignedLong => {
            if std::mem::size_of::<c_long>() == 8 {
                value_from_u64(u64::from_ne_bytes(slot.try_into().unwrap()))
            } else {
                Value::int(i64::from(u32::from_ne_bytes(slot.try_into().unwrap())))
            }
        }
        NativeFormat::SignedLongLong => Value::int(i64::from_ne_bytes(slot.try_into().unwrap())),
        NativeFormat::UnsignedLongLong => {
            value_from_u64(u64::from_ne_bytes(slot.try_into().unwrap()))
        }
        NativeFormat::SSize => {
            let v = isize::from_ne_bytes(slot.try_into().unwrap());
            Value::int(v as i64)
        }
        NativeFormat::USize | NativeFormat::Pointer => {
            let v = usize::from_ne_bytes(slot.try_into().unwrap());
            value_from_usize(v)
        }
        NativeFormat::Float16 => {
            let bits = u16::from_ne_bytes(slot.try_into().unwrap());
            Value::float(f16_bits_to_f64(bits))
        }
        NativeFormat::Float32 => {
            Value::float(f64::from(f32::from_ne_bytes(slot.try_into().unwrap())))
        }
        NativeFormat::Float64 => Value::float(f64::from_ne_bytes(slot.try_into().unwrap())),
        NativeFormat::Unicode => {
            let cp = if cfg!(windows) {
                u32::from(u16::from_ne_bytes([slot[0], slot[1]]))
            } else {
                u32::from_ne_bytes([slot[0], slot[1], slot[2], slot[3]])
            };
            Value::string(pyrust_core::cesu8_encode_codepoint(cp))
        }
    }
}

fn value_to_bigint_for_pack(val: &Value, fmt_str: &str) -> Result<PyBigInt> {
    match val.kind() {
        ValueKind::Int(n) => Ok(PyBigInt::from(n)),
        ValueKind::Bool(b) => Ok(PyBigInt::from(i64::from(b))),
        ValueKind::BigInt(bi) => Ok(bi.clone()),
        _ => Err(PyError::named(
            "TypeError",
            format!("memoryview: invalid type for format '{fmt_str}'"),
        )),
    }
}

fn value_to_float_for_pack(val: &Value, fmt_str: &str) -> Result<f64> {
    match val.kind() {
        ValueKind::Float(f) => Ok(f),
        ValueKind::Int(n) => Ok(n as f64),
        ValueKind::Bool(b) => Ok(if b { 1.0 } else { 0.0 }),
        ValueKind::BigInt(bi) => bi.to_f64().ok_or_else(|| {
            PyError::named(
                "OverflowError",
                "int too large to convert to float".to_string(),
            )
        }),
        _ => Err(PyError::named(
            "TypeError",
            format!("memoryview: invalid type for format '{fmt_str}'"),
        )),
    }
}

fn check_signed(bi: &PyBigInt, min: i128, max: i128, fmt_str: &str) -> Result<i128> {
    if let Some(v) = bi.to_i128()
        && v >= min
        && v <= max
    {
        return Ok(v);
    }
    Err(PyError::named(
        "ValueError",
        format!("memoryview: invalid value for format '{fmt_str}'"),
    ))
}

fn check_unsigned(bi: &PyBigInt, max: u128, fmt_str: &str) -> Result<u128> {
    if bi.sign() != PyBigIntSign::Minus
        && let Some(v) = bi.to_u128()
        && v <= max
    {
        return Ok(v);
    }
    Err(PyError::named(
        "ValueError",
        format!("memoryview: invalid value for format '{fmt_str}'"),
    ))
}

/// Pack a single Python value into `fmt.itemsize()` bytes.
pub(crate) fn pack_element(fmt: NativeFormat, fmt_str: &str, val: &Value) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(fmt.itemsize());
    match fmt {
        NativeFormat::SignedChar => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_signed(&bi, i128::from(i8::MIN), i128::from(i8::MAX), fmt_str)? as i8;
            out.push(n as u8);
        }
        NativeFormat::UnsignedChar => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_unsigned(&bi, u128::from(u8::MAX), fmt_str)? as u8;
            out.push(n);
        }
        NativeFormat::Char => match val.kind() {
            ValueKind::Bytes(rc) if rc.len() == 1 => out.push(rc[0]),
            ValueKind::Bytes(_) => {
                return Err(PyError::named(
                    "ValueError",
                    format!("memoryview: invalid value for format '{fmt_str}'"),
                ));
            }
            _ => {
                return Err(PyError::named(
                    "TypeError",
                    format!("memoryview: invalid type for format '{fmt_str}'"),
                ));
            }
        },
        NativeFormat::Bool => {
            out.push(u8::from(val.truthy_raw()));
        }
        NativeFormat::SignedShort => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_signed(&bi, i128::from(i16::MIN), i128::from(i16::MAX), fmt_str)? as i16;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        NativeFormat::UnsignedShort => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_unsigned(&bi, u128::from(u16::MAX), fmt_str)? as u16;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        NativeFormat::SignedInt => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_signed(&bi, i128::from(i32::MIN), i128::from(i32::MAX), fmt_str)? as i32;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        NativeFormat::UnsignedInt => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_unsigned(&bi, u128::from(u32::MAX), fmt_str)? as u32;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        NativeFormat::SignedLong => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            if std::mem::size_of::<c_long>() == 8 {
                let n =
                    check_signed(&bi, i128::from(i64::MIN), i128::from(i64::MAX), fmt_str)? as i64;
                out.extend_from_slice(&n.to_ne_bytes());
            } else {
                let n =
                    check_signed(&bi, i128::from(i32::MIN), i128::from(i32::MAX), fmt_str)? as i32;
                out.extend_from_slice(&n.to_ne_bytes());
            }
        }
        NativeFormat::UnsignedLong => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            if std::mem::size_of::<c_long>() == 8 {
                let n = check_unsigned(&bi, u128::from(u64::MAX), fmt_str)? as u64;
                out.extend_from_slice(&n.to_ne_bytes());
            } else {
                let n = check_unsigned(&bi, u128::from(u32::MAX), fmt_str)? as u32;
                out.extend_from_slice(&n.to_ne_bytes());
            }
        }
        NativeFormat::SignedLongLong => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_signed(&bi, i128::from(i64::MIN), i128::from(i64::MAX), fmt_str)? as i64;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        NativeFormat::UnsignedLongLong => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_unsigned(&bi, u128::from(u64::MAX), fmt_str)? as u64;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        NativeFormat::SSize => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_signed(&bi, isize::MIN as i128, isize::MAX as i128, fmt_str)? as isize;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        NativeFormat::USize | NativeFormat::Pointer => {
            let bi = value_to_bigint_for_pack(val, fmt_str)?;
            let n = check_unsigned(&bi, usize::MAX as u128, fmt_str)? as usize;
            out.extend_from_slice(&n.to_ne_bytes());
        }
        NativeFormat::Float16 => {
            let x = value_to_float_for_pack(val, fmt_str)?;
            let bits = f64_to_f16_bits(x)?;
            out.extend_from_slice(&bits.to_ne_bytes());
        }
        NativeFormat::Float32 => {
            let x = value_to_float_for_pack(val, fmt_str)?;
            let f = x as f32;
            if x.is_finite() && f.is_infinite() {
                return Err(PyError::named(
                    "OverflowError",
                    "float too large to pack with f format".to_string(),
                ));
            }
            out.extend_from_slice(&f.to_ne_bytes());
        }
        NativeFormat::Float64 => {
            let x = value_to_float_for_pack(val, fmt_str)?;
            out.extend_from_slice(&x.to_ne_bytes());
        }
        NativeFormat::Unicode => {
            let ValueKind::Str(s) = val.kind() else {
                return Err(PyError::named(
                    "TypeError",
                    format!("memoryview: invalid type for format '{fmt_str}'"),
                ));
            };
            let Some((cp, next_pos)) = pyrust_core::cesu8_next_codepoint(s, 0) else {
                return Err(PyError::named(
                    "ValueError",
                    format!("memoryview: invalid value for format '{fmt_str}'"),
                ));
            };
            if next_pos != s.len() {
                return Err(PyError::named(
                    "ValueError",
                    format!("memoryview: invalid value for format '{fmt_str}'"),
                ));
            }
            if cfg!(windows) {
                if cp > 0xFFFF {
                    return Err(PyError::named(
                        "ValueError",
                        format!("memoryview: invalid value for format '{fmt_str}'"),
                    ));
                }
                out.extend_from_slice(&(cp as u16).to_ne_bytes());
            } else {
                out.extend_from_slice(&cp.to_ne_bytes());
            }
        }
    }
    Ok(out)
}

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
        let lead = 63 - mant.leading_zeros() as i32;
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
