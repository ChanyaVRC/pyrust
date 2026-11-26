//! CPython-style argument unpacking for the `_csv` C entry points.
//!
//! `_csv` is a C extension, so its wrong-arity diagnostics use the
//! `getargs.c` / Argument Clinic wordings (`" expected at least 1 argument,
//! got 0"`, `"get_dialect() missing required argument 'name' (pos 1)"`, ...)
//! rather than the Python-function wording the macro signatures produce.

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::value::Value;

pub(super) fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Split raw call arguments into positional values and `(name, value)` pairs.
pub(super) fn split(args: &[ExpandedCallArg]) -> (Vec<Value>, Vec<(String, Value)>) {
    let mut positional = Vec::new();
    let mut keywords = Vec::new();
    for arg in args {
        match &arg.name {
            Some(name) => keywords.push((name.clone(), arg.value.clone())),
            None => positional.push(arg.value.clone()),
        }
    }
    (positional, keywords)
}

/// `PyArg_UnpackTuple(args, "", min, max, ...)`: the nameless form `_csv`
/// uses for `reader`, `writer` and `register_dialect`.
pub(super) fn unpack_tuple_anon(positional: &[Value], min: usize, max: usize) -> Result<()> {
    let n = positional.len();
    if n < min {
        return Err(type_error(format!(
            " expected {}{min} argument{}, got {n}",
            if min == max { "" } else { "at least " },
            plural(min),
        )));
    }
    if n > max {
        return Err(type_error(format!(
            " expected {}{max} argument{}, got {n}",
            if min == max { "" } else { "at most " },
            plural(max),
        )));
    }
    Ok(())
}

/// Argument Clinic `_PyArg_UnpackKeywords` for a function whose parameters
/// are all positional-or-keyword; the first `required` are mandatory.
pub(super) fn unpack_clinic(
    fname: &str,
    params: &[&str],
    required: usize,
    args: &[ExpandedCallArg],
) -> Result<Vec<Option<Value>>> {
    let (positional, keywords) = split(args);
    let maxargs = params.len();
    let total = positional.len() + keywords.len();
    if total > maxargs {
        return Err(type_error(format!(
            "{fname}() takes at most {maxargs} {}argument{} ({total} given)",
            if positional.is_empty() {
                "keyword "
            } else {
                ""
            },
            plural(maxargs),
        )));
    }
    let mut slots: Vec<Option<Value>> = vec![None; maxargs];
    let mut used = vec![false; keywords.len()];
    for (i, param) in params.iter().enumerate() {
        if let Some(value) = positional.get(i) {
            slots[i] = Some(value.clone());
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

/// `METH_NOARGS` module function: `"_csv.list_dialects() takes no
/// arguments (1 given)"`.
pub(super) fn no_args(qualname: &str, args: &[ExpandedCallArg]) -> Result<()> {
    if args.iter().any(|a| a.name.is_some()) {
        return Err(type_error(format!(
            "{qualname}() takes no keyword arguments"
        )));
    }
    if !args.is_empty() {
        return Err(type_error(format!(
            "{qualname}() takes no arguments ({} given)",
            args.len()
        )));
    }
    Ok(())
}

/// `METH_O` method (bound `self` already stripped): `"writer.writerow()
/// takes exactly one argument (0 given)"`.
pub(super) fn exactly_one<'a>(qualname: &str, args: &'a [ExpandedCallArg]) -> Result<&'a Value> {
    if args.iter().any(|a| a.name.is_some()) {
        return Err(type_error(format!(
            "{qualname}() takes no keyword arguments"
        )));
    }
    if args.len() != 1 {
        return Err(type_error(format!(
            "{qualname}() takes exactly one argument ({} given)",
            args.len()
        )));
    }
    Ok(&args[0].value)
}

/// The signature of one `def` in CPython's pure-Python `csv.py`.
pub(super) struct PySig {
    /// `co_qualname` (`"DictReader.__init__"`).
    pub(super) qualname: &'static str,
    /// Positional-or-keyword parameters, `self` included.
    pub(super) params: &'static [&'static str],
    /// How many trailing `params` have defaults.
    pub(super) defaults: usize,
    /// `*args` / `**kwds` present.
    pub(super) var_args: bool,
}

