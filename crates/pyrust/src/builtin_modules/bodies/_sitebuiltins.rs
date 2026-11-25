// `_sitebuiltins` module — 100% native-Rust CPython 3.12 implementation of
// `Quitter`, `_Printer`, and `_Helper` using PyRust's native module macros.
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/_sitebuiltins.py>

use std::path::Path;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, lookup_exc_class, render_instance_str};
use crate::value::Value;

struct QuitterState {
    name: Value,
    eof: Value,
}

struct PrinterState {
    name: String,
    data: String,
    filenames: Vec<String>,
    lines: Option<Vec<String>>,
}

impl PrinterState {
    fn setup(&mut self) {
        if self.lines.is_some() {
            return;
        }
        let mut loaded: Option<String> = None;
        for filename in &self.filenames {
            if let Ok(text) = std::fs::read_to_string(filename) {
                loaded = Some(text);
                break;
            }
        }
        let text = match loaded {
            Some(ref s) if !s.is_empty() => s.as_str(),
            _ => self.data.as_str(),
        };
        self.lines = Some(text.split('\n').map(str::to_owned).collect());
    }
}

fn printer_maxlines(interp: &mut Interpreter, self_val: &Value) -> usize {
    interp
        .get_attr(self_val, "MAXLINES")
        .ok()
        .and_then(|v| v.as_int())
        .filter(|&n| n > 0)
        .map(|n| n as usize)
        .unwrap_or(23)
}

fn call_builtin_print(interp: &mut Interpreter, text: &str) -> Result<()> {
    let builtins = interp.load_module("builtins")?;
    let print_fn = interp.get_attr(&builtins, "print")?;
    interp.call_function_expanded(
        print_fn,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(text),
        }],
    )?;
    Ok(())
}

fn call_builtin_input(interp: &mut Interpreter, prompt: &str) -> Result<String> {
    let builtins = interp.load_module("builtins")?;
    let input_fn = interp.get_attr(&builtins, "input")?;
    let res = interp.call_function_expanded(
        input_fn,
        &[ExpandedCallArg {
            name: None,
            value: Value::string(prompt),
        }],
    )?;
    render_instance_str(interp, &res)
}

pyrust_module! {
    #[native_state(QuitterState)]
    class Quitter {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            eof: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                QuitterState {
                    name: name.0,
                    eof: eof.0,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn name(#[state] st: &QuitterState) -> Result<Value> {
            Ok(st.name.clone())
        }

        #[setter("name")]
        fn set_name(#[state_mut] st: &mut QuitterState, value: PyValue) -> Result<Value> {
            st.name = value.0;
            Ok(Value::none())
        }

        #[getter]
        fn eof(#[state] st: &QuitterState) -> Result<Value> {
            Ok(st.eof.clone())
        }

        #[setter("eof")]
        fn set_eof(#[state_mut] st: &mut QuitterState, value: PyValue) -> Result<Value> {
            st.eof = value.0;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (name_val, eof_val) =
                with_state(self_val.0, |st| Ok((st.name.clone(), st.eof.clone())))?;
            let name_s = render_instance_str(_interp, &name_val)?;
            let eof_s = render_instance_str(_interp, &eof_val)?;
            Ok(Value::string(format!("Use {name_s}() or {eof_s} to exit")))
        }

        fn __call__(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(PyValue(Value::none()))] code: PyValue,
        ) -> Result<Value> {
            // Mirror CPython: attempt sys.stdin.close() and ignore any error.
            if let Ok(sys_mod) = _interp.load_module("sys")
                && let Ok(stdin) = _interp.get_attr(&sys_mod, "stdin")
                && let Ok(close_fn) = _interp.get_attr(&stdin, "close")
            {
                let _ = _interp.call_function_expanded(close_fn, &[]);
            }
            let cls = lookup_exc_class("SystemExit").ok_or_else(|| {
                PyError::runtime("built-in exception 'SystemExit' is not defined".to_string())
            })?;
            let exc = _interp.call_function_expanded(
                Value::py_class(cls),
                &[ExpandedCallArg {
                    name: None,
                    value: code.0,
                }],
            )?;
            Err(PyError::raised(exc))
        }
    }

    #[native_state(PrinterState)]
    class _Printer {
        constants {
            "MAXLINES" => Value::int(23),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            data: PyValue,
            #[default(OptionalArg::Omitted)] files: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] dirs: OptValueRef<'_>,
        ) -> Result<Value> {
            let name_s = render_instance_str(_interp, &name.0)?;
            let data_s = render_instance_str(_interp, &data.0)?;
            let file_vals = match files.as_value() {
                Some(v) => _interp.collect_iterable(v)?,
                None => Vec::new(),
            };
            let dir_vals = match dirs.as_value() {
                Some(v) => _interp.collect_iterable(v)?,
                None => Vec::new(),
            };
            let mut filenames = Vec::with_capacity(dir_vals.len() * file_vals.len());
            for dir_v in &dir_vals {
                let dir_s = render_instance_str(_interp, dir_v)?;
                for file_v in &file_vals {
                    let file_s = render_instance_str(_interp, file_v)?;
                    let joined = Path::new(&dir_s).join(&file_s);
                    filenames.push(joined.to_string_lossy().into_owned());
                }
            }
            set_state(
                self_val.0,
                PrinterState {
                    name: name_s,
                    data: data_s,
                    filenames,
                    lines: None,
                },
            )?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let maxlines = printer_maxlines(_interp, self_val.0);
            with_state_mut(self_val.0, |st| {
                st.setup();
                let lines = st.lines.as_deref().unwrap_or(&[]);
                if lines.len() <= maxlines {
                    Ok(Value::string(lines.join("\n")))
                } else {
                    Ok(Value::string(format!(
                        "Type {}() to see the full {} text",
                        st.name, st.name
                    )))
                }
            })
        }

        fn __call__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let maxlines = printer_maxlines(_interp, self_val.0);
            let lines: Vec<String> = with_state_mut(self_val.0, |st| {
                st.setup();
                Ok(st.lines.clone().unwrap_or_default())
            })?;
            let prompt = "Hit Return for more, or q (and Return) to quit: ";
            let mut lineno = 0usize;
            loop {
                let end = lineno + maxlines;
                if end > lines.len() {
                    for line in &lines[lineno..] {
                        call_builtin_print(_interp, line)?;
                    }
                    break;
                }
                for line in &lines[lineno..end] {
                    call_builtin_print(_interp, line)?;
                }
                lineno = end;
                loop {
                    let key = call_builtin_input(_interp, prompt)?;
                    if key.is_empty() {
                        break;
                    }
                    if key == "q" {
                        return Ok(Value::none());
                    }
                }
            }
            Ok(Value::none())
        }
    }

    class _Helper {
        fn __repr__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::string(
                "Type help() for interactive help, or help(object) for help about object.",
            ))
        }

        fn __call__(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwds: IndexMap<String, Value>,
        ) -> Result<Value> {
            let pydoc = _interp.load_module("pydoc")?;
            let help_fn = _interp.get_attr(&pydoc, "help")?;
            let mut call_args = Vec::with_capacity(args.len() + kwds.len());
            for a in args {
                call_args.push(ExpandedCallArg {
                    name: None,
                    value: a.clone(),
                });
            }
            for (k, v) in kwds {
                call_args.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            _interp.call_function_expanded(help_fn, &call_args)
        }
    }
}
