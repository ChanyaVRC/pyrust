// `ensurepip._uninstall` submodule — 100% native Rust implementation of
// Python 3.12's `ensurepip._uninstall` helper (issue #3267).
//
// Reference: <https://docs.python.org/3.12/library/ensurepip.html>

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter, render_instance_str};
use crate::value::Value;

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded = Vec::with_capacity(pos.len() + kw.len());
    for v in pos {
        expanded.push(ExpandedCallArg {
            name: None,
            value: v.clone(),
        });
    }
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_kw(interp, m, args, &[])
}

fn call_method_kw(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_kw(interp, m, pos, kw)
}

pyrust_module! {
    constants {
        "__package__" => Value::string("ensurepip"),
    }

    fn _main(#[default(PyValue(Value::none()))] argv: PyValue) -> Result<Value> {
        let ep_mod = _interp.load_module("ensurepip")?;
        let ver_val = call_method(_interp, &ep_mod, "version", &[])?;
        let ver = render_instance_str(_interp, &ver_val)?;

        let argparse_mod = _interp.load_module("argparse")?;
        let parser_cls = _interp.get_attr(&argparse_mod, "ArgumentParser")?;
        let parser = call_kw(
            _interp,
            parser_cls,
            &[],
            &[("prog", Value::string("python -m ensurepip._uninstall"))],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("--version")],
            &[
                ("action", Value::string("version")),
                ("version", Value::string(format!("pip {ver}"))),
                (
                    "help",
                    Value::string("Show the version of pip this will attempt to uninstall."),
                ),
            ],
        )?;
        call_method_kw(
            _interp,
            &parser,
            "add_argument",
            &[Value::string("-v"), Value::string("--verbose")],
            &[
                ("action", Value::string("count")),
                ("default", Value::int(0)),
                ("dest", Value::string("verbosity")),
                (
                    "help",
                    Value::string("Give more output. Option is additive, and can be used up to 3 times."),
                ),
            ],
        )?;

        let parsed = call_method(_interp, &parser, "parse_args", &[argv.0])?;
        let verbosity = _interp
            .get_attr(&parsed, "verbosity")?
            .as_int()
            .unwrap_or(0);
        call_method_kw(
            _interp,
            &ep_mod,
            "_uninstall_helper",
            &[],
            &[("verbosity", Value::int(verbosity))],
        )
    }
}
