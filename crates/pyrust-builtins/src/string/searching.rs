/// Validate that the first argument to a `str` search method is itself a `str`,
/// returning the borrowed substring. `method` is the bare method name (e.g.
/// `"index"`) threaded into the missing-argument error message.
fn require_str_arg<'a>(args: &'a [Value], method: &str) -> Result<&'a str> {
    match args.first().map(|v| v.kind()) {
        Some(ValueKind::Str(sub)) => Ok(sub),
        Some(_) => Err(PyError::named(
            "TypeError",
            format!(
                "must be str, not {}",
                builtin_type_name(args.first().unwrap())
            ),
        )),
        None => Err(PyError::named(
            "TypeError",
            format!("str.{method}() requires a str argument"),
        )),
    }
}

fn str_index(s: &str, is_ascii: bool, args: &[Value]) -> Result<Value> {
    let sub = require_str_arg(args, "index")?;
    let Some((start, end)) = str_slice_args(s, is_ascii, args)? else {
        return Err(PyError::named(
            "ValueError",
            "substring not found".to_string(),
        ));
    };
    match find_in(&s[start..end], sub) {
        Some(byte_pos) => Ok(Value::int(
            byte_to_char_idx(s, is_ascii, start + byte_pos) as i64
        )),
        None => Err(PyError::named(
            "ValueError",
            "substring not found".to_string(),
        )),
    }
}

/// First byte offset of `sub` in `haystack`.  The empty needle matches at the
/// window start without asking std: its empty-needle searcher decodes the
/// `char` at the match position, which is UB on a lone surrogate (#3315).
#[inline]
fn find_in(haystack: &str, sub: &str) -> Option<usize> {
    if sub.is_empty() {
        Some(0)
    } else {
        haystack.find(sub)
    }
}

/// Last byte offset of `sub` in `haystack`; see [`find_in`].
#[inline]
fn rfind_in(haystack: &str, sub: &str) -> Option<usize> {
    if sub.is_empty() {
        Some(haystack.len())
    } else {
        haystack.rfind(sub)
    }
}

fn str_count(s: &str, is_ascii: bool, args: &[Value]) -> Result<Value> {
    let sub = require_str_arg(args, "count")?;
    let Some((start, end)) = str_slice_args(s, is_ascii, args)? else {
        // Inverted window: CPython returns 0 for all substrings including empty.
        return Ok(Value::int(0));
    };
    if sub.is_empty() {
        let haystack = &s[start..end];
        // ASCII: char count == byte count (#2032).  A substring of an all-ASCII
        // string is all-ASCII, so the cached whole-string flag applies directly.
        let count = if is_ascii || haystack.is_ascii() {
            haystack.len()
        } else {
            str_codepoint_count(haystack)
        };
        return Ok(Value::int((count + 1) as i64));
    }
    let haystack = &s[start..end];
    let n = haystack.match_indices(sub).count();
    Ok(Value::int(n as i64))
}

fn str_find(s: &str, is_ascii: bool, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = require_str_arg(args, "find")?;
    let Some((start, end)) = str_slice_args(s, is_ascii, args)? else {
        if raise_on_miss {
            return Err(PyError::named(
                "ValueError",
                "substring not found".to_string(),
            ));
        }
        return Ok(Value::int(-1));
    };
    match find_in(&s[start..end], sub) {
        Some(byte_pos) => Ok(Value::int(
            byte_to_char_idx(s, is_ascii, start + byte_pos) as i64
        )),
        None => {
            if raise_on_miss {
                Err(PyError::named(
                    "ValueError",
                    "substring not found".to_string(),
                ))
            } else {
                Ok(Value::int(-1))
            }
        }
    }
}

fn str_rfind(s: &str, is_ascii: bool, args: &[Value], raise_on_miss: bool) -> Result<Value> {
    let sub = require_str_arg(args, "rfind")?;
    let Some((start, end)) = str_slice_args(s, is_ascii, args)? else {
        if raise_on_miss {
            return Err(PyError::named(
                "ValueError",
                "substring not found".to_string(),
            ));
        }
        return Ok(Value::int(-1));
    };
    match rfind_in(&s[start..end], sub) {
        Some(byte_pos) => Ok(Value::int(
            byte_to_char_idx(s, is_ascii, start + byte_pos) as i64
        )),
        None => {
            if raise_on_miss {
                Err(PyError::named(
                    "ValueError",
                    "substring not found".to_string(),
                ))
            } else {
                Ok(Value::int(-1))
            }
        }
    }
}
