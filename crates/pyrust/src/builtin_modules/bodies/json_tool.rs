// `json.tool` submodule — 100% native Rust implementation of Python 3.12's
// `json.tool` CLI utility (issue #3251).
//
// Supports `pyrust -m json.tool` and direct `json.tool.main()` with
// `[infile [outfile]]`, `--sort-keys`, `--no-ensure-ascii`, `--json-lines`,
// `--indent`, `--tab`, `--no-indent`, and `--compact`.
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/json/tool.py>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_method_kw(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_kw(interp, f, pos, kw)
}

fn raise_system_exit(interp: &mut Interpreter, arg: Value) -> PyError {
    if let Some(cls) = crate::interpreter::lookup_exc_class("SystemExit")
        && let Ok(inst) = call_pos(interp, Value::py_class(cls), std::slice::from_ref(&arg))
    {
        return PyError::raised(inst);
    }
    let msg = interp.render_value_as_str(&arg).unwrap_or_default();
    PyError::named("SystemExit", msg)
}

fn run_json_tool_main(interp: &mut Interpreter) -> Result<Value> {
    let argparse_mod = interp.load_module("argparse")?;
    let json_mod = interp.load_module("json")?;
    let sys_mod = interp.load_module("sys")?;

    let parser_cls = interp.get_attr(&argparse_mod, "ArgumentParser")?;
    let description = concat!(
        "A simple command line interface for json module ",
        "to validate and pretty-print JSON objects."
    );
    let parser = call_kw(
        interp,
        parser_cls,
        &[],
        &[
            ("prog", Value::string("python -m json.tool")),
            ("description", Value::string(description)),
        ],
    )?;

    let file_type_cls = interp.get_attr(&argparse_mod, "FileType")?;
    let infile_type = call_kw(
        interp,
        file_type_cls.clone(),
        &[Value::string("r")],
        &[("encoding", Value::string("utf-8"))],
    )?;
    let outfile_type = call_kw(
        interp,
        file_type_cls,
        &[Value::string("w")],
        &[("encoding", Value::string("utf-8"))],
    )?;
    let stdin = interp.get_attr(&sys_mod, "stdin")?;
    let stdout = interp.get_attr(&sys_mod, "stdout")?;

    call_method_kw(
        interp,
        &parser,
        "add_argument",
        &[Value::string("infile")],
        &[
            ("nargs", Value::string("?")),
            ("type", infile_type),
            (
                "help",
                Value::string("a JSON file to be validated or pretty-printed"),
            ),
            ("default", stdin),
        ],
    )?;
    call_method_kw(
        interp,
        &parser,
        "add_argument",
        &[Value::string("outfile")],
        &[
            ("nargs", Value::string("?")),
            ("type", outfile_type),
            (
                "help",
                Value::string("write the output of infile to outfile"),
            ),
            ("default", stdout),
        ],
    )?;
    call_method_kw(
        interp,
        &parser,
        "add_argument",
        &[Value::string("--sort-keys")],
        &[
            ("action", Value::string("store_true")),
            ("default", Value::bool_(false)),
            (
                "help",
                Value::string("sort the output of dictionaries alphabetically by key"),
            ),
        ],
    )?;
    call_method_kw(
        interp,
        &parser,
        "add_argument",
        &[Value::string("--no-ensure-ascii")],
        &[
            ("dest", Value::string("ensure_ascii")),
            ("action", Value::string("store_false")),
            (
                "help",
                Value::string("disable escaping of non-ASCII characters"),
            ),
        ],
    )?;
    call_method_kw(
        interp,
        &parser,
        "add_argument",
        &[Value::string("--json-lines")],
        &[
            ("action", Value::string("store_true")),
            ("default", Value::bool_(false)),
            (
                "help",
                Value::string(
                    "parse input using the JSON Lines format. Use with --no-indent or --compact to produce valid JSON Lines output.",
                ),
            ),
        ],
    )?;

    let group = call_method_kw(interp, &parser, "add_mutually_exclusive_group", &[], &[])?;
    let int_cls = crate::interpreter::primitive_class_by_name("int")
        .map(Value::py_class)
        .unwrap_or_else(|| Value::builtin_function("int"));
    call_method_kw(
        interp,
        &group,
        "add_argument",
        &[Value::string("--indent")],
        &[
            ("default", Value::int(4)),
            ("type", int_cls),
            (
                "help",
                Value::string(
                    "separate items with newlines and use this number of spaces for indentation",
                ),
            ),
        ],
    )?;
    call_method_kw(
        interp,
        &group,
        "add_argument",
        &[Value::string("--tab")],
        &[
            ("action", Value::string("store_const")),
            ("dest", Value::string("indent")),
            ("const", Value::string("\t")),
            (
                "help",
                Value::string("separate items with newlines and use tabs for indentation"),
            ),
        ],
    )?;
    call_method_kw(
        interp,
        &group,
        "add_argument",
        &[Value::string("--no-indent")],
        &[
            ("action", Value::string("store_const")),
            ("dest", Value::string("indent")),
            ("const", Value::none()),
            (
                "help",
                Value::string("separate items with spaces rather than newlines"),
            ),
        ],
    )?;
    call_method_kw(
        interp,
        &group,
        "add_argument",
        &[Value::string("--compact")],
        &[
            ("action", Value::string("store_true")),
            (
                "help",
                Value::string("suppress all whitespace separation (most compact)"),
            ),
        ],
    )?;

    let options = call_method_kw(interp, &parser, "parse_args", &[], &[])?;
    let infile = interp.get_attr(&options, "infile")?;
    let outfile = interp.get_attr(&options, "outfile")?;
    let sort_keys = interp.get_attr(&options, "sort_keys")?;
    let ensure_ascii = interp.get_attr(&options, "ensure_ascii")?;
    let json_lines = interp.get_attr(&options, "json_lines")?;
    let compact = interp.get_attr(&options, "compact")?;
    let indent_opt = interp.get_attr(&options, "indent")?;

    let is_compact = interp.truthy_value(&compact)?;
    let is_json_lines = interp.truthy_value(&json_lines)?;

    let mut dump_kw: Vec<(&str, Value)> =
        vec![("sort_keys", sort_keys), ("ensure_ascii", ensure_ascii)];
    if is_compact {
        dump_kw.push(("indent", Value::none()));
        dump_kw.push((
            "separators",
            Value::tuple(vec![Value::string(","), Value::string(":")]),
        ));
    } else {
        dump_kw.push(("indent", indent_opt));
    }

    let load_fn = interp.get_attr(&json_mod, "load")?;
    let loads_fn = interp.get_attr(&json_mod, "loads")?;
    let dump_fn = interp.get_attr(&json_mod, "dump")?;

    // Enter context managers when supported (e.g. opened files); built-in
    // sys.stdin / sys.stdout handles may not expose `__enter__` / `__exit__`.
    let infile_is_cm = interp.get_attr(&infile, "__enter__").is_ok();
    let infile_entered = if infile_is_cm {
        call_method_kw(interp, &infile, "__enter__", &[], &[])?
    } else {
        infile.clone()
    };
    let res = (|| -> Result<()> {
        let objs: Vec<Value> = if is_json_lines {
            let lines: Vec<Value> = match interp.collect_iterable(&infile_entered) {
                Ok(v) => v,
                Err(_) => {
                    let content = call_method_kw(interp, &infile_entered, "read", &[], &[])?;
                    let text = content.as_str().unwrap_or("");
                    text.lines().map(Value::string).collect()
                }
            };
            let mut parsed = Vec::with_capacity(lines.len());
            for line in lines {
                match call_pos(interp, loads_fn.clone(), &[line]) {
                    Ok(obj) => parsed.push(obj),
                    Err(e) => {
                        if e.class_name_is("ValueError") {
                            let exc = interp.materialize_pyerror(e)?;
                            return Err(raise_system_exit(interp, exc));
                        }
                        return Err(e);
                    }
                }
            }
            parsed
        } else {
            match call_pos(interp, load_fn, std::slice::from_ref(&infile_entered)) {
                Ok(obj) => vec![obj],
                Err(e) => {
                    if e.class_name_is("ValueError") {
                        let exc = interp.materialize_pyerror(e)?;
                        return Err(raise_system_exit(interp, exc));
                    }
                    return Err(e);
                }
            }
        };

        let outfile_is_cm = interp.get_attr(&outfile, "__enter__").is_ok();
        let outfile_entered = if outfile_is_cm {
            call_method_kw(interp, &outfile, "__enter__", &[], &[])?
        } else {
            outfile.clone()
        };
        let out_res = (|| -> Result<()> {
            for obj in objs {
                call_kw(
                    interp,
                    dump_fn.clone(),
                    &[obj, outfile_entered.clone()],
                    &dump_kw,
                )?;
                call_method_kw(
                    interp,
                    &outfile_entered,
                    "write",
                    &[Value::string("\n")],
                    &[],
                )?;
            }
            Ok(())
        })();
        if outfile_is_cm {
            let _ = call_method_kw(
                interp,
                &outfile,
                "__exit__",
                &[Value::none(), Value::none(), Value::none()],
                &[],
            );
        }
        out_res
    })();
    if infile_is_cm {
        let _ = call_method_kw(
            interp,
            &infile,
            "__exit__",
            &[Value::none(), Value::none(), Value::none()],
            &[],
        );
    }

    if let Err(e) = res {
        if e.class_name_is("BrokenPipeError") {
            let exc = interp.materialize_pyerror(e)?;
            let errno_val = interp
                .get_attr(&exc, "errno")
                .unwrap_or_else(|_| Value::int(1));
            return Err(raise_system_exit(interp, errno_val));
        }
        return Err(e);
    }

    Ok(Value::none())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("json"),
    }

    fn main() -> Result<Value> {
        run_json_tool_main(_interp)
    }
}