/// Arguments bound to a [`PySig`]; unset defaulted slots stay `None`.
pub(super) struct Bound {
    pub(super) slots: Vec<Option<Value>>,
    pub(super) var_args: Vec<Value>,
    pub(super) var_kwds: Vec<(String, Value)>,
}

impl Bound {
    /// The bound value of parameter `i`, or `dflt` when it was not passed.
    pub(super) fn take(&mut self, i: usize, dflt: impl FnOnce() -> Value) -> Value {
        self.slots[i].take().unwrap_or_else(dflt)
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
pub(super) fn bind(sig: &PySig, args: &[ExpandedCallArg]) -> Result<Bound> {
    let (positional, keywords) = split(args);
    let n = sig.params.len();
    let mut slots: Vec<Option<Value>> = vec![None; n];
    let mut var_args = Vec::new();
    for (i, value) in positional.iter().enumerate() {
        if i < n {
            slots[i] = Some(value.clone());
        } else if sig.var_args {
            var_args.push(value.clone());
        }
    }
    let mut var_kwds = Vec::new();
    for (name, value) in keywords {
        match sig.params.iter().position(|p| *p == name) {
            Some(i) if slots[i].is_some() => {
                return Err(type_error(format!(
                    "{}() got multiple values for argument '{name}'",
                    sig.qualname
                )));
            }
            Some(i) => slots[i] = Some(value),
            None if sig.var_args => var_kwds.push((name, value)),
            None => {
                return Err(type_error(format!(
                    "{}() got an unexpected keyword argument '{name}'",
                    sig.qualname
                )));
            }
        }
    }
    if positional.len() > n && !sig.var_args {
        let given = positional.len();
        let (takes, plural) = if sig.defaults > 0 {
            (format!("from {} to {n}", n - sig.defaults), "s")
        } else {
            (n.to_string(), plural(n))
        };
        return Err(type_error(format!(
            "{}() takes {takes} positional argument{plural} but {given} {} given",
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
    Ok(Bound {
        slots,
        var_args,
        var_kwds,
    })
}

/// `PyArg_ParseTupleAndKeywords(args, kwargs, "|OOOOOOOOO", kwlist)` with no
/// function name (the `_csv.Dialect` constructor).  Every parameter is
/// optional and may be given by position or by keyword.
pub(super) fn parse_tuple_and_keywords(
    params: &[&str],
    positional: &[Value],
    keywords: &[(String, Value)],
) -> Result<Vec<Option<Value>>> {
    let len = params.len();
    let nargs = positional.len();
    let total = nargs + keywords.len();
    if total > len {
        return Err(type_error(format!(
            "function takes at most {len} {}argument{} ({total} given)",
            if nargs == 0 { "keyword " } else { "" },
            plural(len),
        )));
    }
    let mut slots: Vec<Option<Value>> = vec![None; len];
    let mut used = vec![false; keywords.len()];
    for (i, param) in params.iter().enumerate() {
        if let Some(value) = positional.get(i) {
            slots[i] = Some(value.clone());
        } else if let Some(k) = keywords.iter().position(|(name, _)| name == param) {
            used[k] = true;
            slots[i] = Some(keywords[k].1.clone());
        }
    }
    if used.iter().any(|u| !u) {
        // An argument given both by position and by name is reported first.
        for (i, param) in params.iter().enumerate().take(nargs) {
            if keywords.iter().any(|(name, _)| name == param) {
                return Err(type_error(format!(
                    "argument for function given by name ('{param}') and position ({})",
                    i + 1
                )));
            }
        }
        if let Some(k) = used.iter().position(|u| !u) {
            return Err(type_error(format!(
                "'{}' is an invalid keyword argument for this function",
                keywords[k].0
            )));
        }
    }
    Ok(slots)
}
