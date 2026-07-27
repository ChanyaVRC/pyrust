use crate::errors::{PyError, Result};
use crate::object_model::{Value, ValueKind, builtin_type_name};

// ─────────────────────────────────────────────────────────────────────────────
// Typed argument extractors
//
// These helpers are used by `pyrust-builtins` (which cannot depend on the
// interpreter crate) to extract typed values from `&[Value]` slices while
// producing CPython-3.12-compatible `TypeError` messages.
// ─────────────────────────────────────────────────────────────────────────────

/// Extract a `&str` from `v`.
///
/// On type mismatch raises `TypeError: <method>() argument '<param>' must be
/// str, not <typename>` — matching CPython 3.12's message format for
/// `str.removeprefix` and similar single-str-param methods.
pub fn extract_str<'a>(v: &'a Value, method: &str, param: &str) -> Result<&'a str> {
    match v.kind() {
        ValueKind::Str(s) => Ok(s),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{method}() argument '{param}' must be str, not {}",
                builtin_type_name(v),
            ),
        )),
    }
}

/// Extract an `i64` from `v`, accepting both `Int` and `Bool`.
///
/// On type mismatch raises `TypeError: '<typename>' object cannot be
/// interpreted as an integer` — matching CPython 3.12's message for integer
/// width/tabsize arguments.
pub fn extract_int(v: &Value, _method: &str, _param: &str) -> Result<i64> {
    match v.kind() {
        ValueKind::Int(n) => Ok(n),
        ValueKind::Bool(b) => Ok(b as i64),
        ValueKind::BigInt(_) => Err(PyError::named(
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                builtin_type_name(v),
            ),
        )),
    }
}

/// Extract an optional `&str` from `args[idx]`.
///
/// Returns `None` when the slot is absent or `None`-typed.
/// Returns `Some(s)` when the slot holds a `Str`.
/// Returns an error with `TypeError` when the slot holds another type.
pub fn extract_optional_str<'a>(
    args: &'a [Value],
    idx: usize,
    method: &str,
    param: &str,
) -> Result<Option<&'a str>> {
    match args.get(idx).map(|v| v.kind()) {
        None | Some(ValueKind::None) => Ok(None),
        Some(ValueKind::Str(s)) => Ok(Some(s)),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{method}() argument '{param}' must be str, not {}",
                builtin_type_name(&args[idx]),
            ),
        )),
    }
}

/// Extract an optional `i64` from `args[idx]`, accepting `Int` and `Bool`.
///
/// Returns `None` when the slot is absent.
/// Returns `Some(n)` when the slot holds `Int` or `Bool`.
/// Returns an error with `TypeError` when the slot holds another type.
pub fn extract_optional_int(args: &[Value], idx: usize) -> Result<Option<i64>> {
    match args.get(idx).map(|v| v.kind()) {
        None => Ok(None),
        Some(ValueKind::Int(n)) => Ok(Some(n)),
        Some(ValueKind::Bool(b)) => Ok(Some(b as i64)),
        Some(ValueKind::BigInt(_)) => Err(PyError::named(
            "OverflowError",
            "Python int too large to convert to C int",
        )),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'{}' object cannot be interpreted as an integer",
                builtin_type_name(&args[idx]),
            ),
        )),
    }
}

/// Extract a fill `char` from `args[1]`, defaulting to `' '`.
///
/// CPython 3.12 error messages for the fill-character argument:
/// - Wrong type: `TypeError: The fill character must be a unicode character, not <typename>`
/// - Multiple chars: `TypeError: The fill character must be exactly one character long`
pub fn extract_fill_char(args: &[Value]) -> Result<char> {
    match args.get(1).map(|v| v.kind()) {
        None => Ok(' '),
        Some(ValueKind::Str(s)) => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(c),
                _ => Err(PyError::named(
                    "TypeError",
                    "The fill character must be exactly one character long",
                )),
            }
        }
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "The fill character must be a unicode character, not {}",
                builtin_type_name(&args[1]),
            ),
        )),
    }
}

/// Validate argument count for a method that accepts `min..=max` positional args.
///
/// Uses the CPython 3.12 error message format:
/// - `min == max`: `"<method> takes exactly <n> argument(s) (<got> given)"`
/// - `min == 1, max == ∞`: `"<method>() takes at least 1 argument (<got> given)"`
/// - `min < max`: `"<method> expected at least <min> argument, got <got>"` or
///   `"<method> expected at most <max> arguments, got <got>"`
///
/// The messages are chosen to match CPython's output for the specific methods
/// migrated in this refactor.  Pass `max: usize::MAX` for "no upper bound".
pub fn expect_arg_count(args: &[Value], min: usize, max: usize, method: &str) -> Result<()> {
    let got = args.len();
    if got < min {
        if min == max {
            // "str.zfill() takes exactly one argument (0 given)"
            let noun = exactly_n_noun(min);
            return Err(PyError::named(
                "TypeError",
                format!("str.{method}() takes exactly {noun} ({got} given)"),
            ));
        }
        // "center expected at least 1 argument, got 0"
        return Err(PyError::named(
            "TypeError",
            format!("{method} expected at least {min} argument, got {got}"),
        ));
    }
    if max != usize::MAX && got > max {
        if min == max {
            let noun = exactly_n_noun(min);
            return Err(PyError::named(
                "TypeError",
                format!("str.{method}() takes exactly {noun} ({got} given)"),
            ));
        }
        // "center expected at most 2 arguments, got 3"
        return Err(PyError::named(
            "TypeError",
            format!("{method} expected at most {max} arguments, got {got}"),
        ));
    }
    Ok(())
}

fn exactly_n_noun(n: usize) -> String {
    if n == 1 {
        "one argument".to_string()
    } else {
        format!("{n} arguments")
    }
}
