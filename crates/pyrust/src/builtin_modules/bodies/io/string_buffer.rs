//! Character-oriented buffer policy for `io.StringIO`.
//!
//! The `io` module facade owns Python argument validation and closed-stream
//! errors. This module owns Unicode-character cursor semantics and mutations of
//! the `_buf` / `_pos` instance state.

use std::cell::RefCell;
use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::value::{PyInstance, Value, ValueKind};

use super::{get_pos, set_pos};

pub(super) fn initialize(inst: &Rc<RefCell<PyInstance>>, initial: &str) {
    let mut attrs = inst.borrow_mut();
    attrs.attrs.insert("_buf", Value::string(initial));
    attrs.attrs.insert("_pos", Value::int(0));
    attrs.attrs.insert("_closed", Value::bool_(false));
}

pub(super) fn contents(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<String> {
    match inst.borrow().attrs.get("_buf").map(|value| value.kind()) {
        Some(ValueKind::Str(buffer)) => Ok(buffer.to_string()),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() StringIO._buf corrupted",
        ))),
    }
}

fn character_len(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<usize> {
    match inst.borrow().attrs.get("_buf").map(|value| value.kind()) {
        Some(ValueKind::Str(buffer)) => Ok(buffer.chars().count()),
        _ => Err(PyError::Runtime(format!(
            "internal: {fn_name}() StringIO._buf corrupted",
        ))),
    }
}

pub(super) fn read(
    inst: &Rc<RefCell<PyInstance>>,
    size: Option<usize>,
    fn_name: &str,
) -> Result<String> {
    let buffer = contents(inst, fn_name)?;
    let chars: Vec<char> = buffer.chars().collect();
    let total = chars.len();
    let position = get_pos(inst);
    let start = usize::try_from(position).unwrap_or(usize::MAX).min(total);
    let end = match size {
        None => total,
        Some(size) => start.saturating_add(size).min(total),
    };
    let result = chars[start..end].iter().collect();
    if position <= total as i64 {
        set_pos(inst, end as i64);
    }
    Ok(result)
}

pub(super) fn read_line(
    inst: &Rc<RefCell<PyInstance>>,
    size_limit: Option<usize>,
    fn_name: &str,
) -> Result<String> {
    let buffer = contents(inst, fn_name)?;
    let chars: Vec<char> = buffer.chars().collect();
    let total = chars.len();
    let position = get_pos(inst);
    let start = usize::try_from(position).unwrap_or(usize::MAX).min(total);
    let max_read = size_limit.unwrap_or(total - start);
    let mut count = 0;
    for character in &chars[start..] {
        count += 1;
        if count > max_read {
            count -= 1;
            break;
        }
        if *character == '\n' {
            break;
        }
    }
    let result = chars[start..start + count].iter().collect();
    if position <= total as i64 {
        set_pos(inst, (start + count) as i64);
    }
    Ok(result)
}

pub(super) fn read_lines(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<Vec<String>> {
    let buffer = contents(inst, fn_name)?;
    let chars: Vec<char> = buffer.chars().collect();
    let total = chars.len();
    let position = get_pos(inst);
    let start = usize::try_from(position).unwrap_or(usize::MAX).min(total);
    if position <= total as i64 {
        set_pos(inst, total as i64);
    }

    let remaining: String = chars[start..].iter().collect();
    if remaining.is_empty() {
        return Ok(Vec::new());
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    for character in remaining.chars() {
        current.push(character);
        if character == '\n' {
            lines.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    Ok(lines)
}

/// Splice text at the current character cursor, overwriting existing
/// characters, NUL-padding a gap past EOF, and advancing the cursor.
pub(super) fn write(inst: &Rc<RefCell<PyInstance>>, text: &str) -> Result<usize> {
    // CPython leaves both the cursor and backing buffer untouched for an empty
    // write, including when the cursor is positioned far beyond EOF.
    if text.is_empty() {
        return Ok(0);
    }
    let written_len = text.chars().count();
    let written_len_i64 = i64::try_from(written_len)
        .map_err(|_| PyError::named("OverflowError", "new position too large".to_string()))?;
    let pos_i64 = get_pos(inst);
    let new_pos_i64 = pos_i64
        .checked_add(written_len_i64)
        .ok_or_else(|| PyError::named("OverflowError", "new position too large".to_string()))?;
    let pos = usize::try_from(pos_i64)
        .map_err(|_| PyError::named("OverflowError", "new position too large".to_string()))?;
    let new_pos = usize::try_from(new_pos_i64)
        .map_err(|_| PyError::named("OverflowError", "new position too large".to_string()))?;
    let buffer = match inst.borrow().attrs.get("_buf").map(|value| value.kind()) {
        Some(ValueKind::Str(buffer)) => buffer.to_string(),
        _ => String::new(),
    };
    let total = buffer.chars().count();

    // Resolve character cursor positions to byte offsets before allocating.
    // StringIO indexes characters, while the Rust backing is UTF-8 bytes.
    let mut prefix_byte = buffer.len();
    let mut end_byte = buffer.len();
    if pos <= total {
        let end = new_pos.min(total);
        for (character_index, (byte_index, _)) in buffer.char_indices().enumerate() {
            if character_index == pos {
                prefix_byte = byte_index;
            }
            if character_index == end {
                end_byte = byte_index;
                break;
            }
        }
    }

    let capacity = if pos > total {
        buffer
            .len()
            .checked_add(pos - total)
            .and_then(|size| size.checked_add(text.len()))
    } else {
        prefix_byte
            .checked_add(text.len())
            .and_then(|size| size.checked_add(buffer.len() - end_byte))
    }
    .ok_or_else(|| PyError::named("OverflowError", "new buffer size too large".to_string()))?;
    if capacity > u32::MAX as usize {
        return Err(PyError::named(
            "OverflowError",
            "new buffer size too large".to_string(),
        ));
    }
    let mut new_buffer = String::new();
    new_buffer
        .try_reserve_exact(capacity)
        .map_err(|_| PyError::named("MemoryError", String::new()))?;
    if pos > total {
        new_buffer.push_str(&buffer);
        new_buffer.extend(std::iter::repeat_n('\0', pos - total));
    } else {
        new_buffer.push_str(&buffer[..prefix_byte]);
    }
    new_buffer.push_str(text);
    if end_byte < buffer.len() {
        new_buffer.push_str(&buffer[end_byte..]);
    }
    inst.borrow_mut()
        .attrs
        .insert("_buf", Value::string(new_buffer));
    set_pos(inst, new_pos_i64);
    Ok(written_len)
}

pub(super) fn seek(
    inst: &Rc<RefCell<PyInstance>>,
    offset: i64,
    whence: i64,
    fn_name: &str,
) -> Result<i64> {
    let char_len = i64::try_from(character_len(inst, fn_name)?)
        .map_err(|_| PyError::named("OverflowError", "new position too large".to_string()))?;
    let new_pos = match whence {
        0 => {
            if offset < 0 {
                return Err(PyError::named(
                    "ValueError",
                    format!("{fn_name}(): negative seek position {offset}"),
                ));
            }
            offset
        }
        1 => {
            if offset != 0 {
                return Err(PyError::named(
                    "OSError",
                    "Can't do nonzero cur-relative seeks".to_string(),
                ));
            }
            get_pos(inst)
        }
        2 => {
            if offset != 0 {
                return Err(PyError::named(
                    "OSError",
                    "Can't do nonzero end-relative seeks".to_string(),
                ));
            }
            char_len
        }
        _ => {
            return Err(PyError::named(
                "ValueError",
                format!("{fn_name}(): unsupported whence value {whence}"),
            ));
        }
    };
    let clamped = new_pos.max(0);
    set_pos(inst, clamped);
    Ok(clamped)
}

pub(super) fn truncate(inst: &Rc<RefCell<PyInstance>>, size: i64, fn_name: &str) -> Result<i64> {
    let char_len = character_len(inst, fn_name)?;
    if size < char_len as i64 {
        let buffer = contents(inst, fn_name)?;
        let new_len = usize::try_from(size).expect("truncate size is validated as non-negative");
        let new_buffer: String = buffer.chars().take(new_len).collect();
        inst.borrow_mut()
            .attrs
            .insert("_buf", Value::string(new_buffer));
    }
    // CPython reports the requested size even though truncating past EOF does
    // not extend an in-memory stream.
    Ok(size)
}

pub(super) fn next_line(inst: &Rc<RefCell<PyInstance>>, fn_name: &str) -> Result<Option<String>> {
    let buffer = contents(inst, fn_name)?;
    let chars: Vec<char> = buffer.chars().collect();
    let total = chars.len();
    let start = usize::try_from(get_pos(inst))
        .unwrap_or(usize::MAX)
        .min(total);
    if start >= total {
        return Ok(None);
    }

    let mut count = 0;
    for character in &chars[start..] {
        count += 1;
        if *character == '\n' {
            break;
        }
    }
    let line = chars[start..start + count].iter().collect();
    set_pos(inst, (start + count) as i64);
    Ok(Some(line))
}
