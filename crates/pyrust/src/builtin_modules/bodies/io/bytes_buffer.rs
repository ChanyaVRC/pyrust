//! Byte-oriented buffer policy for `io.BytesIO`.
//!
//! The `io` module facade owns Python argument validation and closed-stream
//! errors. This module owns byte cursor semantics and mutations of the `_buf` /
//! `_pos` instance state.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::value::{PyInstance, Value, ValueKind};

use super::{get_pos, set_pos};

pub(super) fn initialize(inst: &Rc<RefCell<PyInstance>>, initial: Vec<u8>) {
    let mut attrs = inst.borrow_mut();
    attrs.attrs.insert("_buf", Value::bytes(initial));
    attrs.attrs.insert("_pos", Value::int(0));
    attrs.attrs.insert("_closed", Value::bool_(false));
}

pub(super) fn contents(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<Vec<u8>> {
    match inst.borrow().attrs.get("_buf").map(|value| value.kind()) {
        Some(ValueKind::Bytes(buffer)) => Ok(buffer.to_vec()),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() BytesIO._buf corrupted",
        ))),
    }
}

fn buffer_len(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<usize> {
    match inst.borrow().attrs.get("_buf").map(|value| value.kind()) {
        Some(ValueKind::Bytes(buffer)) => Ok(buffer.len()),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() BytesIO._buf corrupted",
        ))),
    }
}

pub(super) fn read(
    inst: &Rc<RefCell<PyInstance>>,
    size: Option<usize>,
    fn_name: &str,
) -> Result<Vec<u8>> {
    let buffer = contents(inst, fn_name)?;
    let total = buffer.len();
    let position = get_pos(inst);
    let start = usize::try_from(position).unwrap_or(usize::MAX).min(total);
    let end = match size {
        None => total,
        Some(size) => start.saturating_add(size).min(total),
    };
    let result = buffer[start..end].to_vec();
    if position <= total as i64 {
        set_pos(inst, end as i64);
    }
    Ok(result)
}

pub(super) fn read_line(
    inst: &Rc<RefCell<PyInstance>>,
    size_limit: Option<usize>,
    fn_name: &str,
) -> Result<Vec<u8>> {
    let buffer = contents(inst, fn_name)?;
    let total = buffer.len();
    let position = get_pos(inst);
    let start = usize::try_from(position).unwrap_or(usize::MAX).min(total);
    let max_read = size_limit.unwrap_or(total - start);
    let mut count = 0;
    for byte in &buffer[start..] {
        if count >= max_read {
            break;
        }
        count += 1;
        if *byte == b'\n' {
            break;
        }
    }
    let result = buffer[start..start + count].to_vec();
    if position <= total as i64 {
        set_pos(inst, (start + count) as i64);
    }
    Ok(result)
}

