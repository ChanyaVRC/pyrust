// ASCII string index and contiguous-slice fast paths for expression evaluation.

/// O(1) ASCII string index: when the backing `str` is all-ASCII, char index is
/// the byte index. Caller has already confirmed `target.str_is_ascii()`.
#[inline]
fn fast_str_ascii_index(text: &str, index: &Value) -> Result<Value> {
    let idx = normalize_index(index, text.len(), "string")?;
    let b = text.as_bytes()[idx];
    Ok(Value::string((b as char).encode_utf8(&mut [0u8; 4]) as &str))
}

/// Copy a contiguous `step == 1` slice. Bounds have already been normalized.
#[inline]
fn fast_slice_contiguous(
    target: &Value,
    start: i64,
    end: i64,
    str_is_ascii: bool,
) -> Result<Value> {
    let s = start as usize;
    let e = (end.max(start)) as usize;
    match target.kind() {
        ValueKind::List(items) => Ok(Value::list(items[s..e].to_vec())),
        ValueKind::Tuple(items) => Ok(Value::tuple(items[s..e].to_vec())),
        ValueKind::Bytes(rc) => Ok(Value::bytes(rc[s..e].to_vec())),
        ValueKind::Str(_) => {
            if s >= e {
                return Ok(Value::string(String::new()));
            }
            if str_is_ascii {
                return Ok(target.string_slice(s, e));
            }
            let byte_start = target.str_codepoint_byte_offset(s);
            let byte_end = target.str_codepoint_byte_offset(e);
            Ok(target.string_slice(byte_start, byte_end))
        }
        _ => unreachable!(),
    }
}
