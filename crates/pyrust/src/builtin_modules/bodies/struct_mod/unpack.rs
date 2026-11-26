//! Binary unpacking engine and `unpack` / `unpack_from` / `iter_unpack`
//! operations for `struct`.

use std::ffi::c_long;

use crate::error::Result;
use crate::interpreter::Interpreter;
use crate::value::Value;

use super::coercion::{
    extract_readonly_bytes, f16_bits_to_f64, resolve_offset, value_from_u64, value_from_usize,
};
use super::format::{CompiledFormat, Endian, FormatCode, raise_struct_error};

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

pub(super) fn unpack_from_slice(compiled: &CompiledFormat, buf: &[u8]) -> Value {
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

/// `struct.unpack` / `Struct.unpack`: the buffer must match the struct size.
pub(super) fn unpack_buffer(
    interp: &mut Interpreter,
    compiled: &CompiledFormat,
    buffer: &Value,
    error_cls: &Value,
) -> Result<Value> {
    let buf = extract_readonly_bytes(interp, buffer)?;
    if buf.len() != compiled.size {
        return Err(raise_struct_error(
            error_cls,
            format!("unpack requires a buffer of {} bytes", compiled.size),
        ));
    }
    Ok(unpack_from_slice(compiled, &buf))
}

/// `struct.unpack_from` / `Struct.unpack_from`.
pub(super) fn unpack_buffer_from(
    interp: &mut Interpreter,
    compiled: &CompiledFormat,
    buffer: &Value,
    offset: &Value,
    error_cls: &Value,
) -> Result<Value> {
    let buf = extract_readonly_bytes(interp, buffer)?;
    let start = resolve_offset(interp, offset, buf.len(), compiled.size, false, error_cls)?;
    Ok(unpack_from_slice(
        compiled,
        &buf[start..start + compiled.size],
    ))
}

/// `struct.iter_unpack` / `Struct.iter_unpack`: validate the buffer and return
/// a snapshot of its bytes for the iterator to walk.
pub(super) fn snapshot_for_iter_unpack(
    interp: &mut Interpreter,
    compiled: &CompiledFormat,
    buffer: &Value,
    error_cls: &Value,
) -> Result<Vec<u8>> {
    let buf = extract_readonly_bytes(interp, buffer)?;
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
    Ok(buf)
}
