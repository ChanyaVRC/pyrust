//! `_csv.Dialect` construction and validation — CPython 3.12.3's
//! `dialect_new` (`Modules/_csv.c`).
//!
//! A dialect is built from an optional base `dialect` (a registered name, a
//! `_csv.Dialect` instance, or any object whose attributes are read) plus
//! keyword overrides.  Values are converted and validated in exactly the
//! order CPython uses, so the first reported error matches.

use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::Interpreter;
use crate::value::{Value, ValueKind};

use super::args;

/// `QuoteStyle` (`QUOTE_MINIMAL` ... `QUOTE_NOTNULL`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Quoting {
    Minimal,
    All,
    NonNumeric,
    None,
    Strings,
    NotNull,
}

impl Quoting {
    fn from_c_int(value: i64) -> Option<Self> {
        Some(match value {
            0 => Quoting::Minimal,
            1 => Quoting::All,
            2 => Quoting::NonNumeric,
            3 => Quoting::None,
            4 => Quoting::Strings,
            5 => Quoting::NotNull,
            _ => return None,
        })
    }

    pub(super) fn as_int(self) -> i64 {
        match self {
            Quoting::Minimal => 0,
            Quoting::All => 1,
            Quoting::NonNumeric => 2,
            Quoting::None => 3,
            Quoting::Strings => 4,
            Quoting::NotNull => 5,
        }
    }
}

/// The validated parameters of one `_csv.Dialect` (`DialectObj`).
pub(super) struct DialectCfg {
    pub(super) doublequote: bool,
    pub(super) skipinitialspace: bool,
    pub(super) strict: bool,
    pub(super) quoting: Quoting,
    pub(super) delimiter: char,
    /// `None` is CPython's `NOT_SET`.
    pub(super) quotechar: Option<char>,
    /// `None` is CPython's `NOT_SET`.
    pub(super) escapechar: Option<char>,
    /// The exact `str` object supplied (CPython keeps a reference to it).
    pub(super) lineterminator: Value,
    pub(super) lineterminator_text: String,
}

/// Native payload of a `_csv.Dialect` instance.
pub(super) struct DialectState {
    pub(super) cfg: Rc<DialectCfg>,
}

/// `dialect_kws` in `_csv.c`, in parameter order.
const DIALECT_KWS: [&str; 9] = [
    "dialect",
    "delimiter",
    "doublequote",
    "escapechar",
    "lineterminator",
    "quotechar",
    "quoting",
    "skipinitialspace",
    "strict",
];

/// `dialect_new(type, args, kwargs)`: build (or reuse) a dialect of class
/// `cls`.
pub(super) fn dialect_new(
    interp: &mut Interpreter,
    cls: &Value,
    positional: &[Value],
    keywords: &[(String, Value)],
) -> Result<Value> {
    let mut slots = args::parse_tuple_and_keywords(&DIALECT_KWS, positional, keywords)?;
    let mut dialect = slots[0].take();
    if let Some(base) = dialect.take() {
        let base = if super::str_text(&base).is_some() {
            super::registry_get(interp, &base)?
        } else {
            base
        };
        // Can we reuse this instance?
        if super::is_native_dialect(&base) && slots[1..].iter().all(Option::is_none) {
            return Ok(base);
        }
        dialect = Some(base);
    }
    if let Some(base) = &dialect {
        for (slot, name) in slots.iter_mut().zip(DIALECT_KWS).skip(1) {
            if slot.is_none() {
                // `PyObject_GetAttrString` failures of any kind are cleared.
                *slot = interp.get_attr(base, name).ok();
            }
        }
    }
    let cfg = build_cfg(interp, &slots)?;
    let inst = super::new_instance(cls)?;
    super::set_native_state(&inst, DialectState { cfg: Rc::new(cfg) })?;
    Ok(inst)
}

