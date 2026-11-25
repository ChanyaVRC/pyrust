// `_string` internal module — 100% native-Rust Python 3.12 PEP 3101 format
// string parser helpers (issue #3250).
//
// Exposes `formatter_parser(format_string)` and
// `formatter_field_name_split(field_name)` matching CPython 3.12's `_string`
// C-extension module.
//
// Reference: CPython 3.12 `Objects/stringlib/unicode_format.h`

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyStr;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

fn call_iter(interp: &mut Interpreter, list_val: Value) -> Result<Value> {
    interp.call_function_expanded(
        Value::builtin_function("iter"),
        &[ExpandedCallArg {
            name: None,
            value: list_val,
        }],
    )
}

fn parse_format_string(s: &str) -> Result<Vec<Value>> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    let mut out = Vec::new();

    while i < n {
        let mut literal = String::new();
        let mut found_field = false;
        let mut ended_on_escaped_brace = false;

        while i < n {
            match bytes[i] {
                b'{' => {
                    if i + 1 < n && bytes[i + 1] == b'{' {
                        literal.push('{');
                        i += 2;
                        ended_on_escaped_brace = true;
                        break;
                    } else {
                        i += 1;
                        found_field = true;
                        break;
                    }
                }
                b'}' => {
                    if i + 1 < n && bytes[i + 1] == b'}' {
                        literal.push('}');
                        i += 2;
                        ended_on_escaped_brace = true;
                        break;
                    } else {
                        return Err(PyError::named(
                            "ValueError",
                            "Single '}' encountered in format string",
                        ));
                    }
                }
                _ => {
                    let ch = s[i..].chars().next().unwrap();
                    literal.push(ch);
                    i += ch.len_utf8();
                }
            }
        }

        if ended_on_escaped_brace {
            out.push(Value::tuple(vec![
                Value::string(literal),
                Value::none(),
                Value::none(),
                Value::none(),
            ]));
            continue;
        }

        if !found_field {
            if !literal.is_empty() {
                out.push(Value::tuple(vec![
                    Value::string(literal),
                    Value::none(),
                    Value::none(),
                    Value::none(),
                ]));
            }
            break;
        }

        // Parse replacement field: field_name ['!' conversion] [':' format_spec] '}'
        let field_start = i;
        let mut depth = 0usize;
        let mut in_bracket = false;
        let mut field_end = None;
        let mut conv_pos = None;
        let mut spec_pos = None;

        while i < n {
            let b = bytes[i];
            if spec_pos.is_some() {
                if b == b'{' {
                    depth += 1;
                } else if b == b'}' {
                    if depth == 0 {
                        field_end = Some(i);
                        i += 1;
                        break;
                    }
                    depth -= 1;
                }
                i += 1;
            } else {
                if b == b'[' {
                    in_bracket = true;
                } else if b == b']' {
                    in_bracket = false;
                } else if !in_bracket {
                    if b == b'!' && conv_pos.is_none() {
                        conv_pos = Some(i);
                        i += 1;
                        continue;
                    } else if b == b':' {
                        spec_pos = Some(i);
                        i += 1;
                        continue;
                    } else if b == b'}' {
                        field_end = Some(i);
                        i += 1;
                        break;
                    } else if b == b'{' {
                        return Err(PyError::named("ValueError", "unexpected '{' in field name"));
                    }
                }
                i += 1;
            }
        }

        let Some(end_idx) = field_end else {
            return Err(PyError::named(
                "ValueError",
                "Single '{' encountered in format string",
            ));
        };

        let name_end = conv_pos.or(spec_pos).unwrap_or(end_idx);
        let field_name = &s[field_start..name_end];
        let conversion = match conv_pos {
            Some(cp) => {
                let c_end = spec_pos.unwrap_or(end_idx);
                let conv_str = &s[cp + 1..c_end];
                if conv_str.len() != 1 {
                    return Err(PyError::named(
                        "ValueError",
                        "expected ':' after conversion specifier",
                    ));
                }
                Value::string(conv_str)
            }
            None => Value::none(),
        };
        let format_spec = match spec_pos {
            Some(sp) => Value::string(&s[sp + 1..end_idx]),
            None => Value::string(""),
        };

        out.push(Value::tuple(vec![
            Value::string(literal),
            Value::string(field_name),
            format_spec,
            conversion,
        ]));
    }

    Ok(out)
}

fn parse_field_name_split(s: &str) -> Result<(Value, Vec<Value>)> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;

    while i < n && bytes[i] != b'.' && bytes[i] != b'[' {
        i += 1;
    }

    let first_str = &s[..i];
    let first = if !first_str.is_empty() && first_str.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(num) = first_str.parse::<i64>() {
            Value::int(num)
        } else {
            Value::string(first_str)
        }
    } else {
        Value::string(first_str)
    };

    let mut rest = Vec::new();
    while i < n {
        match bytes[i] {
            b'.' => {
                i += 1;
                let start = i;
                while i < n && bytes[i] != b'.' && bytes[i] != b'[' {
                    i += 1;
                }
                if i == start {
                    return Err(PyError::named(
                        "ValueError",
                        "Empty attribute in format string",
                    ));
                }
                rest.push(Value::tuple(vec![
                    Value::bool_(true),
                    Value::string(&s[start..i]),
                ]));
            }
            b'[' => {
                i += 1;
                let start = i;
                while i < n && bytes[i] != b']' {
                    i += 1;
                }
                if i >= n {
                    return Err(PyError::named("ValueError", "Missing ']' in format string"));
                }
                let idx_str = &s[start..i];
                i += 1; // skip ']'
                if i < n && bytes[i] != b'.' && bytes[i] != b'[' {
                    return Err(PyError::named(
                        "ValueError",
                        "Only '.' or '[' may follow ']' in format field specifier",
                    ));
                }
                let idx_val = if !idx_str.is_empty() && idx_str.bytes().all(|b| b.is_ascii_digit())
                {
                    idx_str
                        .parse::<i64>()
                        .map(Value::int)
                        .unwrap_or_else(|_| Value::string(idx_str))
                } else {
                    Value::string(idx_str)
                };
                rest.push(Value::tuple(vec![Value::bool_(false), idx_val]));
            }
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    "Invalid character in field name",
                ));
            }
        }
    }

    Ok((first, rest))
}

pyrust_module! {
    fn formatter_parser(#[positional_only] format_string: PyStr<'_>) -> Result<Value> {
        let items = parse_format_string(&format_string)?;
        call_iter(_interp, Value::list(items))
    }

    fn formatter_field_name_split(#[positional_only] field_name: PyStr<'_>) -> Result<Value> {
        let (first, rest) = parse_field_name_split(&field_name)?;
        let rest_iter = call_iter(_interp, Value::list(rest))?;
        Ok(Value::tuple(vec![first, rest_iter]))
    }
}
