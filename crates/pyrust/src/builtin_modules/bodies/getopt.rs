// `getopt` module — 100% native Rust implementation of Python 3.12's `getopt`
// module (issues #3230, #3330).
//
// CPython 3.12's `getopt` provides C-style and GNU-style command-line option
// parsing: `getopt()`, `gnu_getopt()`, the `GetoptError` exception (`error` is
// its backward-compatible alias) and the module-level helpers `do_longs`,
// `long_has_args`, `do_shorts`, and `short_has_arg`.
//
// Reference: <https://docs.python.org/3/library/getopt.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{Value, ValueKind};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn str_list(items: &[&str]) -> Value {
    Value::list(items.iter().map(|s| Value::string(*s)).collect())
}

/// Build `getopt.GetoptError(msg, opt)` from the live module attribute (as
/// `raise GetoptError(...)` resolves the module global in CPython).
fn getopt_error(interp: &mut Interpreter, msg: String, opt: &str) -> PyError {
    let build = |interp: &mut Interpreter| -> Result<Value> {
        let module = interp.load_module("getopt")?;
        let class = interp.get_attr(&module, "GetoptError")?;
        call_pos(interp, class, &[Value::string(msg), Value::string(opt)])
    };
    match build(interp) {
        Ok(instance) => PyError::raised(instance),
        Err(err) => err,
    }
}

fn type_error(msg: String) -> PyError {
    PyError::named("TypeError", msg)
}

/// What `args[i]` slicing (`args[1:]`) produces for the container passed in.
enum SeqKind {
    List,
    Tuple,
    Str,
    /// A falsy, non-sequence value (`None`, `0`, ...): `while args` never runs
    /// and the original object is handed back untouched.
    Opaque,
}

/// The argument vector, flattened so scanning is index based.
struct Args {
    orig: Value,
    items: Vec<Value>,
    kind: SeqKind,
}

impl Args {
    fn load(interp: &mut Interpreter, value: &Value) -> Result<Args> {
        let (items, kind) = if let Some(s) = value.as_str() {
            (
                s.chars().map(|c| Value::string(c.to_string())).collect(),
                SeqKind::Str,
            )
        } else {
            match value.kind() {
                ValueKind::List(l) => (l.clone(), SeqKind::List),
                ValueKind::Tuple(t) => (t.as_ref().to_vec(), SeqKind::Tuple),
                _ => {
                    if !interp.truthy_value(value)? {
                        (Vec::new(), SeqKind::Opaque)
                    } else if interp.get_attr(value, "__getitem__").is_ok() {
                        (interp.collect_iterable(value)?, SeqKind::List)
                    } else {
                        return Err(type_error(format!(
                            "'{}' object is not subscriptable",
                            value_type_name_str(value)
                        )));
                    }
                }
            }
        };
        Ok(Args {
            orig: value.clone(),
            items,
            kind,
        })
    }

    /// `args[from:]`; the original object is returned when nothing was consumed.
    fn rest(&self, from: usize) -> Value {
        if from == 0 {
            return self.orig.clone();
        }
        let tail = &self.items[from.min(self.items.len())..];
        match self.kind {
            SeqKind::Tuple => Value::tuple(tail.to_vec()),
            SeqKind::Str => Value::string(
                tail.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .concat(),
            ),
            SeqKind::List | SeqKind::Opaque => Value::list(tail.to_vec()),
        }
    }
}

/// `shortopts` as the scanner sees it: the characters of a `str`, or the type
/// of whatever else was passed (CPython fails in `len(shortopts)`).
enum ShortOpts {
    Text(Vec<char>),
    Bad(String),
}

impl ShortOpts {
    fn new(value: &Value) -> ShortOpts {
        match value.as_str() {
            Some(s) => ShortOpts::Text(s.chars().collect()),
            None => ShortOpts::Bad(value_type_name_str(value).into_owned()),
        }
    }
}

/// `longopts` normalised like CPython: a lone `str` becomes `[str]`, anything
/// else is materialised with `list(longopts)`.
fn normalise_longopts(interp: &mut Interpreter, longopts: &OptValueRef<'_>) -> Result<Vec<Value>> {
    match longopts {
        OptionalArg::Omitted => Ok(Vec::new()),
        OptionalArg::Provided(v) => {
            if v.0.is_str() {
                Ok(vec![v.0.clone()])
            } else {
                interp.collect_iterable(v.0)
            }
        }
    }
}

