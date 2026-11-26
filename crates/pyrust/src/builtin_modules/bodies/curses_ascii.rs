// `curses.ascii` module — 100% native Rust implementation of Python 3.12's
// `Lib/curses/ascii.py` (constants, membership tests, and converters)
// (issue #3266).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs` to match CPython 3.12.
// Reference: <https://docs.python.org/3.12/library/curses.ascii.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::value_type_name_str;
use crate::value::{Value, ValueKind};

const CONTROL_NAMES: &[&str] = &[
    "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL", "BS", "HT", "LF", "VT", "FF", "CR",
    "SO", "SI", "DLE", "DC1", "DC2", "DC3", "DC4", "NAK", "SYN", "ETB", "CAN", "EM", "SUB", "ESC",
    "FS", "GS", "RS", "US", "SP",
];

fn ctoi(c: &Value) -> Result<i64> {
    match c.kind() {
        ValueKind::Str(s) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Ok(ch as i64),
                _ => Err(PyError::named(
                    "TypeError",
                    format!(
                        "ord() expected a character, but string of length {} found",
                        s.chars().count()
                    ),
                )),
            }
        }
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(i64::from(b)),
        _ => Err(PyError::named(
            "TypeError",
            format!("expected str or int, got '{}'", value_type_name_str(c)),
        )),
    }
}

fn is_upper_i(n: i64) -> bool {
    (65..=90).contains(&n)
}

fn is_lower_i(n: i64) -> bool {
    (97..=122).contains(&n)
}

fn is_alpha_i(n: i64) -> bool {
    is_upper_i(n) || is_lower_i(n)
}

fn is_digit_i(n: i64) -> bool {
    (48..=57).contains(&n)
}

fn is_alnum_i(n: i64) -> bool {
    is_alpha_i(n) || is_digit_i(n)
}

fn is_graph_i(n: i64) -> bool {
    (33..=126).contains(&n)
}

fn is_print_i(n: i64) -> bool {
    (32..=126).contains(&n)
}

