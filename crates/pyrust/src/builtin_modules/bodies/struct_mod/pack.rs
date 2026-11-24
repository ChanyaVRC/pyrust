//! Binary packing engine and `pack` / `pack_into` entry points for `struct`.

use std::ffi::c_long;

use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, reject_keyword_args_expanded,
};
use crate::value::{Value, ValueKind};

use super::coercion::{
    check_signed_range, check_unsigned_range, extract_bytes_for_s_or_p, f64_to_f16_bits,
    parse_offset_i128, validate_offset_i128, value_to_struct_float, value_to_struct_int,
};
use super::format::{CompiledFormat, Endian, FormatCode, compile_format, raise_struct_error};

macro_rules! write_bytes {
    ($endian:expr, $dst:expr, $val:expr) => {
        match $endian {
            Endian::Native => $dst.copy_from_slice(&$val.to_ne_bytes()),
            Endian::Little => $dst.copy_from_slice(&$val.to_le_bytes()),
            Endian::Big => $dst.copy_from_slice(&$val.to_be_bytes()),
        }
    };
}

macro_rules! pack_signed {
    ($interp:expr, $val:expr, $ty:ty, $fmt:literal, $endian:expr, $slot:expr, $err:expr) => {{
        let bi = value_to_struct_int($interp, $val, $err)?;
        let n = check_signed_range(&bi, <$ty>::MIN as i128, <$ty>::MAX as i128, $fmt, $err)? as $ty;
        write_bytes!($endian, $slot, n);
    }};
}

macro_rules! pack_unsigned {
    ($interp:expr, $val:expr, $ty:ty, $fmt:literal, $endian:expr, $slot:expr, $err:expr) => {{
        let bi = value_to_struct_int($interp, $val, $err)?;
        let n = check_unsigned_range(&bi, <$ty>::MAX as u128, $fmt, $err)? as $ty;
        write_bytes!($endian, $slot, n);
    }};
}

pub(super) fn pack_into_vec(
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
                let raw = extract_bytes_for_s_or_p(interp, &values[arg_idx], 's', error_cls)?;
                arg_idx += 1;
                let copy_len = raw.len().min(item.repeat);
                out[cursor..cursor + copy_len].copy_from_slice(&raw[..copy_len]);
            }
            FormatCode::PascalBytes => {
                let raw = extract_bytes_for_s_or_p(interp, &values[arg_idx], 'p', error_cls)?;
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
                            slot[0] =
                                check_unsigned_range(&bi, u128::from(u8::MAX), 'B', error_cls)?
                                    as u8;
                        }
                        FormatCode::Int16 => {
                            pack_signed!(interp, val, i16, 'h', compiled.endian, slot, error_cls);
                        }
                        FormatCode::UInt16 => {
                            pack_unsigned!(interp, val, u16, 'H', compiled.endian, slot, error_cls);
                        }
                        FormatCode::Int32 => {
                            pack_signed!(interp, val, i32, 'i', compiled.endian, slot, error_cls);
                        }
                        FormatCode::UInt32 => {
                            pack_unsigned!(interp, val, u32, 'I', compiled.endian, slot, error_cls);
                        }
                        FormatCode::Long => {
                            if compiled.native && std::mem::size_of::<c_long>() == 8 {
                                pack_signed!(
                                    interp,
                                    val,
                                    i64,
                                    'l',
                                    compiled.endian,
                                    slot,
                                    error_cls
                                );
                            } else {
                                pack_signed!(
                                    interp,
                                    val,
                                    i32,
                                    'l',
                                    compiled.endian,
                                    slot,
                                    error_cls
                                );
                            }
                        }
                        FormatCode::ULong => {
                            if compiled.native && std::mem::size_of::<c_long>() == 8 {
                                pack_unsigned!(
                                    interp,
                                    val,
                                    u64,
                                    'L',
                                    compiled.endian,
                                    slot,
                                    error_cls
                                );
                            } else {
                                pack_unsigned!(
                                    interp,
                                    val,
                                    u32,
                                    'L',
                                    compiled.endian,
                                    slot,
                                    error_cls
                                );
                            }
                        }
                        FormatCode::Int64 => {
                            pack_signed!(interp, val, i64, 'q', compiled.endian, slot, error_cls);
                        }
                        FormatCode::UInt64 => {
                            pack_unsigned!(interp, val, u64, 'Q', compiled.endian, slot, error_cls);
                        }
                        FormatCode::SSize => {
                            pack_signed!(interp, val, isize, 'n', compiled.endian, slot, error_cls);
                        }
                        FormatCode::USize => {
                            pack_unsigned!(
                                interp,
                                val,
                                usize,
                                'N',
                                compiled.endian,
                                slot,
                                error_cls
                            );
                        }
                        FormatCode::Pointer => {
                            pack_unsigned!(
                                interp,
                                val,
                                usize,
                                'P',
                                compiled.endian,
                                slot,
                                error_cls
                            );
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

pub(super) fn pack_entry(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    let error_cls = &args[0].value;
    let compiled = compile_format(&args[1].value, error_cls)?;
    let values: Vec<Value> = match args[2].value.kind() {
        ValueKind::Tuple(items) => items.to_vec(),
        _ => Vec::new(),
    };
    let bytes = pack_into_vec(interp, &compiled, &values, error_cls)?;
    Ok(Value::bytes(bytes))
}

pub(super) fn pack_into_entry(
    interp: &mut Interpreter,
    fn_name: &str,
    args: &[ExpandedCallArg],
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    let error_cls = &args[0].value;
    let compiled = compile_format(&args[1].value, error_cls)?;
    let target = crate::interpreter::acquire_writable_buffer(interp, &args[2].value)?;
    let result = (|| -> Result<Value> {
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
        let offset_i128 = parse_offset_i128(interp, &args[3].value, true)?;
        let initial_len = target.len;
        let _ = validate_offset_i128(offset_i128, initial_len, compiled.size, true, error_cls)?;
        // Pack into a scratch Vec before borrowing the buffer mutably so
        // user `__index__`/`__bool__`/`__float__` cannot alias the borrow or
        // leave a partially written buffer on error.
        let packed = pack_into_vec(interp, &compiled, &values, error_cls)?;
        let final_len = target
            .data
            .borrow()
            .len()
            .saturating_sub(target.offset)
            .min(target.len);
        let start = validate_offset_i128(offset_i128, final_len, compiled.size, true, error_cls)?;
        let mut dst = target.data.borrow_mut();
        let abs_start = target.offset + start;
        dst[abs_start..abs_start + compiled.size].copy_from_slice(&packed);
        Ok(Value::none())
    })();
    let rel_res = target.release(interp);
    let out = result?;
    rel_res?;
    Ok(out)
}
