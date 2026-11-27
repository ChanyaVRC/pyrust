//! CPython `initialize_locals` argument binding for the pure-Python `def`s of
//! `Lib/json` (`json/__init__.py`, `json/decoder.py`, `json/encoder.py`).
//!
//! The typed `pyrust_module!` signatures report arity errors in the macro's
//! own wording; these functions are plain Python functions in CPython, so
//! their `TypeError`s use the interpreter's `initialize_locals` wording and
//! check order instead: unexpected / duplicate keywords first, then too many
//! positional arguments, then missing positional arguments.

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;

pub(crate) fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// The signature of one `def` in `Lib/json`.
pub(crate) struct Sig {
    /// `co_qualname` (`"JSONEncoder.__init__"`).
    pub(crate) qualname: &'static str,
    /// Positional-or-keyword parameters, `self` included.
    pub(crate) params: &'static [&'static str],
    /// How many trailing `params` have defaults.
    pub(crate) defaults: usize,
    /// Keyword-only parameters; every one in `Lib/json` has a default.
    pub(crate) kwonly: &'static [&'static str],
    /// `**kw` present.
    pub(crate) var_kw: bool,
}

/// Arguments bound to a [`Sig`]: the positional-or-keyword slots followed by
/// the keyword-only slots, `None` where the default applies.
pub(crate) struct Bound {
    slots: Vec<Option<Value>>,
    pub(crate) var_kw: Vec<(String, Value)>,
}

impl Bound {
    /// The bound value of slot `i`, or `None` when it was not passed.
    pub(crate) fn take(&mut self, i: usize) -> Option<Value> {
        self.slots[i].take()
    }

    /// The bound value of slot `i`, or `dflt()` when it was not passed.
    pub(crate) fn take_or(&mut self, i: usize, dflt: impl FnOnce() -> Value) -> Value {
        self.slots[i].take().unwrap_or_else(dflt)
    }

    /// The bound value of slot `i`, or `None` (the Python object) when it was
    /// not passed.
    pub(crate) fn take_or_none(&mut self, i: usize) -> Value {
        self.take_or(i, Value::none)
    }
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

/// Bind call arguments the way CPython's `initialize_locals` binds a Python
/// function's, with its exact `TypeError` wordings and check order.
pub(crate) fn bind(sig: &Sig, args: &[ExpandedCallArg]) -> Result<Bound> {
    let n = sig.params.len();
    let mut slots: Vec<Option<Value>> = vec![None; n + sig.kwonly.len()];
    let mut given = 0usize;
    for arg in args.iter().filter(|a| a.name.is_none()) {
        if given < n {
            slots[given] = Some(arg.value.clone());
        }
        given += 1;
    }
    let mut var_kw = Vec::new();
    for arg in args {
        let Some(name) = &arg.name else { continue };
        match sig
            .params
            .iter()
            .chain(sig.kwonly.iter())
            .position(|p| p == name)
        {
            Some(i) if slots[i].is_some() => {
                return Err(type_error(format!(
                    "{}() got multiple values for argument '{name}'",
                    sig.qualname
                )));
            }
            Some(i) => slots[i] = Some(arg.value.clone()),
            None if sig.var_kw => var_kw.push((name.clone(), arg.value.clone())),
            None => {
                return Err(type_error(format!(
                    "{}() got an unexpected keyword argument '{name}'",
                    sig.qualname
                )));
            }
        }
    }
    if given > n {
        let kwonly_given = slots[n..].iter().filter(|s| s.is_some()).count();
        let (takes, plural_s) = if sig.defaults > 0 {
            (format!("from {} to {n}", n - sig.defaults), "s")
        } else {
            (n.to_string(), plural(n))
        };
        let kwonly_sig = if kwonly_given > 0 {
            format!(
                " positional argument{} (and {kwonly_given} keyword-only argument{})",
                plural(given),
                plural(kwonly_given)
            )
        } else {
            String::new()
        };
        return Err(type_error(format!(
            "{}() takes {takes} positional argument{plural_s} but {given}{kwonly_sig} {} given",
            sig.qualname,
            if given == 1 && kwonly_given == 0 {
                "was"
            } else {
                "were"
            }
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
    Ok(Bound { slots, var_kw })
}

/// `PyArg_ParseTupleAndKeywords(args, kwds, "<formats>:<fname>", kwlist)`
/// for a C entry point whose parameters may all be given by position or by
/// keyword; the first `required` are mandatory.
pub(crate) fn parse_c(
    fname: &str,
    params: &[&str],
    required: usize,
    args: &[ExpandedCallArg],
) -> Result<Vec<Option<Value>>> {
    let positional: Vec<&Value> = args
        .iter()
        .filter(|a| a.name.is_none())
        .map(|a| &a.value)
        .collect();
    let keywords: Vec<(&str, &Value)> = args
        .iter()
        .filter_map(|a| a.name.as_deref().map(|n| (n, &a.value)))
        .collect();
    let max = params.len();
    let total = positional.len() + keywords.len();
    if total > max {
        return Err(type_error(format!(
            "{fname}() takes at most {max} {}argument{} ({total} given)",
            if positional.is_empty() {
                "keyword "
            } else {
                ""
            },
            plural(max),
        )));
    }
    let mut slots: Vec<Option<Value>> = vec![None; max];
    let mut used = vec![false; keywords.len()];
    for (i, param) in params.iter().enumerate() {
        if let Some(value) = positional.get(i) {
            if keywords.iter().any(|(name, _)| name == param) {
                return Err(type_error(format!(
                    "argument for {fname}() given by name ('{param}') and position ({})",
                    i + 1
                )));
            }
            slots[i] = Some((*value).clone());
            continue;
        }
        if let Some(k) = keywords.iter().position(|(name, _)| name == param) {
            used[k] = true;
            slots[i] = Some(keywords[k].1.clone());
            continue;
        }
        if i < required {
            return Err(type_error(format!(
                "{fname}() missing required argument '{param}' (pos {})",
                i + 1
            )));
        }
    }
    if let Some(k) = used.iter().position(|u| !u) {
        return Err(type_error(format!(
            "'{}' is an invalid keyword argument for {fname}()",
            keywords[k].0
        )));
    }
    Ok(slots)
}

/// Raw call arguments from positional values and `(name, value)` pairs.
pub(crate) fn expand(positional: &[Value], keywords: &[(String, Value)]) -> Vec<ExpandedCallArg> {
    let mut expanded: Vec<ExpandedCallArg> = positional
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    expanded.extend(keywords.iter().map(|(name, value)| ExpandedCallArg {
        name: Some(name.clone()),
        value: value.clone(),
    }));
    expanded
}