pyrust_module! {
    constants {
        "__package__" => Value::string("curses"),
        "NUL" => Value::int(0x00),
        "SOH" => Value::int(0x01),
        "STX" => Value::int(0x02),
        "ETX" => Value::int(0x03),
        "EOT" => Value::int(0x04),
        "ENQ" => Value::int(0x05),
        "ACK" => Value::int(0x06),
        "BEL" => Value::int(0x07),
        "BS" => Value::int(0x08),
        "TAB" => Value::int(0x09),
        "HT" => Value::int(0x09),
        "LF" => Value::int(0x0a),
        "NL" => Value::int(0x0a),
        "VT" => Value::int(0x0b),
        "FF" => Value::int(0x0c),
        "CR" => Value::int(0x0d),
        "SO" => Value::int(0x0e),
        "SI" => Value::int(0x0f),
        "DLE" => Value::int(0x10),
        "DC1" => Value::int(0x11),
        "DC2" => Value::int(0x12),
        "DC3" => Value::int(0x13),
        "DC4" => Value::int(0x14),
        "NAK" => Value::int(0x15),
        "SYN" => Value::int(0x16),
        "ETB" => Value::int(0x17),
        "CAN" => Value::int(0x18),
        "EM" => Value::int(0x19),
        "SUB" => Value::int(0x1a),
        "ESC" => Value::int(0x1b),
        "FS" => Value::int(0x1c),
        "GS" => Value::int(0x1d),
        "RS" => Value::int(0x1e),
        "US" => Value::int(0x1f),
        "SP" => Value::int(0x20),
        "DEL" => Value::int(0x7f),
        "controlnames" => Value::list(
            CONTROL_NAMES.iter().map(|s| Value::string(*s)).collect()
        ),
    }

    fn isalnum(c: PyValue) -> Result<Value> {
        Ok(Value::bool_(is_alnum_i(ctoi(&c.0)?)))
    }

    fn isalpha(c: PyValue) -> Result<Value> {
        Ok(Value::bool_(is_alpha_i(ctoi(&c.0)?)))
    }

    fn isascii(c: PyValue) -> Result<Value> {
        Ok(Value::bool_((0..=127).contains(&ctoi(&c.0)?)))
    }

    fn isblank(c: PyValue) -> Result<Value> {
        let n = ctoi(&c.0)?;
        Ok(Value::bool_(n == 9 || n == 32))
    }

    fn iscntrl(c: PyValue) -> Result<Value> {
        let n = ctoi(&c.0)?;
        Ok(Value::bool_((0..=31).contains(&n) || n == 127))
    }

    fn isdigit(c: PyValue) -> Result<Value> {
        Ok(Value::bool_(is_digit_i(ctoi(&c.0)?)))
    }

    fn isgraph(c: PyValue) -> Result<Value> {
        Ok(Value::bool_(is_graph_i(ctoi(&c.0)?)))
    }

    fn islower(c: PyValue) -> Result<Value> {
        Ok(Value::bool_(is_lower_i(ctoi(&c.0)?)))
    }

    fn isprint(c: PyValue) -> Result<Value> {
        Ok(Value::bool_(is_print_i(ctoi(&c.0)?)))
    }

    fn ispunct(c: PyValue) -> Result<Value> {
        let n = ctoi(&c.0)?;
        Ok(Value::bool_(is_graph_i(n) && !is_alnum_i(n)))
    }

    fn isspace(c: PyValue) -> Result<Value> {
        let n = ctoi(&c.0)?;
        Ok(Value::bool_(matches!(n, 9 | 10 | 11 | 12 | 13 | 32)))
    }

    fn isupper(c: PyValue) -> Result<Value> {
        Ok(Value::bool_(is_upper_i(ctoi(&c.0)?)))
    }

    fn isxdigit(c: PyValue) -> Result<Value> {
        let n = ctoi(&c.0)?;
        Ok(Value::bool_(
            is_digit_i(n) || (65..=70).contains(&n) || (97..=102).contains(&n),
        ))
    }

    fn isctrl(c: PyValue) -> Result<Value> {
        let n = ctoi(&c.0)?;
        Ok(Value::bool_((0..32).contains(&n)))
    }

    fn ismeta(c: PyValue) -> Result<Value> {
        Ok(Value::bool_(ctoi(&c.0)? > 127))
    }

    fn ascii(c: PyValue) -> Result<Value> {
        let bits = ctoi(&c.0)? & 0x7f;
        if matches!(c.0.kind(), ValueKind::Str(_)) {
            Ok(Value::string(
                char::from_u32(bits as u32).unwrap_or('\0').to_string(),
            ))
        } else {
            Ok(Value::int(bits))
        }
    }

    fn ctrl(c: PyValue) -> Result<Value> {
        let bits = ctoi(&c.0)? & 0x1f;
        if matches!(c.0.kind(), ValueKind::Str(_)) {
            Ok(Value::string(
                char::from_u32(bits as u32).unwrap_or('\0').to_string(),
            ))
        } else {
            Ok(Value::int(bits))
        }
    }

    fn alt(c: PyValue) -> Result<Value> {
        let bits = ctoi(&c.0)? | 0x80;
        if matches!(c.0.kind(), ValueKind::Str(_)) {
            let ch = char::from_u32(bits as u32)
                .ok_or_else(|| PyError::named("ValueError", "chr() arg not in range".to_string()))?;
            Ok(Value::string(ch.to_string()))
        } else {
            Ok(Value::int(bits))
        }
    }

    fn unctrl(c: PyValue) -> Result<Value> {
        let bits = ctoi(&c.0)?;
        let low = bits & 0x7f;
        let rep = if bits == 0x7f {
            "^?".to_string()
        } else if is_print_i(low) {
            char::from_u32(low as u32).unwrap_or(' ').to_string()
        } else {
            let code = ((low | 0x20) + 0x20) as u32;
            format!("^{}", char::from_u32(code).unwrap_or('?'))
        };
        if (bits & 0x80) != 0 {
            Ok(Value::string(format!("!{rep}")))
        } else {
            Ok(Value::string(rep))
        }
    }
}