fn build_cfg(interp: &mut Interpreter, slots: &[Option<Value>]) -> Result<DialectCfg> {
    let delimiter = set_char("delimiter", slots[1].as_ref(), ',')?;
    let doublequote = set_bool(interp, slots[2].as_ref(), true)?;
    let escapechar = set_char_or_none("escapechar", slots[3].as_ref(), None)?;
    let lineterminator = set_str("lineterminator", slots[4].as_ref())?;
    let quotechar = set_char_or_none("quotechar", slots[5].as_ref(), Some('"'))?;
    let quoting_int = set_int("quoting", slots[6].as_ref(), 0)?;
    let skipinitialspace = set_bool(interp, slots[7].as_ref(), false)?;
    let strict = set_bool(interp, slots[8].as_ref(), false)?;

    let Some(mut quoting) = Quoting::from_c_int(quoting_int) else {
        return Err(args::type_error("bad \"quoting\" value"));
    };
    if slots[5].as_ref().is_some_and(Value::is_none) && slots[6].is_none() {
        quoting = Quoting::None;
    }
    if quoting != Quoting::None && quotechar.is_none() {
        return Err(args::type_error("quotechar must be set if quoting enabled"));
    }
    let Some((lineterminator, lineterminator_text)) = lineterminator else {
        return Err(args::type_error("lineterminator must be set"));
    };
    Ok(DialectCfg {
        doublequote,
        skipinitialspace,
        strict,
        quoting,
        delimiter,
        quotechar,
        escapechar,
        lineterminator,
        lineterminator_text,
    })
}

/// The single character of a one-character string, if it is one.
fn single_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

fn one_char_error(name: &str) -> PyError {
    args::type_error(format!("\"{name}\" must be a 1-character string"))
}

/// `_set_char`: a required one-character string.
fn set_char(name: &str, src: Option<&Value>, dflt: char) -> Result<char> {
    let Some(src) = src else {
        return Ok(dflt);
    };
    let Some(text) = super::str_text(src) else {
        return Err(args::type_error(format!(
            "\"{name}\" must be string, not {}",
            super::type_name(src)
        )));
    };
    single_char(&text).ok_or_else(|| one_char_error(name))
}

/// `_set_char_or_none`: a one-character string or `None` (`NOT_SET`).
fn set_char_or_none(name: &str, src: Option<&Value>, dflt: Option<char>) -> Result<Option<char>> {
    let Some(src) = src else {
        return Ok(dflt);
    };
    if src.is_none() {
        return Ok(None);
    }
    let Some(text) = super::str_text(src) else {
        return Err(args::type_error(format!(
            "\"{name}\" must be string or None, not {}",
            super::type_name(src)
        )));
    };
    single_char(&text)
        .map(Some)
        .ok_or_else(|| one_char_error(name))
}

/// `_set_str`: any string; `None` leaves it unset.
fn set_str(name: &str, src: Option<&Value>) -> Result<Option<(Value, String)>> {
    let Some(src) = src else {
        return Ok(Some((Value::string("\r\n"), "\r\n".to_string())));
    };
    if src.is_none() {
        return Ok(None);
    }
    let Some(text) = super::str_text(src) else {
        return Err(args::type_error(format!("\"{name}\" must be a string")));
    };
    let text = text.into_owned();
    Ok(Some((src.clone(), text)))
}

/// `_set_int`: an exact `int` that fits a C `int`.
fn set_int(name: &str, src: Option<&Value>, dflt: i64) -> Result<i64> {
    let Some(src) = src else {
        return Ok(dflt);
    };
    let value = match src.kind() {
        ValueKind::Int(v) => Some(v),
        ValueKind::BigInt(_) => None,
        _ => {
            return Err(args::type_error(format!("\"{name}\" must be an integer")));
        }
    };
    match value {
        Some(v) if i32::try_from(v).is_ok() => Ok(v),
        _ => Err(PyError::named(
            "OverflowError",
            "Python int too large to convert to C int".to_string(),
        )),
    }
}

/// `_set_bool`: the truth value of any object.
fn set_bool(interp: &mut Interpreter, src: Option<&Value>, dflt: bool) -> Result<bool> {
    match src {
        None => Ok(dflt),
        Some(src) => interp.truthy_value(src),
    }
}

/// `get_char_or_None`.
pub(super) fn char_or_none(c: Option<char>) -> Value {
    match c {
        Some(c) => Value::string(c.to_string()),
        None => Value::none(),
    }
}
