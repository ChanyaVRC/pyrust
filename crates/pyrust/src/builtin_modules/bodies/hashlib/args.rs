//! CPython-style (Argument Clinic / `getargs.c`) argument unpacking.
//!
//! `_hashlib` and `_blake2` are C modules, so their wrong-arity diagnostics
//! use the clinic wording (`takes at most 1 positional argument (2 given)`,
//! `'foo' is an invalid keyword argument for md5()`, ...) rather than the
//! Python-function wording the macro-generated signatures produce.

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;

/// One C-level callable signature.
pub(super) struct Sig {
    /// Name shown in diagnostics (`openssl_md5`, `blake2b`, `digest`, ...).
    pub(super) name: &'static str,
    /// Positional parameters (those that may also be passed by keyword).
    pub(super) positional: &'static [&'static str],
    /// How many leading entries of `positional` are positional-only (`/`).
    pub(super) positional_only: usize,
    /// How many leading entries of `positional` are required.
    pub(super) required: usize,
    /// Keyword-only parameters (after `*`).
    pub(super) kwonly: &'static [&'static str],
}

fn type_error(msg: String) -> PyError {
    PyError::named("TypeError", msg)
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Bind `args` to `sig`; slot `i` is `positional[i]` then the keyword-only
/// names. Missing optional parameters stay `None`.
pub(super) fn unpack(sig: &Sig, args: &[ExpandedCallArg]) -> Result<Vec<Option<Value>>> {
    let maxpos = sig.positional.len();
    let total = maxpos + sig.kwonly.len();
    let nargs = args.iter().filter(|a| a.name.is_none()).count();
    let nkw = args.len() - nargs;
    if nargs > maxpos {
        // Signatures without keyword-only parameters go through the older
        // `vgetargskeywords` path, which always says "at most".
        let qualifier = if sig.required < maxpos || sig.kwonly.is_empty() {
            "at most"
        } else {
            "exactly"
        };
        let noun = if sig.kwonly.is_empty() {
            "argument"
        } else {
            "positional argument"
        };
        return Err(type_error(format!(
            "{}() takes {qualifier} {maxpos} {noun}{} ({nargs} given)",
            sig.name,
            plural(maxpos),
        )));
    }
    let mut slots: Vec<Option<Value>> = vec![None; total];
    let mut unknown_keyword: Option<&str> = None;
    for (slot, arg) in slots
        .iter_mut()
        .zip(args.iter().filter(|a| a.name.is_none()))
    {
        *slot = Some(arg.value.clone());
    }
    if nkw > 0 {
        if nargs + nkw > total {
            let kw = if nargs == 0 { "keyword " } else { "" };
            return Err(type_error(format!(
                "{}() takes at most {total} {kw}argument{} ({} given)",
                sig.name,
                plural(total),
                nargs + nkw,
            )));
        }
        for arg in args {
            let Some(name) = arg.name.as_deref() else {
                continue;
            };
            let index = sig
                .positional
                .iter()
                .position(|p| *p == name)
                .filter(|i| *i >= sig.positional_only)
                .or_else(|| {
                    sig.kwonly
                        .iter()
                        .position(|p| *p == name)
                        .map(|i| maxpos + i)
                });
            let Some(index) = index else {
                // CPython reports a missing required argument before an
                // unknown keyword, so remember the first one.
                unknown_keyword.get_or_insert(name);
                continue;
            };
            if slots[index].is_some() {
                return Err(type_error(format!(
                    "argument for {}() given by name ('{name}') and position ({})",
                    sig.name,
                    index + 1
                )));
            }
            slots[index] = Some(arg.value.clone());
        }
    }
    for (i, slot) in slots.iter().enumerate().take(sig.required) {
        if slot.is_none() {
            return Err(type_error(format!(
                "{}() missing required argument '{}' (pos {})",
                sig.name,
                sig.positional[i],
                i + 1
            )));
        }
    }
    if let Some(name) = unknown_keyword {
        return Err(type_error(format!(
            "'{name}' is an invalid keyword argument for {}()",
            sig.name
        )));
    }
    Ok(slots)
}

/// `METH_NOARGS`: `NAME() takes no arguments (N given)`.
pub(super) fn no_args(qualified: &str, args: &[ExpandedCallArg]) -> Result<()> {
    // `args[0]` is the receiver.
    if args.iter().skip(1).any(|a| a.name.is_some()) {
        return Err(type_error(format!(
            "{qualified}() takes no keyword arguments"
        )));
    }
    if args.len() > 1 {
        return Err(type_error(format!(
            "{qualified}() takes no arguments ({} given)",
            args.len() - 1
        )));
    }
    Ok(())
}

/// `METH_O`: `NAME() takes exactly one argument (N given)`.
pub(super) fn one_arg<'a>(qualified: &str, args: &'a [ExpandedCallArg]) -> Result<&'a Value> {
    if args.iter().skip(1).any(|a| a.name.is_some()) {
        return Err(type_error(format!(
            "{qualified}() takes no keyword arguments"
        )));
    }
    if args.len() != 2 {
        return Err(type_error(format!(
            "{qualified}() takes exactly one argument ({} given)",
            args.len().saturating_sub(1)
        )));
    }
    Ok(&args[1].value)
}