/// `value.startswith(prefix)` for a non-`str` operand: surfaces CPython's own
/// `AttributeError` / `TypeError` for the operand's type.
fn non_str_startswith(interp: &mut Interpreter, value: &Value, prefix: &str) -> Result<bool> {
    let method = interp.get_attr(value, "startswith")?;
    let result = call_pos(interp, method, &[Value::string(prefix)])?;
    interp.truthy_value(&result)
}

fn opt_pair(name: String, optarg: Value) -> Value {
    Value::tuple(vec![Value::string(name), optarg])
}

fn short_has_arg_impl(interp: &mut Interpreter, opt: char, shortopts: &ShortOpts) -> Result<bool> {
    let chars = match shortopts {
        ShortOpts::Text(chars) => chars,
        ShortOpts::Bad(type_name) => {
            return Err(type_error(format!(
                "object of type '{type_name}' has no len()"
            )));
        }
    };
    for (i, &c) in chars.iter().enumerate() {
        if opt == c && c != ':' {
            return Ok(chars.get(i + 1) == Some(&':'));
        }
    }
    Err(getopt_error(
        interp,
        format!("option -{opt} not recognized"),
        &opt.to_string(),
    ))
}

/// `(has_arg, full option name)` for a possibly abbreviated long option.
fn long_has_args_impl(
    interp: &mut Interpreter,
    opt: &str,
    longopts: &[Value],
) -> Result<(bool, String)> {
    let mut possibilities: Vec<&str> = Vec::new();
    for candidate in longopts {
        match candidate.as_str() {
            Some(s) => {
                if s.starts_with(opt) {
                    possibilities.push(s);
                }
            }
            None => {
                non_str_startswith(interp, candidate, opt)?;
            }
        }
    }
    if possibilities.is_empty() {
        return Err(getopt_error(
            interp,
            format!("option --{opt} not recognized"),
            opt,
        ));
    }
    // Is there an exact match?
    if possibilities.contains(&opt) {
        return Ok((false, opt.to_string()));
    }
    let with_equals = format!("{opt}=");
    if possibilities.contains(&with_equals.as_str()) {
        return Ok((true, opt.to_string()));
    }
    // No exact match, so better be unique.
    if possibilities.len() > 1 {
        return Err(getopt_error(
            interp,
            format!("option --{opt} not a unique prefix"),
            opt,
        ));
    }
    let unique = possibilities[0];
    match unique.strip_suffix('=') {
        Some(name) => Ok((true, name.to_string())),
        None => Ok((false, unique.to_string())),
    }
}

/// Parse one `--name[=value]` argument; `pos` is the index of the argument
/// that follows it and is advanced past a consumed option value.
fn do_longs_impl(
    interp: &mut Interpreter,
    opts: &mut Vec<Value>,
    opt: &str,
    longopts: &[Value],
    args: &Args,
    pos: &mut usize,
) -> Result<()> {
    let (opt, mut optarg) = match opt.find('=') {
        Some(i) => (&opt[..i], Some(Value::string(&opt[i + 1..]))),
        None => (opt, None),
    };
    let (has_arg, opt) = long_has_args_impl(interp, opt, longopts)?;
    if has_arg {
        if optarg.is_none() {
            let Some(next) = args.items.get(*pos) else {
                return Err(getopt_error(
                    interp,
                    format!("option --{opt} requires argument"),
                    &opt,
                ));
            };
            optarg = Some(next.clone());
            *pos += 1;
        }
    } else if optarg.is_some() {
        return Err(getopt_error(
            interp,
            format!("option --{opt} must not have an argument"),
            &opt,
        ));
    }
    let value = match optarg {
        Some(v) if interp.truthy_value(&v)? => v,
        _ => Value::string(""),
    };
    opts.push(opt_pair(format!("--{opt}"), value));
    Ok(())
}

/// Parse one `-abc` bundle; `pos` is the index of the argument that follows it
/// and is advanced past a consumed option value.
fn do_shorts_impl(
    interp: &mut Interpreter,
    opts: &mut Vec<Value>,
    optstring: &str,
    shortopts: &ShortOpts,
    args: &Args,
    pos: &mut usize,
) -> Result<()> {
    let mut rest = optstring;
    while let Some(opt) = rest.chars().next() {
        rest = &rest[opt.len_utf8()..];
        let optarg = if short_has_arg_impl(interp, opt, shortopts)? {
            let value = if rest.is_empty() {
                let Some(next) = args.items.get(*pos) else {
                    return Err(getopt_error(
                        interp,
                        format!("option -{opt} requires argument"),
                        &opt.to_string(),
                    ));
                };
                *pos += 1;
                next.clone()
            } else {
                Value::string(rest)
            };
            rest = "";
            value
        } else {
            Value::string("")
        };
        opts.push(opt_pair(format!("-{opt}"), optarg));
    }
    Ok(())
}