pub(super) fn read_lines(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<Vec<Vec<u8>>> {
    let buffer = contents(inst, fn_name)?;
    let total = buffer.len();
    let position = get_pos(inst);
    let start = usize::try_from(position).unwrap_or(usize::MAX).min(total);
    let remaining = &buffer[start..];
    if position <= total as i64 {
        set_pos(inst, total as i64);
    }

    let mut lines = Vec::new();
    let mut current_start = 0;
    for (index, byte) in remaining.iter().enumerate() {
        if *byte == b'\n' {
            lines.push(remaining[current_start..=index].to_vec());
            current_start = index + 1;
        }
    }
    if current_start < remaining.len() {
        lines.push(remaining[current_start..].to_vec());
    }
    Ok(lines)
}

/// Splice bytes at the current cursor, overwriting existing data, NUL-padding
/// a gap past EOF, and advancing the cursor.
pub(super) fn write(inst: &Rc<RefCell<PyInstance>>, data: &[u8]) -> Result<usize> {
    // CPython leaves both the cursor and backing buffer untouched for an empty
    // write, including when the cursor is positioned far beyond EOF.
    if data.is_empty() {
        return Ok(0);
    }
    let written_len = data.len();
    let written_len_i64 = i64::try_from(written_len)
        .map_err(|_| PyError::named("OverflowError", "new buffer size too large".to_string()))?;
    let pos_i64 = get_pos(inst);
    let new_pos_i64 = pos_i64
        .checked_add(written_len_i64)
        .ok_or_else(|| PyError::named("OverflowError", "new buffer size too large".to_string()))?;
    let pos = usize::try_from(pos_i64)
        .map_err(|_| PyError::named("OverflowError", "new buffer size too large".to_string()))?;
    let new_pos = usize::try_from(new_pos_i64)
        .map_err(|_| PyError::named("OverflowError", "new buffer size too large".to_string()))?;
    let buffer = match inst.borrow().attrs.get("_buf").map(|value| value.kind()) {
        Some(ValueKind::Bytes(buffer)) => buffer.to_vec(),
        _ => Vec::new(),
    };
    let total = buffer.len();
    let mut new_buffer = Vec::new();
    new_buffer
        .try_reserve_exact(total.max(new_pos))
        .map_err(|_| PyError::named("MemoryError", String::new()))?;
    if pos > total {
        new_buffer.extend_from_slice(&buffer);
        new_buffer.extend(std::iter::repeat_n(0, pos - total));
    } else {
        new_buffer.extend_from_slice(&buffer[..pos]);
    }
    new_buffer.extend_from_slice(data);
    let end = new_pos.min(total);
    if end < total {
        new_buffer.extend_from_slice(&buffer[end..]);
    }
    inst.borrow_mut()
        .attrs
        .insert("_buf", Value::bytes(new_buffer));
    set_pos(inst, new_pos_i64);
    Ok(written_len)
}

pub(super) fn seek(
    inst: &Rc<RefCell<PyInstance>>,
    offset: i64,
    whence: i64,
    fn_name: &str,
) -> Result<i64> {
    let buffer_len = i64::try_from(buffer_len(inst, fn_name)?)
        .map_err(|_| PyError::named("OverflowError", "new position too large".to_string()))?;
    let new_pos = match whence {
        0 => offset,
        1 => get_pos(inst)
            .checked_add(offset)
            .ok_or_else(|| PyError::named("OverflowError", "new position too large".to_string()))?,
        2 => buffer_len
            .checked_add(offset)
            .ok_or_else(|| PyError::named("OverflowError", "new position too large".to_string()))?,
        _ => {
            return Err(PyError::named(
                "ValueError",
                format!("{fn_name}(): invalid whence value {whence}"),
            ));
        }
    };
    if new_pos < 0 {
        return Err(PyError::named(
            "ValueError",
            format!("{fn_name}(): negative seek position {new_pos}"),
        ));
    }
    set_pos(inst, new_pos);
    Ok(new_pos)
}

pub(super) fn truncate(inst: &Rc<RefCell<PyInstance>>, size: i64, fn_name: &str) -> Result<i64> {
    let current_len = buffer_len(inst, fn_name)?;
    if size < current_len as i64 {
        let buffer = contents(inst, fn_name)?;
        let new_len = usize::try_from(size).expect("truncate size is validated as non-negative");
        inst.borrow_mut()
            .attrs
            .insert("_buf", Value::bytes(buffer[..new_len].to_vec()));
    }
    // CPython reports the requested size even though truncating past EOF does
    // not extend an in-memory stream.
    Ok(size)
}

pub(super) fn read_into(
    inst: &Rc<RefCell<PyInstance>>,
    destination: &mut [u8],
    fn_name: &str,
) -> Result<usize> {
    let buffer = contents(inst, fn_name)?;
    let total = buffer.len();
    let position = get_pos(inst);
    let start = usize::try_from(position).unwrap_or(usize::MAX).min(total);
    let read_len = (total - start).min(destination.len());
    destination[..read_len].copy_from_slice(&buffer[start..start + read_len]);
    if position <= total as i64 {
        set_pos(inst, (start + read_len) as i64);
    }
    Ok(read_len)
}

pub(super) fn next_line(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<Option<Vec<u8>>> {
    let buffer = contents(inst, fn_name)?;
    let total = buffer.len();
    let start = usize::try_from(get_pos(inst))
        .unwrap_or(usize::MAX)
        .min(total);
    if start >= total {
        return Ok(None);
    }

    let mut count = 0;
    for byte in &buffer[start..] {
        count += 1;
        if *byte == b'\n' {
            break;
        }
    }
    let line = buffer[start..start + count].to_vec();
    set_pos(inst, (start + count) as i64);
    Ok(Some(line))
}
