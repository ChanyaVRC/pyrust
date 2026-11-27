//! Python-`def` argument binding for the natively implemented `fractions`
//! functions and methods.
//!
//! CPython's `fractions` is pure Python, so a bad call reports the
//! `initialize_locals` wordings (`"Fraction.__new__() takes from 1 to 3
//! positional arguments but 4 were given"`, `"... missing 1 required
//! positional argument: 'b'"`, ...) under each function's `__qualname__`.
//! [`bind`] reproduces those messages and their check order.  It borrows the
//! bound values straight out of the call's argument slice, so the hot
//! operator path (`a + b` calls `__add__(a, b)` positionally) never allocates.

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;

/// Largest parameter list among the bound `def`s (`_round_to_exponent`).
pub(super) const MAX_PARAMS: usize = 4;

/// The signature of one `def` in CPython 3.12's `Lib/fractions.py`.
pub(super) struct Sig {
    /// `co_qualname`, used as the prefix of every binding error.
    pub(super) qualname: &'static str,
    /// Positional-or-keyword parameters (`self` / `cls` included).
    pub(super) params: &'static [&'static str],
    /// How many trailing `params` have defaults.
    pub(super) defaults: usize,
    /// How many leading `params` are positional-only (before a `/`).
    pub(super) posonly: usize,
}

/// Bound arguments: `None` marks a defaulted parameter that was not passed.
pub(super) type Bound<'a> = [Option<&'a Value>; MAX_PARAMS];

pub(super) fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// `'a'`, `'a' and 'b'`, `'a', 'b', and 'c'` (CPython's `format_missing`).
fn format_names(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
    match quoted.len() {
        0 => String::new(),
        1 => quoted[0].clone(),
        2 => format!("{} and {}", quoted[0], quoted[1]),
        n => format!("{}, and {}", quoted[..n - 1].join(", "), quoted[n - 1]),
    }
}

/// Bind `args` to `sig` the way CPython's `initialize_locals` binds a Python
/// function's arguments, with its exact `TypeError` wordings and order:
/// keyword problems first (in call order), then too many positionals, then
/// missing required parameters.
pub(super) fn bind<'a>(sig: &Sig, args: &'a [ExpandedCallArg]) -> Result<Bound<'a>> {
    let n = sig.params.len();
    debug_assert!(n <= MAX_PARAMS);
    let mut slots: Bound<'a> = [None; MAX_PARAMS];
    let mut given = 0usize;
    let mut has_keywords = false;
    for arg in args {
        if arg.name.is_some() {
            has_keywords = true;
        } else {
            if given < n {
                slots[given] = Some(&arg.value);
            }
            given += 1;
        }
    }
    if has_keywords {
        for arg in args {
            let Some(name) = arg.name.as_deref() else {
                continue;
            };
            let found = sig.params[sig.posonly..]
                .iter()
                .position(|p| *p == name)
                .map(|offset| sig.posonly + offset);
            match found {
                Some(i) if slots[i].is_some() => {
                    return Err(type_error(format!(
                        "{}() got multiple values for argument '{name}'",
                        sig.qualname
                    )));
                }
                Some(i) => slots[i] = Some(&arg.value),
                None => return Err(unknown_keyword(sig, args, name)),
            }
        }
    }
    if given > n {
        let (takes, plural_s) = if sig.defaults > 0 {
            (format!("from {} to {n}", n - sig.defaults), "s")
        } else {
            (n.to_string(), plural(n))
        };
        return Err(type_error(format!(
            "{}() takes {takes} positional argument{plural_s} but {given} {} given",
            sig.qualname,
            if given == 1 { "was" } else { "were" }
        )));
    }
    let required = n - sig.defaults;
    let missing: Vec<&str> = (0..required)
        .filter(|&i| slots[i].is_none())
        .map(|i| sig.params[i])
        .collect();
    if !missing.is_empty() {
        return Err(type_error(format!(
            "{}() missing {} required positional argument{}: {}",
            sig.qualname,
            missing.len(),
            plural(missing.len()),
            format_names(&missing)
        )));
    }
    Ok(slots)
}

/// The error for a keyword that names no positional-or-keyword parameter:
/// CPython first reports every positional-only parameter passed by keyword
/// (`positional_only_passed_as_keyword`), else the unexpected keyword.
fn unknown_keyword(sig: &Sig, args: &[ExpandedCallArg], name: &str) -> PyError {
    let posonly_names: Vec<&str> = sig.params[..sig.posonly]
        .iter()
        .copied()
        .flat_map(|param| {
            args.iter()
                .filter(move |arg| arg.name.as_deref() == Some(param))
                .map(move |_| param)
        })
        .collect();
    if !posonly_names.is_empty() {
        return type_error(format!(
            "{}() got some positional-only arguments passed as keyword arguments: '{}'",
            sig.qualname,
            posonly_names.join(", ")
        ));
    }
    type_error(format!(
        "{}() got an unexpected keyword argument '{name}'",
        sig.qualname
    ))
}