/// Append `pending` to the caller-supplied `opts` object (`opts.append(...)`).
fn flush_opts(interp: &mut Interpreter, target: &Value, pending: Vec<Value>) -> Result<()> {
    if matches!(target.kind(), ValueKind::List(_)) {
        for item in pending {
            target.list_push(item)?;
        }
        return Ok(());
    }
    for item in pending {
        let append = interp.get_attr(target, "append")?;
        call_pos(interp, append, &[item])?;
    }
    Ok(())
}

/// `os.environ.get("POSIXLY_CORRECT")` truthiness.
fn posixly_correct(interp: &mut Interpreter) -> Result<bool> {
    let os = interp.load_module("os")?;
    let environ = interp.get_attr(&os, "environ")?;
    let get = interp.get_attr(&environ, "get")?;
    let value = call_pos(interp, get, &[Value::string("POSIXLY_CORRECT")])?;
    interp.truthy_value(&value)
}

/// Wire `error = GetoptError` and the `os` module attribute (`import os` in
/// CPython's `getopt.py`).
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    if let Ok(os) = interp.load_module("os") {
        module_rc.borrow_mut().insert_attr("os".to_string(), os);
    }
    let class = module_rc.borrow().attrs.get("GetoptError").cloned();
    if let Some(class) = class {
        module_rc
            .borrow_mut()
            .insert_attr("error".to_string(), class);
    }
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(
            "Parser for command line options.\n\n\
             This module helps scripts to parse the command line arguments in\n\
             sys.argv.  It supports the same conventions as the Unix getopt()\n\
             function (including the special meanings of arguments of the form `-'\n\
             and `--').  Long options similar to those supported by GNU software\n\
             may be used as well via an optional third argument.  This module\n\
             provides two functions and an exception:\n\n\
             getopt() -- Parse command line options\n\
             gnu_getopt() -- Like getopt(), but allow option and non-option arguments\n\
             to be intermixed.\n\
             GetoptError -- exception (class) raised with 'opt' attribute, which is the\n\
             option involved with the exception.\n"
        ),
        "__all__" => str_list(&["GetoptError", "error", "getopt", "gnu_getopt"]),
    }

    #[py_name = "_"]
    fn gettext_passthrough(s: PyValue) -> Result<Value> {
        Ok(s.0)
    }

    class GetoptError("Exception") {
        constants {
            "opt" => Value::string(""),
            "msg" => Value::string(""),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
            #[default(PyValue(Value::string("")))] opt: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "msg", msg.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "opt", opt.0.clone())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "args",
                Value::tuple(vec![msg.0, opt.0]),
            )?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "msg")
        }
    }

    fn getopt(
        args: PyValueRef<'_>,
        shortopts: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] longopts: OptValueRef<'_>,
    ) -> Result<Value> {
        let longs = normalise_longopts(_interp, &longopts)?;
        let args = Args::load(_interp, args.0)?;
        let short = ShortOpts::new(shortopts.0);
        let mut opts: Vec<Value> = Vec::new();
        let mut i = 0;
        while let Some(current) = args.items.get(i) {
            let Some(text) = current.as_str() else {
                // `args[0].startswith("-")` on a non-str operand.
                non_str_startswith(_interp, current, "-")?;
                break;
            };
            if !text.starts_with('-') || text == "-" {
                break;
            }
            if text == "--" {
                i += 1;
                break;
            }
            let mut next = i + 1;
            if let Some(name) = text.strip_prefix("--") {
                do_longs_impl(_interp, &mut opts, name, &longs, &args, &mut next)?;
            } else {
                do_shorts_impl(_interp, &mut opts, &text[1..], &short, &args, &mut next)?;
            }
            i = next;
        }
        Ok(Value::tuple(vec![Value::list(opts), args.rest(i)]))
    }

    fn gnu_getopt(
        args: PyValueRef<'_>,
        shortopts: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] longopts: OptValueRef<'_>,
    ) -> Result<Value> {
        let longs = normalise_longopts(_interp, &longopts)?;
        // Allow options after non-option arguments?
        let (short, all_options_first) = if shortopts.0.is_str() {
            let text = shortopts.0.as_str().unwrap_or("");
            match text.strip_prefix('+') {
                Some(stripped) => (ShortOpts::Text(stripped.chars().collect()), true),
                None => (ShortOpts::new(shortopts.0), posixly_correct(_interp)?),
            }
        } else {
            // `shortopts.startswith("+")` on a non-str operand.
            non_str_startswith(_interp, shortopts.0, "+")?;
            (ShortOpts::new(shortopts.0), posixly_correct(_interp)?)
        };
        let args = Args::load(_interp, args.0)?;
        let mut opts: Vec<Value> = Vec::new();
        let mut prog_args: Vec<Value> = Vec::new();
        let mut i = 0;
        while let Some(current) = args.items.get(i) {
            let Some(text) = current.as_str() else {
                // Sequences slice without error and never compare equal to a
                // `str`; any other operand cannot be subscripted.
                let type_name = value_type_name_str(current);
                if !matches!(type_name.as_ref(), "list" | "tuple" | "bytes" | "bytearray") {
                    return Err(type_error(format!(
                        "'{type_name}' object is not subscriptable"
                    )));
                }
                if all_options_first {
                    prog_args.extend_from_slice(&args.items[i..]);
                    break;
                }
                prog_args.push(current.clone());
                i += 1;
                continue;
            };
            if text == "--" {
                prog_args.extend_from_slice(&args.items[i + 1..]);
                break;
            }
            let mut next = i + 1;
            if let Some(name) = text.strip_prefix("--") {
                do_longs_impl(_interp, &mut opts, name, &longs, &args, &mut next)?;
                i = next;
            } else if text.starts_with('-') && text != "-" {
                do_shorts_impl(_interp, &mut opts, &text[1..], &short, &args, &mut next)?;
                i = next;
            } else if all_options_first {
                prog_args.extend_from_slice(&args.items[i..]);
                break;
            } else {
                prog_args.push(current.clone());
                i += 1;
            }
        }
        Ok(Value::tuple(vec![Value::list(opts), Value::list(prog_args)]))
    }

    fn do_longs(
        opts: PyValueRef<'_>,
        opt: PyStr<'_>,
        longopts: PyValueRef<'_>,
        args: PyValueRef<'_>,
    ) -> Result<Value> {
        let longs = _interp.collect_iterable(longopts.0)?;
        let args = Args::load(_interp, args.0)?;
        let mut pending = Vec::new();
        let mut pos = 0;
        let outcome = do_longs_impl(_interp, &mut pending, &opt, &longs, &args, &mut pos);
        flush_opts(_interp, opts.0, pending)?;
        outcome?;
        Ok(Value::tuple(vec![opts.0.clone(), args.rest(pos)]))
    }

    fn long_has_args(opt: PyStr<'_>, longopts: PyValueRef<'_>) -> Result<Value> {
        let longs = _interp.collect_iterable(longopts.0)?;
        let (has_arg, name) = long_has_args_impl(_interp, &opt, &longs)?;
        Ok(Value::tuple(vec![Value::bool_(has_arg), Value::string(name)]))
    }

    fn do_shorts(
        opts: PyValueRef<'_>,
        optstring: PyStr<'_>,
        shortopts: PyValueRef<'_>,
        args: PyValueRef<'_>,
    ) -> Result<Value> {
        let short = ShortOpts::new(shortopts.0);
        let args = Args::load(_interp, args.0)?;
        let mut pending = Vec::new();
        let mut pos = 0;
        let outcome = do_shorts_impl(_interp, &mut pending, &optstring, &short, &args, &mut pos);
        flush_opts(_interp, opts.0, pending)?;
        outcome?;
        Ok(Value::tuple(vec![opts.0.clone(), args.rest(pos)]))
    }

    fn short_has_arg(opt: PyStr<'_>, shortopts: PyValueRef<'_>) -> Result<Value> {
        let short = ShortOpts::new(shortopts.0);
        let mut chars = opt.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => Ok(Value::bool_(short_has_arg_impl(_interp, c, &short)?)),
            _ => Err(getopt_error(
                _interp,
                format!("option -{} not recognized", &*opt),
                &opt,
            )),
        }
    }
}
