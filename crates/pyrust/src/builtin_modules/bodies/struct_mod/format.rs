//! Format string decoding, alignment rules, and layout compilation for `struct`.

use std::ffi::{c_int, c_long, c_longlong, c_short};
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::{builtin_data_backing, value_type_name_str};
use crate::value::{Value, ValueKind};

pub(super) fn raise_struct_error(error_cls: &Value, msg: impl Into<String>) -> PyError {
    let msg = msg.into();
    if let ValueKind::PyClass(cls_rc) = error_cls.kind() {
        return PyError::Raised(crate::interpreter::instantiate_exception(
            Rc::clone(cls_rc),
            vec![Value::string(&msg)],
        ));
    }
    PyError::Runtime(msg)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Endian {
    Native,
    Little,
    Big,
}

#[derive(Clone, Copy)]
pub(super) enum FormatCode {
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
pub(super) struct FormatItem {
    pub(super) code: FormatCode,
    pub(super) repeat: usize,
    pub(super) size: usize,
    pub(super) align: usize,
    pub(super) offset: usize,
}

pub(super) struct CompiledFormat {
    pub(super) format_str: String,
    pub(super) endian: Endian,
    pub(super) native: bool,
    pub(super) size: usize,
    pub(super) num_values: usize,
    pub(super) items: Vec<FormatItem>,
}

fn encode_ascii_str(s: &str) -> Result<(String, Vec<u8>)> {
    if let Some((pos, ch)) = s.char_indices().find(|(_, ch)| !ch.is_ascii()) {
        return Err(PyError::named(
            "UnicodeEncodeError",
            format!(
                "'ascii' codec can't encode character '\\u{:04x}' in position {pos}: ordinal not in range(128)",
                ch as u32
            ),
        ));
    }
    Ok((s.to_string(), s.as_bytes().to_vec()))
}

fn decode_format_input(fmt_val: &Value, error_cls: &Value) -> Result<(String, Vec<u8>)> {
    let type_err = || {
        PyError::named(
            "TypeError",
            format!(
                "Struct() argument 1 must be a str or bytes object, not {}",
                value_type_name_str(fmt_val)
            ),
        )
    };
    let raw = match fmt_val.kind() {
        ValueKind::Str(s) => return encode_ascii_str(s),
        ValueKind::Bytes(rc) => rc.as_ref().clone(),
        _ => match builtin_data_backing(fmt_val) {
            Some(backing) => match backing.kind() {
                ValueKind::Str(s) => return encode_ascii_str(s),
                ValueKind::Bytes(rc) => rc.as_ref().clone(),
                _ => return Err(type_err()),
            },
            None => return Err(type_err()),
        },
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

pub(super) fn compile_format(fmt_val: &Value, error_cls: &Value) -> Result<CompiledFormat> {
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
