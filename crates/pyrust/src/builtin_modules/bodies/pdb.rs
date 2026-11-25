// `pdb` module — 100% native Rust implementation of Python 3.12's `pdb`
// (The Python Debugger) module (issue #3247).
//
// `Pdb` inherits from `(bdb.Bdb, cmd.Cmd)` (wired via `prepare_module` using
// `link_class_bases`), supporting full scripted non-interactive `stdin`/`stdout`
// `io.StringIO` execution as well as interactive debugging.
//
// Reference: <https://docs.python.org/3/library/pdb.html>

use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue, PyValueRef};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, render_value_repr, type_class_singleton, value_class,
    values_are_identical,
};
use crate::value::{PyDict, Value, ValueKind};

const CO_VARARGS: i64 = 0x04;
const CO_VARKEYWORDS: i64 = 0x08;
const DEFAULT_LINE_PREFIX: &str = "\n-> ";

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

fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_pos(interp, f, args)
}

fn pdb_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("pdb")?;
    interp.get_attr(&m, name)
}

fn bdb_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("bdb")?;
    interp.get_attr(&m, name)
}

fn format_exc_only(interp: &mut Interpreter, exc: &Value) -> String {
    if let Ok(tb_mod) = interp.load_module("traceback")
        && let Ok(fmt_fn) = interp.get_attr(&tb_mod, "format_exception_only")
        && let Ok(lines_val) = call_pos(interp, fmt_fn, std::slice::from_ref(exc))
        && let Ok(lines) = interp.collect_iterable(&lines_val)
        && let Some(last) = lines.last()
        && let Ok(s) = interp.render_value_as_str(last)
    {
        return s.trim().to_string();
    }
    let cls = value_class(exc);
    let name = interp
        .get_attr(&cls, "__name__")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "Exception".to_string());
    let msg = interp.render_value_as_str(exc).unwrap_or_default();
    if msg.is_empty() {
        name
    } else {
        format!("{name}: {msg}")
    }
}

fn format_pyerror(interp: &mut Interpreter, err: &PyError) -> String {
    match interp.materialize_pyerror(err.clone()) {
        Ok(exc) => format_exc_only(interp, &exc),
        Err(_) => format!("{err}"),
    }
}

fn is_exc_type(interp: &mut Interpreter, err: &PyError, exc_name: &str) -> bool {
    let Ok(exc) = interp.materialize_pyerror(err.clone()) else {
        return false;
    };
    let Some(cls) = crate::interpreter::lookup_exc_class(exc_name) else {
        return false;
    };
    call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[exc, Value::py_class(cls)],
    )
    .and_then(|v| interp.truthy_value(&v))
    .unwrap_or(false)
}

fn pdb_message(interp: &mut Interpreter, self_val: &Value, msg: &str) -> Result<()> {
    call_method(interp, self_val, "message", &[Value::string(msg)])?;
    Ok(())
}

fn pdb_error(interp: &mut Interpreter, self_val: &Value, msg: &str) -> Result<()> {
    call_method(interp, self_val, "error", &[Value::string(msg)])?;
    Ok(())
}

fn safe_repr(interp: &mut Interpreter, obj: &Value, expr: &str) -> String {
    match render_value_repr(interp, obj) {
        Ok(s) => s,
        Err(e) => format!(
            "*** repr({expr}) failed: {} ***",
            format_pyerror(interp, &e)
        ),
    }
}

fn compile_error_message(interp: &mut Interpreter, expr: &str) -> Option<String> {
    match call_pos(
        interp,
        Value::builtin_function("compile"),
        &[
            Value::string(expr),
            Value::string("<stdin>"),
            Value::string("eval"),
        ],
    ) {
        Ok(_) => None,
        Err(e) => Some(format_pyerror(interp, &e)),
    }
}

fn eval_in_curframe(interp: &mut Interpreter, self_val: &Value, expr: &str) -> Result<Value> {
    let curframe = interp.get_attr(self_val, "curframe")?;
    let f_globals = interp.get_attr(&curframe, "f_globals")?;
    let f_locals = interp.get_attr(self_val, "curframe_locals")?;
    match call_pos(
        interp,
        Value::builtin_function("eval"),
        &[Value::string(expr), f_globals, f_locals],
    ) {
        Ok(val) => Ok(val),
        Err(e) => {
            let msg = format_pyerror(interp, &e);
            let _ = pdb_error(interp, self_val, &msg);
            Err(e)
        }
    }
}

fn eval_in_frame_except(
    interp: &mut Interpreter,
    self_val: &Value,
    expr: &str,
    frame_opt: Option<&Value>,
) -> Value {
    let eval_res = (|| -> Result<Value> {
        let (f_globals, f_locals) = match frame_opt {
            Some(f) => (
                interp.get_attr(f, "f_globals")?,
                interp.get_attr(f, "f_locals")?,
            ),
            None => {
                let cf = interp.get_attr(self_val, "curframe")?;
                (
                    interp.get_attr(&cf, "f_globals")?,
                    interp.get_attr(self_val, "curframe_locals")?,
                )
            }
        };
        call_pos(
            interp,
            Value::builtin_function("eval"),
            &[Value::string(expr), f_globals, f_locals],
        )
    })();
    match eval_res {
        Ok(v) => v,
        Err(e) => Value::string(format!("** raised {} **", format_pyerror(interp, &e))),
    }
}

fn find_function_impl(
    interp: &mut Interpreter,
    funcname: &str,
    filename: &str,
) -> Result<Option<(String, String, i64)>> {
    let lc = interp.load_module("linecache")?;
    let getlines = interp.get_attr(&lc, "getlines")?;
    let lines_val = call_pos(interp, getlines, &[Value::string(filename)])?;
    let lines = interp.collect_iterable(&lines_val)?;
    if lines.is_empty() {
        return Ok(None);
    }
    let def_prefix = format!("def {funcname}");
    let async_prefix = format!("async def {funcname}");
    for (idx, line_val) in lines.iter().enumerate() {
        let line = line_val.as_str().unwrap_or("");
        let trimmed = line.trim_start();
        let matched = if let Some(rest) = trimmed.strip_prefix(&def_prefix) {
            rest.is_empty()
                || rest.starts_with('(')
                || rest.starts_with(' ')
                || rest.starts_with('\t')
                || rest.starts_with('[')
        } else if let Some(rest) = trimmed.strip_prefix(&async_prefix) {
            rest.is_empty()
                || rest.starts_with('(')
                || rest.starts_with(' ')
                || rest.starts_with('\t')
                || rest.starts_with('[')
        } else {
            false
        };
        if matched {
            return Ok(Some((
                funcname.to_string(),
                filename.to_string(),
                (idx + 1) as i64,
            )));
        }
    }
    Ok(None)
}

fn lasti2lineno_impl(interp: &mut Interpreter, code: &Value, lasti: i64) -> Result<i64> {
    let co_firstlineno = interp
        .get_attr(code, "co_firstlineno")
        .ok()
        .and_then(|v| v.as_int())
        .unwrap_or(0);
    if let Ok(co_lines_fn) = interp.get_attr(code, "co_lines")
        && let Ok(iter_val) = call_pos(interp, co_lines_fn, &[])
        && let Ok(entries) = interp.collect_iterable(&iter_val)
    {
        let mut last_line = co_firstlineno;
        for entry in entries {
            if let Some(t) = entry.as_tuple()
                && t.len() == 3
            {
                let start = t[0].as_int().unwrap_or(0);
                let end = t[1].as_int().unwrap_or(0);
                if let Some(ln) = t[2].as_int() {
                    last_line = ln;
                    if lasti >= start && lasti < end {
                        return Ok(ln);
                    }
                }
            }
        }
        return Ok(last_line);
    }
    Ok(co_firstlineno)
}

fn command_doc(cmd: &str) -> Option<&'static str> {
    match cmd {
        "h" | "help" => Some(
            "h(elp)\n\nWithout argument, print the list of available commands.\nWith a command name as argument, print help about that command.\n\"help pdb\" shows the full pdb documentation.\n\"help exec\" gives help on the ! command.",
        ),
        "w" | "where" | "bt" => Some(
            "w(here)\n\nPrint a stack trace, with the most recent frame at the bottom.\nAn arrow indicates the \"current frame\", which determines the\ncontext of most commands.  'bt' is an alias for this command.",
        ),
        "d" | "down" => Some(
            "d(own) [count]\n\nMove the current frame count (default one) levels down in the\nstack trace (to a newer frame).",
        ),
        "u" | "up" => Some(
            "u(p) [count]\n\nMove the current frame count (default one) levels up in the\nstack trace (to an older frame).",
        ),
        "b" | "break" => Some(
            "b(reak) [ ([filename:]lineno | function) [, condition] ]\n\nWithout argument, list all breaks.\nWith a line number argument, set a break at this line in the\ncurrent file.  With a function name, set a break at the first\nexecutable line of that function.",
        ),
        "tbreak" => Some(
            "tbreak [ ([filename:]lineno | function) [, condition] ]\n\nSame arguments as break, but sets a temporary breakpoint: it\nis automatically deleted when first hit.",
        ),
        "cl" | "clear" => Some(
            "cl(ear) [filename:lineno | bpnumber ...]\n\nWith a space separated list of breakpoint numbers, clear\nthose breakpoints.  Without argument, clear all breaks.",
        ),
        "disable" => Some(
            "disable bpnumber [bpnumber ...]\n\nDisables the breakpoints given as a space separated list of\nbreakpoint numbers.",
        ),
        "enable" => Some(
            "enable bpnumber [bpnumber ...]\n\nEnables the breakpoints given as a space separated list of\nbreakpoint numbers.",
        ),
        "ignore" => {
            Some("ignore bpnumber [count]\n\nSet the ignore count for the given breakpoint number.")
        }
        "condition" => {
            Some("condition bpnumber [condition]\n\nSet a new condition for the breakpoint.")
        }
        "s" | "step" => Some(
            "s(tep)\n\nExecute the current line, stop at the first possible occasion\n(either in a function that is called or in the current\nfunction).",
        ),
        "n" | "next" => Some(
            "n(ext)\n\nContinue execution until the next line in the current function\nis reached or it returns.",
        ),
        "unt" | "until" => Some(
            "unt(il) [lineno]\n\nWithout argument, continue execution until the line with a\nnumber greater than the current one is reached.",
        ),
        "r" | "return" => {
            Some("r(eturn)\n\nContinue execution until the current function returns.")
        }
        "c" | "cont" | "continue" => {
            Some("c(ont(inue))\n\nContinue execution, only stop when a breakpoint is encountered.")
        }
        "j" | "jump" => Some("j(ump) lineno\n\nSet the next line that will be executed."),
        "l" | "list" => {
            Some("l(ist) [first[, last] | .]\n\nList source code for the current file.")
        }
        "ll" | "longlist" => {
            Some("ll | longlist\n\nList the whole source code for the current function or frame.")
        }
        "a" | "args" => Some("a(rgs)\n\nPrint the argument list of the current function."),
        "rv" | "retval" => {
            Some("retval\n\nPrint the return value for the last return of a function.")
        }
        "p" => Some("p expression\n\nPrint the value of the expression."),
        "pp" => Some("pp expression\n\nPretty-print the value of the expression."),
        "whatis" => Some("whatis expression\n\nPrint the type of the argument."),
        "source" => {
            Some("source expression\n\nTry to get source code for the given object and display it.")
        }
        "display" => {
            Some("display [expression]\n\nDisplay the value of the expression if it changed.")
        }
        "undisplay" => Some(
            "undisplay [expression]\n\nDo not display the expression any more in the current frame.",
        ),
        "alias" => {
            Some("alias [name [command]]\n\nCreate an alias called 'name' that executes 'command'.")
        }
        "unalias" => Some("unalias name\n\nDelete the specified alias."),
        "q" | "quit" | "exit" => {
            Some("q(uit) | exit\n\nQuit from the debugger. The program being executed is aborted.")
        }
        "EOF" => Some("EOF\n\nHandles the receipt of EOF as a command."),
        "run" | "restart" => Some("run [args...]\n\nRestart the debugged python program."),
        _ => None,
    }
}

fn format_help_from_doc(prompt: &str, doc: &str) -> String {
    let lines: Vec<&str> = doc.trim_end().lines().map(str::trim).collect();
    if lines.is_empty() {
        return "No help message found.".to_string();
    }
    let usage_end = lines.iter().position(|l| l.is_empty()).unwrap_or(1);
    let indent = " ".repeat(prompt.len());
    let mut formatted = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let prefix = if i == 0 {
            "Usage: "
        } else if i < usage_end {
            "       "
        } else {
            ""
        };
        formatted.push(format!("{indent}{prefix}{line}"));
    }
    formatted.join("\n")
}

fn do_break_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    arg_str: &str,
    temporary: bool,
) -> Result<Value> {
    if arg_str.is_empty() {
        let breaks = interp.get_attr(self_val, "breaks")?;
        if interp.truthy_value(&breaks)? {
            pdb_message(interp, self_val, "Num Type         Disp Enb   Where")?;
            let bp_cls = bdb_attr(interp, "Breakpoint")?;
            let bpbynumber = interp.get_attr(&bp_cls, "bpbynumber")?;
            let bps = interp.collect_iterable(&bpbynumber)?;
            for bp in bps {
                if interp.truthy_value(&bp)? {
                    let fmt = call_method(interp, &bp, "bpformat", &[])?;
                    let s = interp.render_value_as_str(&fmt)?;
                    pdb_message(interp, self_val, &s)?;
                }
            }
        }
        return Ok(Value::none());
    }

    let mut arg = arg_str.to_string();
    let mut cond: Option<String> = None;
    if let Some(comma) = arg.find(',')
        && comma > 0
    {
        let c = arg[comma + 1..].trim_start().to_string();
        if let Some(err) = compile_error_message(interp, &c) {
            pdb_error(interp, self_val, &format!("Invalid condition {c}: {err}"))?;
            return Ok(Value::none());
        }
        cond = Some(c);
        arg = arg[..comma].trim_end().to_string();
    }

    let mut filename: Option<String> = None;
    let lineno: Option<i64>;
    let mut funcname: Option<String> = None;

    if let Some(colon) = arg.rfind(':') {
        let raw_file = arg[..colon].trim_end();
        let found = call_method(interp, self_val, "lookupmodule", &[Value::string(raw_file)])?;
        if !interp.truthy_value(&found)? {
            let repr_f = render_value_repr(interp, &Value::string(raw_file))?;
            pdb_error(
                interp,
                self_val,
                &format!("{repr_f} not found from sys.path"),
            )?;
            return Ok(Value::none());
        }
        filename = Some(interp.render_value_as_str(&found)?);
        let line_part = arg[colon + 1..].trim_start();
        match line_part.parse::<i64>() {
            Ok(n) => lineno = Some(n),
            Err(_) => {
                pdb_error(interp, self_val, &format!("Bad lineno: {line_part}"))?;
                return Ok(Value::none());
            }
        }
    } else if let Ok(n) = arg.trim().parse::<i64>() {
        lineno = Some(n);
    } else {
        let curframe = interp.get_attr(self_val, "curframe")?;
        let f_globals = interp.get_attr(&curframe, "f_globals")?;
        let f_locals = interp.get_attr(self_val, "curframe_locals")?;
        let func = call_pos(
            interp,
            Value::builtin_function("eval"),
            &[Value::string(&arg), f_globals, f_locals],
        )
        .unwrap_or_else(|_| Value::string(&arg));
        let func = interp.get_attr(&func, "__func__").unwrap_or(func);
        if let Ok(code) = interp.get_attr(&func, "__code__") {
            let co_name = interp.get_attr(&code, "co_name")?;
            let co_firstlineno = interp.get_attr(&code, "co_firstlineno")?;
            let co_filename = interp.get_attr(&code, "co_filename")?;
            funcname = Some(interp.render_value_as_str(&co_name)?);
            lineno = Some(co_firstlineno.as_int().unwrap_or(1));
            filename = Some(interp.render_value_as_str(&co_filename)?);
        } else {
            let li = call_method(interp, self_val, "lineinfo", &[Value::string(&arg)])?;
            let parts = interp.collect_iterable(&li)?;
            if parts.len() == 3 && interp.truthy_value(&parts[0])? {
                funcname = Some(interp.render_value_as_str(&parts[0])?);
                filename = Some(interp.render_value_as_str(&parts[1])?);
                lineno = Some(
                    parts[2]
                        .as_int()
                        .or_else(|| parts[2].as_str().and_then(|s| s.parse::<i64>().ok()))
                        .unwrap_or(1),
                );
            } else {
                let repr_arg = render_value_repr(interp, &Value::string(&arg))?;
                pdb_error(
                    interp,
                    self_val,
                    &format!(
                        "The specified object {repr_arg} is not a function or was not found along sys.path."
                    ),
                )?;
                return Ok(Value::none());
            }
        }
    }

    let fname = match filename {
        Some(f) if !f.is_empty() => f,
        _ => {
            let df = call_method(interp, self_val, "defaultFile", &[])?;
            interp.render_value_as_str(&df)?
        }
    };
    let line_num = lineno.unwrap_or(1);
    let checked = call_method(
        interp,
        self_val,
        "checkline",
        &[Value::string(&fname), Value::int(line_num)],
    )?;
    let checked_line = checked.as_int().unwrap_or(0);
    if checked_line != 0 {
        let cond_val = cond.map(Value::string).unwrap_or_else(Value::none);
        let func_val = funcname.map(Value::string).unwrap_or_else(Value::none);
        let err = call_method(
            interp,
            self_val,
            "set_break",
            &[
                Value::string(&fname),
                Value::int(checked_line),
                Value::bool_(temporary),
                cond_val,
                func_val,
            ],
        )?;
        if interp.truthy_value(&err)? {
            let err_s = interp.render_value_as_str(&err)?;
            pdb_error(interp, self_val, &err_s)?;
        } else {
            let bps_val = call_method(
                interp,
                self_val,
                "get_breaks",
                &[Value::string(&fname), Value::int(checked_line)],
            )?;
            let bps = interp.collect_iterable(&bps_val)?;
            if let Some(bp) = bps.last() {
                let num = interp.get_attr(bp, "number")?.as_int().unwrap_or(0);
                let bf = interp.get_attr(bp, "file")?;
                let bf_s = interp.render_value_as_str(&bf)?;
                let bl = interp.get_attr(bp, "line")?.as_int().unwrap_or(0);
                pdb_message(
                    interp,
                    self_val,
                    &format!("Breakpoint {num} at {bf_s}:{bl}"),
                )?;
            }
        }
    }
    Ok(Value::none())
}

/// Wire `Pdb` inheritance from `(bdb.Bdb, cmd.Cmd)` and install all short
/// command aliases on `Pdb`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let pdb_val = mod_rc.borrow().attrs.get("Pdb").cloned();
    let Some(pdb_val) = pdb_val else {
        return;
    };
    let ValueKind::PyClass(pdb_cls) = pdb_val.kind() else {
        return;
    };

    let mut bases = Vec::new();
    if let Ok(bdb_mod) = interp.load_module("bdb")
        && let Ok(bdb_val) = interp.get_attr(&bdb_mod, "Bdb")
        && let ValueKind::PyClass(bdb_cls) = bdb_val.kind()
    {
        bases.push(Rc::clone(bdb_cls));
    }
    if let Ok(cmd_mod) = interp.load_module("cmd")
        && let Ok(cmd_val) = interp.get_attr(&cmd_mod, "Cmd")
        && let ValueKind::PyClass(cmd_cls) = cmd_val.kind()
    {
        bases.push(Rc::clone(cmd_cls));
    }
    if !bases.is_empty() {
        crate::builtin_modules::macros::link_class_bases(pdb_cls, bases);
    }

    // Bind standard CPython command aliases onto Pdb.
    let aliases = [
        ("do_b", "do_break"),
        ("do_cl", "do_clear"),
        ("do_w", "do_where"),
        ("do_bt", "do_where"),
        ("do_u", "do_up"),
        ("do_d", "do_down"),
        ("do_unt", "do_until"),
        ("do_s", "do_step"),
        ("do_n", "do_next"),
        ("do_r", "do_return"),
        ("do_c", "do_continue"),
        ("do_cont", "do_continue"),
        ("do_j", "do_jump"),
        ("do_q", "do_quit"),
        ("do_exit", "do_quit"),
        ("do_a", "do_args"),
        ("do_rv", "do_retval"),
        ("do_l", "do_list"),
        ("do_ll", "do_longlist"),
        ("do_h", "do_help"),
        ("do_restart", "do_run"),
    ];
    let mut cls_mut = pdb_cls.borrow_mut();
    for (alias, target) in aliases {
        if let Some(method) = cls_mut.attrs.get(target).cloned() {
            cls_mut.attrs.insert(alias.to_string(), method);
        }
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("run"),
            Value::string("pm"),
            Value::string("Pdb"),
            Value::string("runeval"),
            Value::string("runctx"),
            Value::string("runcall"),
            Value::string("set_trace"),
            Value::string("post_mortem"),
            Value::string("help"),
        ]),
        "line_prefix" => Value::string(DEFAULT_LINE_PREFIX),
        "TESTCMD" => Value::string("import x; x.main()"),
    }

    /// Causes a debugger to be restarted for the debugged python program.
    exception Restart;

    class Pdb {
        constants {
            "_previous_sigint_handler" => Value::none(),
            "commands_resuming" => Value::list(vec![
                Value::string("do_continue"),
                Value::string("do_step"),
                Value::string("do_next"),
                Value::string("do_return"),
                Value::string("do_quit"),
                Value::string("do_jump"),
            ]),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] completekey: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] stdin: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] stdout: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] skip: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] nosigint: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] readrc: OptValueRef<'_>,
        ) -> Result<Value> {
            let ckey = completekey
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::string("tab"));
            let stdin_val = stdin.as_value().cloned().unwrap_or_else(Value::none);
            let stdout_val = stdout.as_value().cloned().unwrap_or_else(Value::none);
            let skip_val = skip.as_value().cloned().unwrap_or_else(Value::none);
            let nosig_val = nosigint
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::bool_(false));
            let readrc_val = readrc
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::bool_(true));

            // Call bdb.Bdb.__init__(self, skip=skip)
            let bdb_cls = bdb_attr(_interp, "Bdb")?;
            let bdb_init = _interp.get_attr(&bdb_cls, "__init__")?;
            call_kw(
                _interp,
                bdb_init,
                std::slice::from_ref(self_val.0),
                &[("skip", skip_val)],
            )?;

            // Call cmd.Cmd.__init__(self, completekey, stdin, stdout)
            let cmd_mod = _interp.load_module("cmd")?;
            let cmd_cls = _interp.get_attr(&cmd_mod, "Cmd")?;
            let cmd_init = _interp.get_attr(&cmd_cls, "__init__")?;
            call_pos(
                _interp,
                cmd_init,
                &[self_val.0.clone(), ckey, stdin_val, stdout_val.clone()],
            )?;

            // sys.audit("pdb.Pdb")
            if let Ok(sys_mod) = _interp.load_module("sys")
                && let Ok(audit_fn) = _interp.get_attr(&sys_mod, "audit")
            {
                call_pos(_interp, audit_fn, &[Value::string("pdb.Pdb")])?;
            }

            if _interp.truthy_value(&stdout_val)? {
                _interp.assign_attr(self_val.0.clone(), "use_rawinput", Value::int(0))?;
            }
            _interp.assign_attr(self_val.0.clone(), "prompt", Value::string("(Pdb) "))?;
            _interp.assign_attr(self_val.0.clone(), "aliases", Value::dict(PyDict::default()))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "displaying",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "mainpyfile", Value::string(""))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_wait_for_mainpyfile",
                Value::bool_(false),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "tb_lineno",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "allow_kbdint", Value::bool_(false))?;
            _interp.assign_attr(self_val.0.clone(), "nosigint", nosig_val)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_user_requested_quit",
                Value::bool_(false),
            )?;

            let mut rc_lines: Vec<Value> = Vec::new();
            if _interp.truthy_value(&readrc_val)? {
                let mut candidates = Vec::new();
                if let Ok(os_path) = _interp.load_module("os.path")
                    && let Ok(expand) = _interp.get_attr(&os_path, "expanduser")
                    && let Ok(home_rc) =
                        call_pos(_interp, expand, &[Value::string("~/.pdbrc")])
                    && let Some(p) = home_rc.as_str()
                {
                    candidates.push(p.to_string());
                }
                candidates.push(".pdbrc".to_string());
                for path in candidates {
                    if let Ok(contents) = std::fs::read_to_string(&path) {
                        for line in contents.split_inclusive('\n') {
                            rc_lines.push(Value::string(line));
                        }
                    }
                }
            }
            _interp.assign_attr(self_val.0.clone(), "rcLines", Value::list(rc_lines))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "commands",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "commands_doprompt",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "commands_silent",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "commands_defining",
                Value::bool_(false),
            )?;
            _interp.assign_attr(self_val.0.clone(), "commands_bnum", Value::none())?;
            Ok(Value::none())
        }

        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let bdb_cls = bdb_attr(_interp, "Bdb")?;
            let bdb_reset = _interp.get_attr(&bdb_cls, "reset")?;
            call_pos(_interp, bdb_reset, std::slice::from_ref(self_val.0))?;
            call_method(_interp, self_val.0, "forget", &[])?;
            Ok(Value::none())
        }

        fn forget(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "lineno", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "stack", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "curindex", Value::int(0))?;
            if let Ok(cf) = _interp.get_attr(self_val.0, "curframe")
                && !cf.is_none()
                && let Ok(fg) = _interp.get_attr(&cf, "f_globals")
            {
                let _ = call_method(
                    _interp,
                    &fg,
                    "pop",
                    &[Value::string("__pdb_convenience_variables"), Value::none()],
                );
            }
            _interp.assign_attr(self_val.0.clone(), "curframe", Value::none())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "curframe_locals",
                Value::dict(PyDict::default()),
            )?;
            if let Ok(tbl) = _interp.get_attr(self_val.0, "tb_lineno") {
                let _ = call_method(_interp, &tbl, "clear", &[]);
            }
            Ok(Value::none())
        }

        fn setup(
            #[positional_only] self_val: PyValueRef<'_>,
            f: PyValue,
            tb: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "forget", &[])?;
            let pair = call_method(_interp, self_val.0, "get_stack", &[f.0, tb.0.clone()])?;
            let items = _interp.collect_iterable(&pair)?;
            let stack_val = items.first().cloned().unwrap_or_else(|| Value::list(Vec::new()));
            let curindex_val = items.get(1).cloned().unwrap_or_else(|| Value::int(0));
            _interp.assign_attr(self_val.0.clone(), "stack", stack_val.clone())?;
            _interp.assign_attr(self_val.0.clone(), "curindex", curindex_val.clone())?;

            let tb_lineno_dict = _interp.get_attr(self_val.0, "tb_lineno")?;
            let mut cur_tb = tb.0;
            while !cur_tb.is_none() {
                let tb_frame = _interp.get_attr(&cur_tb, "tb_frame")?;
                let tb_code = _interp.get_attr(&tb_frame, "f_code")?;
                let tb_lasti = _interp
                    .get_attr(&cur_tb, "tb_lasti")
                    .ok()
                    .and_then(|v| v.as_int())
                    .unwrap_or(-1);
                let ln = if tb_lasti >= 0 {
                    lasti2lineno_impl(_interp, &tb_code, tb_lasti)?
                } else {
                    _interp
                        .get_attr(&cur_tb, "tb_lineno")?
                        .as_int()
                        .unwrap_or(0)
                };
                call_method(
                    _interp,
                    &tb_lineno_dict,
                    "__setitem__",
                    &[tb_frame, Value::int(ln)],
                )?;
                cur_tb = _interp.get_attr(&cur_tb, "tb_next")?;
            }

            let cur_entry = _interp.eval_index(&stack_val, curindex_val)?;
            let cur_frame = _interp.eval_index(&cur_entry, Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "curframe", cur_frame.clone())?;
            let f_locals = _interp.get_attr(&cur_frame, "f_locals")?;
            _interp.assign_attr(self_val.0.clone(), "curframe_locals", f_locals)?;
            call_method(
                _interp,
                self_val.0,
                "set_convenience_variable",
                &[cur_frame.clone(), Value::string("_frame"), cur_frame],
            )?;

            let rc_lines = _interp.get_attr(self_val.0, "rcLines")?;
            if _interp.truthy_value(&rc_lines)? {
                let raw = _interp.collect_iterable(&rc_lines)?;
                let filtered: Vec<Value> = raw
                    .into_iter()
                    .filter(|v| {
                        let s = v.as_str().unwrap_or("").trim();
                        !s.is_empty() && !s.starts_with('#')
                    })
                    .collect();
                _interp.assign_attr(self_val.0.clone(), "cmdqueue", Value::list(filtered))?;
                _interp.assign_attr(self_val.0.clone(), "rcLines", Value::list(Vec::new()))?;
            }
            Ok(Value::none())
        }

        fn user_call(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            _argument_list: PyValue,
        ) -> Result<Value> {
            let wait = _interp.get_attr(self_val.0, "_wait_for_mainpyfile")?;
            if _interp.truthy_value(&wait)? {
                return Ok(Value::none());
            }
            let stop = call_method(
                _interp,
                self_val.0,
                "stop_here",
                std::slice::from_ref(&frame.0),
            )?;
            if _interp.truthy_value(&stop)? {
                pdb_message(_interp, self_val.0, "--Call--")?;
                call_method(_interp, self_val.0, "interaction", &[frame.0, Value::none()])?;
            }
            Ok(Value::none())
        }

        fn user_line(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            let wait = _interp.get_attr(self_val.0, "_wait_for_mainpyfile")?;
            if _interp.truthy_value(&wait)? {
                let mainpy = _interp.get_attr(self_val.0, "mainpyfile")?;
                let code = _interp.get_attr(&frame.0, "f_code")?;
                let fname = _interp.get_attr(&code, "co_filename")?;
                let canon = call_method(_interp, self_val.0, "canonic", &[fname])?;
                if mainpy.as_str() != canon.as_str() {
                    return Ok(Value::none());
                }
                _interp.assign_attr(
                    self_val.0.clone(),
                    "_wait_for_mainpyfile",
                    Value::bool_(false),
                )?;
            }
            let should_interact = call_method(
                _interp,
                self_val.0,
                "bp_commands",
                std::slice::from_ref(&frame.0),
            )?;
            if _interp.truthy_value(&should_interact)? {
                call_method(_interp, self_val.0, "interaction", &[frame.0, Value::none()])?;
            }
            Ok(Value::none())
        }

        fn bp_commands(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            let curbp = _interp
                .get_attr(self_val.0, "currentbp")
                .unwrap_or_else(|_| Value::bool_(false));
            let commands = _interp.get_attr(self_val.0, "commands")?;
            if _interp.truthy_value(&curbp)? {
                let has = call_method(
                    _interp,
                    &commands,
                    "__contains__",
                    std::slice::from_ref(&curbp),
                )?;
                if _interp.truthy_value(&has)? {
                    _interp.assign_attr(self_val.0.clone(), "currentbp", Value::int(0))?;
                    let lastcmd_back = _interp.get_attr(self_val.0, "lastcmd")?;
                    call_method(_interp, self_val.0, "setup", &[frame.0, Value::none()])?;
                    let cmd_list_val = _interp.eval_index(&commands, curbp.clone())?;
                    let cmd_list = _interp.collect_iterable(&cmd_list_val)?;
                    for line in cmd_list {
                        call_method(_interp, self_val.0, "onecmd", &[line])?;
                    }
                    _interp.assign_attr(self_val.0.clone(), "lastcmd", lastcmd_back)?;
                    let silent_map = _interp.get_attr(self_val.0, "commands_silent")?;
                    let is_silent = _interp.eval_index(&silent_map, curbp.clone())?;
                    if !_interp.truthy_value(&is_silent)? {
                        let stack = _interp.get_attr(self_val.0, "stack")?;
                        let curidx = _interp.get_attr(self_val.0, "curindex")?;
                        let entry = _interp.eval_index(&stack, curidx)?;
                        call_method(_interp, self_val.0, "print_stack_entry", &[entry])?;
                    }
                    let doprompt_map = _interp.get_attr(self_val.0, "commands_doprompt")?;
                    let doprompt = _interp.eval_index(&doprompt_map, curbp)?;
                    if _interp.truthy_value(&doprompt)? {
                        call_method(_interp, self_val.0, "_cmdloop", &[])?;
                    }
                    call_method(_interp, self_val.0, "forget", &[])?;
                    return Ok(Value::none());
                }
            }
            Ok(Value::int(1))
        }

        fn user_return(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            return_value: PyValue,
        ) -> Result<Value> {
            let wait = _interp.get_attr(self_val.0, "_wait_for_mainpyfile")?;
            if _interp.truthy_value(&wait)? {
                return Ok(Value::none());
            }
            let f_locals = _interp.get_attr(&frame.0, "f_locals")?;
            call_method(
                _interp,
                &f_locals,
                "__setitem__",
                &[Value::string("__return__"), return_value.0.clone()],
            )?;
            call_method(
                _interp,
                self_val.0,
                "set_convenience_variable",
                &[frame.0.clone(), Value::string("_retval"), return_value.0],
            )?;
            pdb_message(_interp, self_val.0, "--Return--")?;
            call_method(_interp, self_val.0, "interaction", &[frame.0, Value::none()])?;
            Ok(Value::none())
        }

        fn user_exception(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            exc_info: PyValue,
        ) -> Result<Value> {
            let wait = _interp.get_attr(self_val.0, "_wait_for_mainpyfile")?;
            if _interp.truthy_value(&wait)? {
                return Ok(Value::none());
            }
            let items = _interp.collect_iterable(&exc_info.0)?;
            let exc_type = items.first().cloned().unwrap_or_else(Value::none);
            let exc_value = items.get(1).cloned().unwrap_or_else(Value::none);
            let exc_tb = items.get(2).cloned().unwrap_or_else(Value::none);

            let f_locals = _interp.get_attr(&frame.0, "f_locals")?;
            call_method(
                _interp,
                &f_locals,
                "__setitem__",
                &[
                    Value::string("__exception__"),
                    Value::tuple(vec![exc_type.clone(), exc_value.clone()]),
                ],
            )?;
            call_method(
                _interp,
                self_val.0,
                "set_convenience_variable",
                &[
                    frame.0.clone(),
                    Value::string("_exception"),
                    exc_value.clone(),
                ],
            )?;
            let stop_iter_cls = crate::interpreter::lookup_exc_class("StopIteration");
            let is_internal_stop = exc_tb.is_none()
                && stop_iter_cls
                    .as_ref()
                    .is_some_and(|c| values_are_identical(&exc_type, &Value::py_class(c.clone())));
            let prefix = if is_internal_stop { "Internal " } else { "" };
            let formatted = format_exc_only(_interp, &exc_value);
            pdb_message(_interp, self_val.0, &format!("{prefix}{formatted}"))?;
            call_method(_interp, self_val.0, "interaction", &[frame.0, exc_tb])?;
            Ok(Value::none())
        }

        fn _cmdloop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            loop {
                _interp.assign_attr(self_val.0.clone(), "allow_kbdint", Value::bool_(true))?;
                match call_method(_interp, self_val.0, "cmdloop", &[]) {
                    Ok(_) => {
                        _interp.assign_attr(
                            self_val.0.clone(),
                            "allow_kbdint",
                            Value::bool_(false),
                        )?;
                        break;
                    }
                    Err(e) if is_exc_type(_interp, &e, "KeyboardInterrupt") => {
                        pdb_message(_interp, self_val.0, "--KeyboardInterrupt--")?;
                    }
                    Err(e) => {
                        let _ = _interp.assign_attr(
                            self_val.0.clone(),
                            "allow_kbdint",
                            Value::bool_(false),
                        );
                        return Err(e);
                    }
                }
            }
            Ok(Value::none())
        }

        fn _show_display(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let displaying = _interp.get_attr(self_val.0, "displaying")?;
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            let entry = call_method(_interp, &displaying, "get", &[curframe])?;
            if !_interp.truthy_value(&entry)? {
                return Ok(Value::none());
            }
            let items_val = call_method(_interp, &entry, "items", &[])?;
            let pairs = _interp.collect_iterable(&items_val)?;
            for pair in pairs {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() != 2 {
                    continue;
                }
                let expr_str = kv[0].as_str().unwrap_or("").to_string();
                let old_val = &kv[1];
                let new_val = eval_in_frame_except(_interp, self_val.0, &expr_str, None);
                let changed = !values_are_identical(&new_val, old_val)
                    && _interp
                        .eval_binary(new_val.clone(), crate::ast::BinaryOp::Ne, old_val.clone())
                        .and_then(|v| _interp.truthy_value(&v))
                        .unwrap_or(true);
                if changed {
                    call_method(
                        _interp,
                        &entry,
                        "__setitem__",
                        &[kv[0].clone(), new_val.clone()],
                    )?;
                    let new_r = safe_repr(_interp, &new_val, &expr_str);
                    let old_r = safe_repr(_interp, old_val, &expr_str);
                    pdb_message(
                        _interp,
                        self_val.0,
                        &format!("display {expr_str}: {new_r}  [old: {old_r}]"),
                    )?;
                }
            }
            Ok(Value::none())
        }

        fn interaction(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            traceback: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "setup", &[frame.0, traceback.0])?;
            let cmdqueue = _interp.get_attr(self_val.0, "cmdqueue")?;
            call_method(
                _interp,
                &cmdqueue,
                "append",
                &[Value::string("_pdbcmd_print_frame_status")],
            )?;
            let loop_res = call_method(_interp, self_val.0, "_cmdloop", &[]);
            if let Ok(cq) = _interp.get_attr(self_val.0, "cmdqueue")
                && let Ok(items) = _interp.collect_iterable(&cq)
                && items
                    .last()
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s == "_pdbcmd_print_frame_status")
            {
                let _ = call_method(_interp, &cq, "pop", &[]);
            }
            let _ = call_method(_interp, self_val.0, "forget", &[]);
            loop_res?;
            Ok(Value::none())
        }

        fn _pdbcmd_print_frame_status(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _arg: OptValueRef<'_>,
        ) -> Result<Value> {
            let stack = _interp.get_attr(self_val.0, "stack")?;
            let curindex = _interp.get_attr(self_val.0, "curindex")?;
            let entry = _interp.eval_index(&stack, curindex)?;
            call_method(_interp, self_val.0, "print_stack_entry", &[entry])?;
            call_method(_interp, self_val.0, "_show_display", &[])?;
            Ok(Value::none())
        }

        fn displayhook(
            #[positional_only] self_val: PyValueRef<'_>,
            obj: PyValue,
        ) -> Result<Value> {
            if !obj.0.is_none() {
                let r = render_value_repr(_interp, &obj.0)?;
                pdb_message(_interp, self_val.0, &r)?;
            }
            Ok(Value::none())
        }

        fn default(
            #[positional_only] self_val: PyValueRef<'_>,
            line: PyStr<'_>,
        ) -> Result<Value> {
            let mut src: &str = line.0.as_ref();
            if let Some(rest) = src.strip_prefix('!') {
                src = rest.trim();
            }
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            let globals = _interp.get_attr(&curframe, "f_globals")?;
            let locals = _interp.get_attr(self_val.0, "curframe_locals")?;

            let code = match call_pos(
                _interp,
                Value::builtin_function("compile"),
                &[
                    Value::string(format!("{src}\n")),
                    Value::string("<stdin>"),
                    Value::string("single"),
                ],
            ) {
                Ok(c) => c,
                Err(e) => {
                    let msg = format_pyerror(_interp, &e);
                    pdb_error(_interp, self_val.0, &msg)?;
                    return Ok(Value::none());
                }
            };

            let sys_mod = _interp.load_module("sys")?;
            let save_stdout = _interp.get_attr(&sys_mod, "stdout")?;
            let save_stdin = _interp.get_attr(&sys_mod, "stdin")?;
            let save_dh = _interp.get_attr(&sys_mod, "displayhook")?;
            let my_stdin = _interp.get_attr(self_val.0, "stdin")?;
            let my_stdout = _interp.get_attr(self_val.0, "stdout")?;
            let my_dh = _interp.get_attr(self_val.0, "displayhook")?;

            _interp.assign_attr(sys_mod.clone(), "stdin", my_stdin)?;
            _interp.assign_attr(sys_mod.clone(), "stdout", my_stdout)?;
            _interp.assign_attr(sys_mod.clone(), "displayhook", my_dh)?;
            let exec_res = call_pos(
                _interp,
                Value::builtin_function("exec"),
                &[code, globals, locals],
            );
            let _ = _interp.assign_attr(sys_mod.clone(), "stdout", save_stdout);
            let _ = _interp.assign_attr(sys_mod.clone(), "stdin", save_stdin);
            let _ = _interp.assign_attr(sys_mod, "displayhook", save_dh);

            if let Err(e) = exec_res {
                let msg = format_pyerror(_interp, &e);
                pdb_error(_interp, self_val.0, &msg)?;
            }
            Ok(Value::none())
        }

        fn precmd(
            #[positional_only] self_val: PyValueRef<'_>,
            line: PyStr<'_>,
        ) -> Result<Value> {
            let mut s = line.0.to_string();
            if s.trim().is_empty() {
                return Ok(Value::string(s));
            }
            let aliases = _interp.get_attr(self_val.0, "aliases")?;
            let mut guard = 0usize;
            while guard < 64 {
                guard += 1;
                let parts: Vec<&str> = s.split_whitespace().collect();
                if parts.is_empty() {
                    break;
                }
                let first = parts[0];
                let has = call_method(
                    _interp,
                    &aliases,
                    "__contains__",
                    &[Value::string(first)],
                )?;
                if !_interp.truthy_value(&has)? {
                    break;
                }
                let tmpl_val = _interp.eval_index(&aliases, Value::string(first))?;
                let mut expanded = _interp.render_value_as_str(&tmpl_val)?;
                for (i, arg) in parts[1..].iter().enumerate() {
                    expanded = expanded.replace(&format!("%{}", i + 1), arg);
                }
                expanded = expanded.replace("%*", &parts[1..].join(" "));
                s = expanded;
            }
            let first_word = s.split_whitespace().next().unwrap_or("");
            if first_word != "alias"
                && let Some(marker) = s.find(";;")
            {
                let next_cmd = s[marker + 2..].trim_start().to_string();
                let cmdqueue = _interp.get_attr(self_val.0, "cmdqueue")?;
                call_method(
                    _interp,
                    &cmdqueue,
                    "insert",
                    &[Value::int(0), Value::string(next_cmd)],
                )?;
                s = s[..marker].trim_end().to_string();
            }
            Ok(Value::string(s))
        }

        fn onecmd(
            #[positional_only] self_val: PyValueRef<'_>,
            line: PyStr<'_>,
        ) -> Result<Value> {
            let line_str: &str = line.0.as_ref();
            let defining = _interp.get_attr(self_val.0, "commands_defining")?;
            if !_interp.truthy_value(&defining)? {
                if line_str.starts_with("_pdbcmd") {
                    let parsed = call_method(
                        _interp,
                        self_val.0,
                        "parseline",
                        &[Value::string(line_str)],
                    )?;
                    let parts = _interp.collect_iterable(&parsed)?;
                    if let Some(cmd_name) = parts.first().and_then(|v| v.as_str())
                        && let Ok(method) = _interp.get_attr(self_val.0, cmd_name)
                    {
                        let arg_val = parts.get(1).cloned().unwrap_or_else(|| Value::string(""));
                        return call_pos(_interp, method, &[arg_val]);
                    }
                }
                let cmd_mod = _interp.load_module("cmd")?;
                let cmd_cls = _interp.get_attr(&cmd_mod, "Cmd")?;
                let cmd_onecmd = _interp.get_attr(&cmd_cls, "onecmd")?;
                return call_pos(
                    _interp,
                    cmd_onecmd,
                    &[self_val.0.clone(), Value::string(line_str)],
                );
            }
            Ok(Value::bool_(false))
        }

        fn message(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
        ) -> Result<Value> {
            let stdout = _interp.get_attr(self_val.0, "stdout")?;
            let text = _interp.render_value_as_str(&msg.0)?;
            call_method(_interp, &stdout, "write", &[Value::string(format!("{text}\n"))])?;
            Ok(Value::none())
        }

        fn error(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
        ) -> Result<Value> {
            let stdout = _interp.get_attr(self_val.0, "stdout")?;
            let text = _interp.render_value_as_str(&msg.0)?;
            call_method(
                _interp,
                &stdout,
                "write",
                &[Value::string(format!("*** {text}\n"))],
            )?;
            Ok(Value::none())
        }

        fn set_convenience_variable(
            #[positional_only] _self_val: PyValueRef<'_>,
            frame: PyValue,
            name: PyValue,
            value: PyValue,
        ) -> Result<Value> {
            let f_globals = _interp.get_attr(&frame.0, "f_globals")?;
            let key = Value::string("__pdb_convenience_variables");
            let has = call_method(_interp, &f_globals, "__contains__", std::slice::from_ref(&key))?;
            let cv = if _interp.truthy_value(&has)? {
                _interp.eval_index(&f_globals, key)?
            } else {
                let d = Value::dict(PyDict::default());
                call_method(_interp, &f_globals, "__setitem__", &[key, d.clone()])?;
                d
            };
            call_method(_interp, &cv, "__setitem__", &[name.0, value.0])?;
            Ok(Value::none())
        }

        fn defaultFile(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            let code = _interp.get_attr(&curframe, "f_code")?;
            let fname = _interp.get_attr(&code, "co_filename")?;
            if fname.as_str() == Some("<string>") {
                let mainpy = _interp.get_attr(self_val.0, "mainpyfile")?;
                if _interp.truthy_value(&mainpy)? {
                    return Ok(mainpy);
                }
            }
            Ok(fname)
        }

        fn lineinfo(
            #[positional_only] self_val: PyValueRef<'_>,
            identifier: PyStr<'_>,
        ) -> Result<Value> {
            let failed = || Value::tuple(vec![Value::none(), Value::none(), Value::none()]);
            let idstring: Vec<&str> = identifier.0.split('\'').collect();
            let id = if idstring.len() == 1 {
                idstring[0].trim()
            } else if idstring.len() == 3 {
                idstring[1].trim()
            } else {
                return Ok(failed());
            };
            if id.is_empty() {
                return Ok(failed());
            }
            let mut parts: Vec<&str> = id.split('.').collect();
            if parts.first() == Some(&"self") {
                parts.remove(0);
                if parts.is_empty() {
                    return Ok(failed());
                }
            }
            let df = call_method(_interp, self_val.0, "defaultFile", &[])?;
            let mut fname = _interp.render_value_as_str(&df)?;
            let item = if parts.len() == 1 {
                parts[0]
            } else {
                let f = call_method(_interp, self_val.0, "lookupmodule", &[Value::string(parts[0])])?;
                if _interp.truthy_value(&f)? {
                    fname = _interp.render_value_as_str(&f)?;
                }
                parts[1]
            };
            match find_function_impl(_interp, item, &fname)? {
                Some((n, f, l)) => Ok(Value::tuple(vec![
                    Value::string(n),
                    Value::string(f),
                    Value::int(l),
                ])),
                None => Ok(failed()),
            }
        }

        fn checkline(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
            lineno: PyValue,
        ) -> Result<Value> {
            let line_num = lineno.0.as_int().unwrap_or(0);
            let globs = match _interp.get_attr(self_val.0, "curframe") {
                Ok(f) if !f.is_none() => _interp.get_attr(&f, "f_globals")?,
                _ => Value::none(),
            };
            let lc = _interp.load_module("linecache")?;
            let getline = _interp.get_attr(&lc, "getline")?;
            let line_val = call_pos(
                _interp,
                getline,
                &[Value::string(filename.0), Value::int(line_num), globs],
            )?;
            let line_str = line_val.as_str().unwrap_or("");
            if line_str.is_empty() {
                pdb_message(_interp, self_val.0, "End of file")?;
                return Ok(Value::int(0));
            }
            let trimmed = line_str.trim();
            if trimmed.is_empty()
                || trimmed.starts_with('#')
                || trimmed.starts_with("\"\"\"")
                || trimmed.starts_with("'''")
            {
                pdb_error(_interp, self_val.0, "Blank or comment")?;
                return Ok(Value::int(0));
            }
            Ok(Value::int(line_num))
        }

        fn lookupmodule(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let fname_ref: &str = filename.0.as_ref();
            let os_path = _interp.load_module("os.path")?;
            let isabs = _interp.get_attr(&os_path, "isabs")?;
            let exists = _interp.get_attr(&os_path, "exists")?;
            let join_fn = _interp.get_attr(&os_path, "join")?;
            let splitext = _interp.get_attr(&os_path, "splitext")?;

            let fval = Value::string(fname_ref);
            let is_abs_v = call_pos(_interp, isabs.clone(), std::slice::from_ref(&fval))?;
            let is_abs = _interp.truthy_value(&is_abs_v)?;
            if is_abs {
                let ex_v = call_pos(_interp, exists.clone(), std::slice::from_ref(&fval))?;
                if _interp.truthy_value(&ex_v)? {
                    return Ok(fval);
                }
            }

            let sys_mod = _interp.load_module("sys")?;
            let sys_path = _interp.get_attr(&sys_mod, "path")?;
            let path_items = _interp.collect_iterable(&sys_path)?;
            if let Some(p0) = path_items.first() {
                let candidate = call_pos(_interp, join_fn.clone(), &[p0.clone(), fval.clone()])?;
                let ex_c = call_pos(_interp, exists.clone(), std::slice::from_ref(&candidate))?;
                if _interp.truthy_value(&ex_c)? {
                    let mainpy = _interp.get_attr(self_val.0, "mainpyfile")?;
                    let canon = call_method(
                        _interp,
                        self_val.0,
                        "canonic",
                        std::slice::from_ref(&candidate),
                    )?;
                    if mainpy.as_str() == canon.as_str() {
                        return Ok(candidate);
                    }
                }
            }

            let se = call_pos(_interp, splitext, std::slice::from_ref(&fval))?;
            let se_parts = _interp.collect_iterable(&se)?;
            let ext = se_parts.get(1).and_then(|v| v.as_str()).unwrap_or("");
            let fname_py = if ext.is_empty() {
                format!("{fname_ref}.py")
            } else {
                fname_ref.to_string()
            };
            let fname_py_val = Value::string(&fname_py);
            let is_abs2 = call_pos(_interp, isabs, std::slice::from_ref(&fname_py_val))?;
            if _interp.truthy_value(&is_abs2)? {
                return Ok(fname_py_val);
            }
            for dirname in path_items {
                let fullname = call_pos(_interp, join_fn.clone(), &[dirname, fname_py_val.clone()])?;
                let ex_full = call_pos(_interp, exists.clone(), std::slice::from_ref(&fullname))?;
                if _interp.truthy_value(&ex_full)? {
                    return Ok(fullname);
                }
            }
            Ok(Value::none())
        }

        // ── Debugger Commands ────────────────────────────────────────────────

        fn do_break(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
            #[default(OptionalArg::Omitted)] temporary: OptValueRef<'_>,
        ) -> Result<Value> {
            let is_temp = match temporary.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => false,
            };
            do_break_impl(_interp, self_val.0, arg.0.as_ref(), is_temp)
        }

        fn do_tbreak(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            do_break_impl(_interp, self_val.0, arg.0.as_ref(), true)
        }

        fn do_enable(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            for tok in arg.0.split_whitespace() {
                match call_method(_interp, self_val.0, "get_bpbynumber", &[Value::string(tok)]) {
                    Ok(bp) => {
                        call_method(_interp, &bp, "enable", &[])?;
                        let s = _interp.render_value_as_str(&bp)?;
                        pdb_message(_interp, self_val.0, &format!("Enabled {s}"))?;
                    }
                    Err(e) => {
                        let msg = format_pyerror(_interp, &e);
                        let stripped = msg.strip_prefix("ValueError: ").unwrap_or(&msg);
                        pdb_error(_interp, self_val.0, stripped)?;
                    }
                }
            }
            Ok(Value::none())
        }

        fn do_disable(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            for tok in arg.0.split_whitespace() {
                match call_method(_interp, self_val.0, "get_bpbynumber", &[Value::string(tok)]) {
                    Ok(bp) => {
                        call_method(_interp, &bp, "disable", &[])?;
                        let s = _interp.render_value_as_str(&bp)?;
                        pdb_message(_interp, self_val.0, &format!("Disabled {s}"))?;
                    }
                    Err(e) => {
                        let msg = format_pyerror(_interp, &e);
                        let stripped = msg.strip_prefix("ValueError: ").unwrap_or(&msg);
                        pdb_error(_interp, self_val.0, stripped)?;
                    }
                }
            }
            Ok(Value::none())
        }

        fn do_condition(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let (bp_tok, cond) = match arg.0.split_once(' ') {
                Some((a, b)) => {
                    if let Some(err) = compile_error_message(_interp, b) {
                        pdb_error(_interp, self_val.0, &format!("Invalid condition {b}: {err}"))?;
                        return Ok(Value::none());
                    }
                    (a.trim(), Some(b))
                }
                None => (arg.0.trim(), None),
            };
            match call_method(_interp, self_val.0, "get_bpbynumber", &[Value::string(bp_tok)]) {
                Ok(bp) => {
                    let cond_val = cond.map(Value::string).unwrap_or_else(Value::none);
                    _interp.assign_attr(bp.clone(), "cond", cond_val)?;
                    let num = _interp.get_attr(&bp, "number")?.as_int().unwrap_or(0);
                    if cond.is_none() || cond == Some("") {
                        pdb_message(
                            _interp,
                            self_val.0,
                            &format!("Breakpoint {num} is now unconditional."),
                        )?;
                    } else {
                        pdb_message(
                            _interp,
                            self_val.0,
                            &format!("New condition set for breakpoint {num}."),
                        )?;
                    }
                }
                Err(e) => {
                    let msg = format_pyerror(_interp, &e);
                    let stripped = msg.strip_prefix("ValueError: ").unwrap_or(&msg);
                    pdb_error(_interp, self_val.0, stripped)?;
                }
            }
            Ok(Value::none())
        }

        fn do_ignore(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let parts: Vec<&str> = arg.0.split_whitespace().collect();
            if parts.is_empty() {
                pdb_error(_interp, self_val.0, "Breakpoint number expected")?;
                return Ok(Value::none());
            }
            let count = parts
                .get(1)
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            match call_method(
                _interp,
                self_val.0,
                "get_bpbynumber",
                &[Value::string(parts[0])],
            ) {
                Ok(bp) => {
                    _interp.assign_attr(bp.clone(), "ignore", Value::int(count))?;
                    let num = _interp.get_attr(&bp, "number")?.as_int().unwrap_or(0);
                    if count > 0 {
                        let countstr = if count > 1 {
                            format!("{count} crossings")
                        } else {
                            "1 crossing".to_string()
                        };
                        pdb_message(
                            _interp,
                            self_val.0,
                            &format!("Will ignore next {countstr} of breakpoint {num}."),
                        )?;
                    } else {
                        pdb_message(
                            _interp,
                            self_val.0,
                            &format!("Will stop next time breakpoint {num} is reached."),
                        )?;
                    }
                }
                Err(e) => {
                    let msg = format_pyerror(_interp, &e);
                    let stripped = msg.strip_prefix("ValueError: ").unwrap_or(&msg);
                    pdb_error(_interp, self_val.0, stripped)?;
                }
            }
            Ok(Value::none())
        }

        fn do_clear(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            if arg.0.is_empty() {
                let use_raw = _interp
                    .get_attr(self_val.0, "use_rawinput")
                    .and_then(|v| _interp.truthy_value(&v))
                    .unwrap_or(false);
                let reply = if use_raw {
                    call_pos(
                        _interp,
                        Value::builtin_function("input"),
                        &[Value::string("Clear all breaks? ")],
                    )
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "no".to_string())
                } else {
                    let stdout = _interp.get_attr(self_val.0, "stdout")?;
                    let _ = call_method(
                        _interp,
                        &stdout,
                        "write",
                        &[Value::string("Clear all breaks? ")],
                    );
                    let stdin = _interp.get_attr(self_val.0, "stdin")?;
                    call_method(_interp, &stdin, "readline", &[])
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "no".to_string())
                };
                let r = reply.trim().to_ascii_lowercase();
                if r == "y" || r == "yes" {
                    let bp_cls = bdb_attr(_interp, "Breakpoint")?;
                    let bpbynumber = _interp.get_attr(&bp_cls, "bpbynumber")?;
                    let bplist: Vec<Value> = _interp
                        .collect_iterable(&bpbynumber)?
                        .into_iter()
                        .filter(|v| !v.is_none())
                        .collect();
                    call_method(_interp, self_val.0, "clear_all_breaks", &[])?;
                    for bp in bplist {
                        let s = _interp.render_value_as_str(&bp)?;
                        pdb_message(_interp, self_val.0, &format!("Deleted {s}"))?;
                    }
                }
                return Ok(Value::none());
            }
            if let Some(colon) = arg.0.rfind(':') {
                let filename = &arg.0[..colon];
                let line_part = &arg.0[colon + 1..];
                match line_part.trim().parse::<i64>() {
                    Err(_) => {
                        pdb_error(_interp, self_val.0, &format!("Invalid line number ({line_part})"))?;
                    }
                    Ok(lineno) => {
                        let bps_val = call_method(
                            _interp,
                            self_val.0,
                            "get_breaks",
                            &[Value::string(filename), Value::int(lineno)],
                        )?;
                        let bps = _interp.collect_iterable(&bps_val)?;
                        let err = call_method(
                            _interp,
                            self_val.0,
                            "clear_break",
                            &[Value::string(filename), Value::int(lineno)],
                        )?;
                        if _interp.truthy_value(&err)? {
                            let es = _interp.render_value_as_str(&err)?;
                            pdb_error(_interp, self_val.0, &es)?;
                        } else {
                            for bp in bps {
                                let s = _interp.render_value_as_str(&bp)?;
                                pdb_message(_interp, self_val.0, &format!("Deleted {s}"))?;
                            }
                        }
                    }
                }
                return Ok(Value::none());
            }
            for tok in arg.0.split_whitespace() {
                match call_method(_interp, self_val.0, "get_bpbynumber", &[Value::string(tok)]) {
                    Ok(bp) => {
                        call_method(_interp, self_val.0, "clear_bpbynumber", &[Value::string(tok)])?;
                        let s = _interp.render_value_as_str(&bp)?;
                        pdb_message(_interp, self_val.0, &format!("Deleted {s}"))?;
                    }
                    Err(e) => {
                        let msg = format_pyerror(_interp, &e);
                        let stripped = msg.strip_prefix("ValueError: ").unwrap_or(&msg);
                        pdb_error(_interp, self_val.0, stripped)?;
                    }
                }
            }
            Ok(Value::none())
        }

        fn do_where(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "print_stack_trace", &[])?;
            Ok(Value::none())
        }

        fn _select_frame(
            #[positional_only] self_val: PyValueRef<'_>,
            number: PyValue,
        ) -> Result<Value> {
            let stack = _interp.get_attr(self_val.0, "stack")?;
            _interp.assign_attr(self_val.0.clone(), "curindex", number.0.clone())?;
            let entry = _interp.eval_index(&stack, number.0)?;
            let frame = _interp.eval_index(&entry, Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "curframe", frame.clone())?;
            let f_locals = _interp.get_attr(&frame, "f_locals")?;
            _interp.assign_attr(self_val.0.clone(), "curframe_locals", f_locals)?;
            call_method(
                _interp,
                self_val.0,
                "set_convenience_variable",
                &[frame.clone(), Value::string("_frame"), frame],
            )?;
            call_method(_interp, self_val.0, "print_stack_entry", &[entry])?;
            _interp.assign_attr(self_val.0.clone(), "lineno", Value::none())?;
            Ok(Value::none())
        }

        fn do_up(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let curindex = _interp
                .get_attr(self_val.0, "curindex")?
                .as_int()
                .unwrap_or(0);
            if curindex == 0 {
                pdb_error(_interp, self_val.0, "Oldest frame")?;
                return Ok(Value::none());
            }
            let count = if arg.0.trim().is_empty() {
                1i64
            } else {
                match arg.0.trim().parse::<i64>() {
                    Ok(n) => n,
                    Err(_) => {
                        pdb_error(_interp, self_val.0, &format!("Invalid frame count ({})", arg.0))?;
                        return Ok(Value::none());
                    }
                }
            };
            let newframe = if count < 0 { 0 } else { (curindex - count).max(0) };
            call_method(_interp, self_val.0, "_select_frame", &[Value::int(newframe)])?;
            Ok(Value::none())
        }

        fn do_down(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let curindex = _interp
                .get_attr(self_val.0, "curindex")?
                .as_int()
                .unwrap_or(0);
            let stack = _interp.get_attr(self_val.0, "stack")?;
            let stack_len = _interp.collect_iterable(&stack)?.len() as i64;
            if curindex + 1 >= stack_len {
                pdb_error(_interp, self_val.0, "Newest frame")?;
                return Ok(Value::none());
            }
            let count = if arg.0.trim().is_empty() {
                1i64
            } else {
                match arg.0.trim().parse::<i64>() {
                    Ok(n) => n,
                    Err(_) => {
                        pdb_error(_interp, self_val.0, &format!("Invalid frame count ({})", arg.0))?;
                        return Ok(Value::none());
                    }
                }
            };
            let newframe = if count < 0 {
                stack_len - 1
            } else {
                (curindex + count).min(stack_len - 1)
            };
            call_method(_interp, self_val.0, "_select_frame", &[Value::int(newframe)])?;
            Ok(Value::none())
        }

        fn do_until(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            let lineno = if !arg.0.trim().is_empty() {
                let n = match arg.0.trim().parse::<i64>() {
                    Ok(n) => n,
                    Err(_) => {
                        let r = render_value_repr(_interp, &Value::string(arg.0))?;
                        pdb_error(_interp, self_val.0, &format!("Error in argument: {r}"))?;
                        return Ok(Value::none());
                    }
                };
                let cur_line = _interp
                    .get_attr(&curframe, "f_lineno")?
                    .as_int()
                    .unwrap_or(0);
                if n <= cur_line {
                    pdb_error(
                        _interp,
                        self_val.0,
                        "\"until\" line number is smaller than current line number",
                    )?;
                    return Ok(Value::none());
                }
                Value::int(n)
            } else {
                Value::none()
            };
            call_method(_interp, self_val.0, "set_until", &[curframe, lineno])?;
            Ok(Value::int(1))
        }

        fn do_step(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "set_step", &[])?;
            Ok(Value::int(1))
        }

        fn do_next(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            call_method(_interp, self_val.0, "set_next", &[curframe])?;
            Ok(Value::int(1))
        }

        fn do_return(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            call_method(_interp, self_val.0, "set_return", &[curframe])?;
            Ok(Value::int(1))
        }

        fn do_continue(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "set_continue", &[])?;
            Ok(Value::int(1))
        }

        fn do_jump(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let curindex = _interp
                .get_attr(self_val.0, "curindex")?
                .as_int()
                .unwrap_or(0);
            let stack = _interp.get_attr(self_val.0, "stack")?;
            let stack_len = _interp.collect_iterable(&stack)?.len() as i64;
            if curindex + 1 != stack_len {
                pdb_error(_interp, self_val.0, "You can only jump within the bottom frame")?;
                return Ok(Value::none());
            }
            let Ok(target_line) = arg.0.trim().parse::<i64>() else {
                pdb_error(_interp, self_val.0, "The 'jump' command requires a line number")?;
                return Ok(Value::none());
            };
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            match _interp.assign_attr(curframe.clone(), "f_lineno", Value::int(target_line)) {
                Ok(()) => {
                    let new_entry = Value::tuple(vec![curframe, Value::int(target_line)]);
                    call_method(
                        _interp,
                        &stack,
                        "__setitem__",
                        &[Value::int(curindex), new_entry.clone()],
                    )?;
                    call_method(_interp, self_val.0, "print_stack_entry", &[new_entry])?;
                }
                Err(e) => {
                    let msg = format_pyerror(_interp, &e);
                    let stripped = msg.strip_prefix("ValueError: ").unwrap_or(&msg);
                    pdb_error(_interp, self_val.0, &format!("Jump failed: {stripped}"))?;
                }
            }
            Ok(Value::none())
        }

        fn do_run(
            #[positional_only] _self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            if !arg.0.trim().is_empty() {
                let shlex_mod = _interp.load_module("shlex")?;
                let split_fn = _interp.get_attr(&shlex_mod, "split")?;
                let new_args = call_pos(_interp, split_fn, &[Value::string(arg.0)])?;
                let sys_mod = _interp.load_module("sys")?;
                let argv = _interp.get_attr(&sys_mod, "argv")?;
                let argv_items = _interp.collect_iterable(&argv)?;
                let mut combined = Vec::new();
                if let Some(a0) = argv_items.first() {
                    combined.push(a0.clone());
                }
                combined.extend(_interp.collect_iterable(&new_args)?);
                _interp.assign_attr(sys_mod, "argv", Value::list(combined))?;
            }
            let restart_cls = pdb_attr(_interp, "Restart")?;
            let exc = call_pos(_interp, restart_cls, &[])?;
            Err(PyError::raised(exc))
        }

        fn do_quit(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "_user_requested_quit",
                Value::bool_(true),
            )?;
            call_method(_interp, self_val.0, "set_quit", &[])?;
            Ok(Value::int(1))
        }

        fn do_EOF(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            pdb_message(_interp, self_val.0, "")?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_user_requested_quit",
                Value::bool_(true),
            )?;
            call_method(_interp, self_val.0, "set_quit", &[])?;
            Ok(Value::int(1))
        }

        fn do_args(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            let co = _interp.get_attr(&curframe, "f_code")?;
            let locals = _interp.get_attr(self_val.0, "curframe_locals")?;
            let argcount = _interp.get_attr(&co, "co_argcount")?.as_int().unwrap_or(0);
            let kwonly = _interp
                .get_attr(&co, "co_kwonlyargcount")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(0);
            let flags = _interp.get_attr(&co, "co_flags")?.as_int().unwrap_or(0);
            let mut n = (argcount + kwonly).max(0) as usize;
            if flags & CO_VARARGS != 0 {
                n += 1;
            }
            if flags & CO_VARKEYWORDS != 0 {
                n += 1;
            }
            let varnames_val = _interp.get_attr(&co, "co_varnames")?;
            let varnames = _interp.collect_iterable(&varnames_val)?;
            for name_val in varnames.iter().take(n) {
                let name = name_val.as_str().unwrap_or("");
                let has = call_method(
                    _interp,
                    &locals,
                    "__contains__",
                    std::slice::from_ref(name_val),
                )?;
                if _interp.truthy_value(&has)? {
                    let val = _interp.eval_index(&locals, name_val.clone())?;
                    let r = safe_repr(_interp, &val, name);
                    pdb_message(_interp, self_val.0, &format!("{name} = {r}"))?;
                } else {
                    pdb_message(_interp, self_val.0, &format!("{name} = *** undefined ***"))?;
                }
            }
            Ok(Value::none())
        }

        fn do_retval(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            let locals = _interp.get_attr(self_val.0, "curframe_locals")?;
            let key = Value::string("__return__");
            let has = call_method(_interp, &locals, "__contains__", std::slice::from_ref(&key))?;
            if _interp.truthy_value(&has)? {
                let val = _interp.eval_index(&locals, key)?;
                let r = safe_repr(_interp, &val, "retval");
                pdb_message(_interp, self_val.0, &r)?;
            } else {
                pdb_error(_interp, self_val.0, "Not yet returned!")?;
            }
            Ok(Value::none())
        }

        fn do_p(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            if let Ok(val) = eval_in_curframe(_interp, self_val.0, arg.0.as_ref()) {
                match render_value_repr(_interp, &val) {
                    Ok(s) => pdb_message(_interp, self_val.0, &s)?,
                    Err(e) => {
                        let msg = format_pyerror(_interp, &e);
                        pdb_error(_interp, self_val.0, &msg)?;
                    }
                }
            }
            Ok(Value::none())
        }

        fn do_pp(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            if let Ok(val) = eval_in_curframe(_interp, self_val.0, arg.0.as_ref()) {
                let pprint_mod = _interp.load_module("pprint")?;
                let pformat = _interp.get_attr(&pprint_mod, "pformat")?;
                match call_pos(_interp, pformat, &[val]) {
                    Ok(formatted) => {
                        let s = _interp.render_value_as_str(&formatted)?;
                        pdb_message(_interp, self_val.0, &s)?;
                    }
                    Err(e) => {
                        let msg = format_pyerror(_interp, &e);
                        pdb_error(_interp, self_val.0, &msg)?;
                    }
                }
            }
            Ok(Value::none())
        }

        fn do_list(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "lastcmd", Value::string("list"))?;
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            let cur_lineno = _interp
                .get_attr(&curframe, "f_lineno")?
                .as_int()
                .unwrap_or(1);
            let mut last: Option<i64> = None;
            let first: i64;
            if !arg.0.is_empty() && arg.0 != "." {
                if let Some((a, b)) = arg.0.split_once(',') {
                    let Ok(f_num) = a.trim().parse::<i64>() else {
                        let r = render_value_repr(_interp, &Value::string(arg.0))?;
                        pdb_error(_interp, self_val.0, &format!("Error in argument: {r}"))?;
                        return Ok(Value::none());
                    };
                    let Ok(mut l_num) = b.trim().parse::<i64>() else {
                        let r = render_value_repr(_interp, &Value::string(arg.0))?;
                        pdb_error(_interp, self_val.0, &format!("Error in argument: {r}"))?;
                        return Ok(Value::none());
                    };
                    if l_num < f_num {
                        l_num += f_num;
                    }
                    first = f_num;
                    last = Some(l_num);
                } else {
                    let Ok(f_num) = arg.0.trim().parse::<i64>() else {
                        let r = render_value_repr(_interp, &Value::string(arg.0))?;
                        pdb_error(_interp, self_val.0, &format!("Error in argument: {r}"))?;
                        return Ok(Value::none());
                    };
                    first = (f_num - 5).max(1);
                }
            } else {
                let prev_lineno = _interp.get_attr(self_val.0, "lineno")?;
                if prev_lineno.is_none() || arg.0 == "." {
                    first = (cur_lineno - 5).max(1);
                } else {
                    first = prev_lineno.as_int().unwrap_or(0) + 1;
                }
            }
            let last = last.unwrap_or(first + 10);

            let code = _interp.get_attr(&curframe, "f_code")?;
            let mut filename = _interp
                .get_attr(&code, "co_filename")?
                .as_str()
                .unwrap_or("")
                .to_string();
            let f_globals = _interp.get_attr(&curframe, "f_globals")?;
            if filename.starts_with("<frozen")
                && let Ok(tmp) = call_method(_interp, &f_globals, "get", &[Value::string("__file__")])
                && let Some(s) = tmp.as_str()
            {
                filename = s.to_string();
            }
            let breaklist =
                call_method(_interp, self_val.0, "get_file_breaks", &[Value::string(&filename)])?;
            let lc = _interp.load_module("linecache")?;
            let getlines = _interp.get_attr(&lc, "getlines")?;
            let lines_val = call_pos(_interp, getlines, &[Value::string(&filename), f_globals])?;
            let lines = _interp.collect_iterable(&lines_val)?;
            let start_idx = (first.max(1) - 1) as usize;
            let end_idx = (last.max(0) as usize).min(lines.len());
            let slice: Vec<Value> = if start_idx < end_idx {
                lines[start_idx..end_idx].to_vec()
            } else {
                Vec::new()
            };
            call_method(
                _interp,
                self_val.0,
                "_print_lines",
                &[Value::list(slice), Value::int(first), breaklist, curframe],
            )?;
            let new_lineno = last.min(lines.len() as i64);
            _interp.assign_attr(self_val.0.clone(), "lineno", Value::int(new_lineno))?;
            if (lines.len() as i64) < last {
                pdb_message(_interp, self_val.0, "[EOF]")?;
            }
            Ok(Value::none())
        }

        fn do_longlist(
            #[positional_only] self_val: PyValueRef<'_>,
            _arg: PyStr<'_>,
        ) -> Result<Value> {
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            let code = _interp.get_attr(&curframe, "f_code")?;
            let filename = _interp.get_attr(&code, "co_filename")?;
            let breaklist = call_method(_interp, self_val.0, "get_file_breaks", &[filename])?;
            let inspect_mod = _interp.load_module("inspect")?;
            let getsourcelines = _interp.get_attr(&inspect_mod, "getsourcelines")?;
            match call_pos(_interp, getsourcelines, std::slice::from_ref(&curframe)) {
                Ok(pair) => {
                    let items = _interp.collect_iterable(&pair)?;
                    let lines_val = items.first().cloned().unwrap_or_else(|| Value::list(Vec::new()));
                    let lineno = items
                        .get(1)
                        .and_then(|v| v.as_int())
                        .unwrap_or(1)
                        .max(1);
                    call_method(
                        _interp,
                        self_val.0,
                        "_print_lines",
                        &[lines_val, Value::int(lineno), breaklist, curframe],
                    )?;
                }
                Err(e) => {
                    let msg = format_pyerror(_interp, &e);
                    let stripped = msg.strip_prefix("OSError: ").unwrap_or(&msg);
                    pdb_error(_interp, self_val.0, stripped)?;
                }
            }
            Ok(Value::none())
        }

        fn do_source(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let Ok(obj) = eval_in_curframe(_interp, self_val.0, arg.0.as_ref()) else {
                return Ok(Value::none());
            };
            let inspect_mod = _interp.load_module("inspect")?;
            let getsourcelines = _interp.get_attr(&inspect_mod, "getsourcelines")?;
            match call_pos(_interp, getsourcelines, &[obj]) {
                Ok(pair) => {
                    let items = _interp.collect_iterable(&pair)?;
                    let lines_val = items.first().cloned().unwrap_or_else(|| Value::list(Vec::new()));
                    let lineno = items
                        .get(1)
                        .and_then(|v| v.as_int())
                        .unwrap_or(1)
                        .max(1);
                    call_method(
                        _interp,
                        self_val.0,
                        "_print_lines",
                        &[lines_val, Value::int(lineno)],
                    )?;
                }
                Err(e) => {
                    let msg = format_pyerror(_interp, &e);
                    pdb_error(_interp, self_val.0, &msg)?;
                }
            }
            Ok(Value::none())
        }

        fn _print_lines(
            #[positional_only] self_val: PyValueRef<'_>,
            lines: PyValue,
            start: PyValue,
            #[default(OptionalArg::Omitted)] breaks: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] frame: OptValueRef<'_>,
        ) -> Result<Value> {
            let start_num = start.0.as_int().unwrap_or(1);
            let break_items: Vec<i64> = match breaks.as_value() {
                Some(b) => _interp
                    .collect_iterable(b)?
                    .into_iter()
                    .filter_map(|v| v.as_int())
                    .collect(),
                None => Vec::new(),
            };
            let (current_lineno, exc_lineno) = match frame.as_value().filter(|v| !v.is_none()) {
                Some(f) => {
                    let cur = _interp.get_attr(f, "f_lineno")?.as_int().unwrap_or(-1);
                    let tb_map = _interp.get_attr(self_val.0, "tb_lineno")?;
                    let exc = call_method(_interp, &tb_map, "get", &[f.clone(), Value::int(-1)])?
                        .as_int()
                        .unwrap_or(-1);
                    (cur, exc)
                }
                None => (-1, -1),
            };
            let line_list = _interp.collect_iterable(&lines.0)?;
            for (idx, line_val) in line_list.iter().enumerate() {
                let lineno = start_num + idx as i64;
                let mut s = format!("{lineno:>3}");
                if s.len() < 4 {
                    s.push(' ');
                }
                if break_items.contains(&lineno) {
                    s.push('B');
                } else {
                    s.push(' ');
                }
                if lineno == current_lineno {
                    s.push_str("->");
                } else if lineno == exc_lineno {
                    s.push_str(">>");
                }
                let text = line_val.as_str().unwrap_or("").trim_end();
                pdb_message(_interp, self_val.0, &format!("{s}\t{text}"))?;
            }
            Ok(Value::none())
        }

        fn do_whatis(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let Ok(val) = eval_in_curframe(_interp, self_val.0, arg.0.as_ref()) else {
                return Ok(Value::none());
            };
            if let Ok(func) = _interp.get_attr(&val, "__func__")
                && let Ok(code) = _interp.get_attr(&func, "__code__")
                && let Ok(name) = _interp.get_attr(&code, "co_name")
            {
                let ns = _interp.render_value_as_str(&name)?;
                pdb_message(_interp, self_val.0, &format!("Method {ns}"))?;
                return Ok(Value::none());
            }
            if let Ok(code) = _interp.get_attr(&val, "__code__")
                && let Ok(name) = _interp.get_attr(&code, "co_name")
            {
                let ns = _interp.render_value_as_str(&name)?;
                pdb_message(_interp, self_val.0, &format!("Function {ns}"))?;
                return Ok(Value::none());
            }
            let val_cls = value_class(&val);
            let type_cls = Value::py_class(type_class_singleton());
            if values_are_identical(&val_cls, &type_cls) {
                let mod_name = _interp
                    .get_attr(&val, "__module__")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let qual_name = _interp
                    .get_attr(&val, "__qualname__")
                    .or_else(|_| _interp.get_attr(&val, "__name__"))
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                pdb_message(_interp, self_val.0, &format!("Class {mod_name}.{qual_name}"))?;
                return Ok(Value::none());
            }
            let type_str = _interp.render_value_as_str(&val_cls)?;
            pdb_message(_interp, self_val.0, &type_str)?;
            Ok(Value::none())
        }

        fn do_display(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let displaying = _interp.get_attr(self_val.0, "displaying")?;
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            if arg.0.is_empty() {
                if _interp.truthy_value(&displaying)? {
                    pdb_message(_interp, self_val.0, "Currently displaying:")?;
                    let cur_map = call_method(
                        _interp,
                        &displaying,
                        "get",
                        &[curframe, Value::dict(PyDict::default())],
                    )?;
                    let items_val = call_method(_interp, &cur_map, "items", &[])?;
                    let items = _interp.collect_iterable(&items_val)?;
                    for pair in items {
                        let kv = _interp.collect_iterable(&pair)?;
                        if kv.len() == 2 {
                            let k = kv[0].as_str().unwrap_or("");
                            let r = safe_repr(_interp, &kv[1], k);
                            pdb_message(_interp, self_val.0, &format!("{k}: {r}"))?;
                        }
                    }
                } else {
                    pdb_message(_interp, self_val.0, "No expression is being displayed")?;
                }
            } else if let Some(err) = compile_error_message(_interp, arg.0.as_ref()) {
                pdb_error(_interp, self_val.0, &format!("Unable to display {}: {err}", arg.0))?;
            } else {
                let val = eval_in_frame_except(_interp, self_val.0, arg.0.as_ref(), None);
                let cur_map = call_method(
                    _interp,
                    &displaying,
                    "setdefault",
                    &[curframe, Value::dict(PyDict::default())],
                )?;
                call_method(
                    _interp,
                    &cur_map,
                    "__setitem__",
                    &[Value::string(arg.0.as_ref()), val.clone()],
                )?;
                let r = safe_repr(_interp, &val, arg.0.as_ref());
                pdb_message(_interp, self_val.0, &format!("display {}: {r}", arg.0))?;
            }
            Ok(Value::none())
        }

        fn do_undisplay(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let arg_ref: &str = arg.0.as_ref();
            let displaying = _interp.get_attr(self_val.0, "displaying")?;
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            if !arg_ref.is_empty() {
                let cur_map = call_method(
                    _interp,
                    &displaying,
                    "get",
                    &[curframe, Value::dict(PyDict::default())],
                )?;
                if call_method(_interp, &cur_map, "__delitem__", &[Value::string(arg_ref)]).is_err() {
                    pdb_error(_interp, self_val.0, &format!("not displaying {arg_ref}"))?;
                }
            } else {
                let _ = call_method(_interp, &displaying, "pop", &[curframe, Value::none()]);
            }
            Ok(Value::none())
        }

        fn do_alias(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let aliases = _interp.get_attr(self_val.0, "aliases")?;
            let parts: Vec<&str> = arg.0.split_whitespace().collect();
            if parts.is_empty() {
                let keys_val = call_method(_interp, &aliases, "keys", &[])?;
                let mut keys: Vec<String> = _interp
                    .collect_iterable(&keys_val)?
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                keys.sort();
                for k in keys {
                    let v = _interp.eval_index(&aliases, Value::string(&k))?;
                    let vs = _interp.render_value_as_str(&v)?;
                    pdb_message(_interp, self_val.0, &format!("{k} = {vs}"))?;
                }
            } else if parts.len() == 1 {
                let has = call_method(
                    _interp,
                    &aliases,
                    "__contains__",
                    &[Value::string(parts[0])],
                )?;
                if _interp.truthy_value(&has)? {
                    let v = _interp.eval_index(&aliases, Value::string(parts[0]))?;
                    let vs = _interp.render_value_as_str(&v)?;
                    pdb_message(_interp, self_val.0, &format!("{} = {vs}", parts[0]))?;
                } else {
                    pdb_error(_interp, self_val.0, &format!("Unknown alias '{}'", parts[0]))?;
                }
            } else {
                call_method(
                    _interp,
                    &aliases,
                    "__setitem__",
                    &[Value::string(parts[0]), Value::string(parts[1..].join(" "))],
                )?;
            }
            Ok(Value::none())
        }

        fn do_unalias(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let parts: Vec<&str> = arg.0.split_whitespace().collect();
            if let Some(first) = parts.first() {
                let aliases = _interp.get_attr(self_val.0, "aliases")?;
                let _ = call_method(_interp, &aliases, "pop", &[Value::string(*first), Value::none()]);
            }
            Ok(Value::none())
        }

        fn print_stack_trace(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stack = _interp.get_attr(self_val.0, "stack")?;
            let entries = _interp.collect_iterable(&stack)?;
            for entry in entries {
                call_method(_interp, self_val.0, "print_stack_entry", &[entry])?;
            }
            Ok(Value::none())
        }

        fn print_stack_entry(
            #[positional_only] self_val: PyValueRef<'_>,
            frame_lineno: PyValue,
            #[default(OptionalArg::Omitted)] prompt_prefix: OptValueRef<'_>,
        ) -> Result<Value> {
            let prefix_str = match prompt_prefix.as_value() {
                Some(v) => v.clone(),
                None => pdb_attr(_interp, "line_prefix")
                    .unwrap_or_else(|_| Value::string(DEFAULT_LINE_PREFIX)),
            };
            let pair = _interp.collect_iterable(&frame_lineno.0)?;
            let frame = pair.first().cloned().unwrap_or_else(Value::none);
            let curframe = _interp.get_attr(self_val.0, "curframe")?;
            let marker = if values_are_identical(&frame, &curframe) {
                "> "
            } else {
                "  "
            };
            let formatted = call_method(
                _interp,
                self_val.0,
                "format_stack_entry",
                &[frame_lineno.0, prefix_str],
            )?;
            let fs = _interp.render_value_as_str(&formatted)?;
            pdb_message(_interp, self_val.0, &format!("{marker}{fs}"))?;
            Ok(Value::none())
        }

        fn do_help(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            if arg.0.is_empty() {
                let names_val = call_method(_interp, self_val.0, "get_names", &[])?;
                let mut names: Vec<String> = _interp
                    .collect_iterable(&names_val)?
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                names.sort();
                names.dedup();
                let mut cmds_doc: Vec<Value> = Vec::new();
                let mut topics: Vec<String> = Vec::new();
                for n in &names {
                    if let Some(t) = n.strip_prefix("help_") {
                        topics.push(t.to_string());
                    }
                }
                for n in &names {
                    if let Some(c) = n.strip_prefix("do_")
                        && (command_doc(c).is_some() || topics.iter().any(|t| t == c))
                    {
                        cmds_doc.push(Value::string(c));
                        topics.retain(|t| t != c);
                    }
                }
                let doc_leader = _interp
                    .get_attr(self_val.0, "doc_leader")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let doc_header = _interp
                    .get_attr(self_val.0, "doc_header")
                    .unwrap_or_else(|_| Value::string("Documented commands (type help <topic>):"));
                let misc_header = _interp
                    .get_attr(self_val.0, "misc_header")
                    .unwrap_or_else(|_| Value::string("Miscellaneous help topics:"));
                let stdout = _interp.get_attr(self_val.0, "stdout")?;
                call_method(
                    _interp,
                    &stdout,
                    "write",
                    &[Value::string(format!("{doc_leader}\n"))],
                )?;
                call_method(
                    _interp,
                    self_val.0,
                    "print_topics",
                    &[doc_header, Value::list(cmds_doc), Value::int(15), Value::int(80)],
                )?;
                let topic_vals: Vec<Value> = topics.into_iter().map(Value::string).collect();
                call_method(
                    _interp,
                    self_val.0,
                    "print_topics",
                    &[misc_header, Value::list(topic_vals), Value::int(15), Value::int(80)],
                )?;
                return Ok(Value::none());
            }

            if let Ok(topic_fn) = _interp.get_attr(self_val.0, &format!("help_{}", arg.0)) {
                return call_pos(_interp, topic_fn, &[]);
            }
            let prompt = _interp
                .get_attr(self_val.0, "prompt")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "(Pdb) ".to_string());
            if let Some(doc) = command_doc(arg.0.as_ref()) {
                let msg = format_help_from_doc(&prompt, doc);
                pdb_message(_interp, self_val.0, &msg)?;
                return Ok(Value::none());
            }
            if let Ok(cmd_fn) = _interp.get_attr(self_val.0, &format!("do_{}", arg.0)) {
                let doc = _interp.get_attr(&cmd_fn, "__doc__").unwrap_or_else(|_| Value::none());
                if let Some(s) = doc.as_str() {
                    let msg = format_help_from_doc(&prompt, s);
                    pdb_message(_interp, self_val.0, &msg)?;
                    return Ok(Value::none());
                }
                let r = render_value_repr(_interp, &Value::string(arg.0))?;
                pdb_error(_interp, self_val.0, &format!("No help for {r}; __doc__ string missing"))?;
                return Ok(Value::none());
            }
            let r = render_value_repr(_interp, &Value::string(arg.0))?;
            pdb_error(_interp, self_val.0, &format!("No help for {r}"))?;
            Ok(Value::none())
        }

        fn help_exec(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            pdb_message(
                _interp,
                self_val.0,
                "(!) statement\n\nExecute the (one-line) statement in the context of the current\nstack frame.",
            )?;
            Ok(Value::none())
        }

        fn help_pdb(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            let help_fn = pdb_attr(_interp, "help")?;
            call_pos(_interp, help_fn, &[])
        }
    }

    // ── Top-level convenience API ────────────────────────────────────────────

    fn run(
        statement: PyValue,
        #[default(OptionalArg::Omitted)] globals: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] locals: OptValueRef<'_>,
    ) -> Result<Value> {
        let pdb_cls = pdb_attr(_interp, "Pdb")?;
        let p = call_pos(_interp, pdb_cls, &[])?;
        let g = globals.as_value().cloned().unwrap_or_else(Value::none);
        let l = locals.as_value().cloned().unwrap_or_else(Value::none);
        call_method(_interp, &p, "run", &[statement.0, g, l])
    }

    fn runeval(
        expression: PyValue,
        #[default(OptionalArg::Omitted)] globals: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] locals: OptValueRef<'_>,
    ) -> Result<Value> {
        let pdb_cls = pdb_attr(_interp, "Pdb")?;
        let p = call_pos(_interp, pdb_cls, &[])?;
        let g = globals.as_value().cloned().unwrap_or_else(Value::none);
        let l = locals.as_value().cloned().unwrap_or_else(Value::none);
        call_method(_interp, &p, "runeval", &[expression.0, g, l])
    }

    fn runctx(statement: PyValue, globals: PyValue, locals: PyValue) -> Result<Value> {
        let run_fn = pdb_attr(_interp, "run")?;
        call_pos(_interp, run_fn, &[statement.0, globals.0, locals.0])
    }

    fn runcall(
        #[varargs] args: &[Value],
        #[varkwargs] kwds: IndexMap<String, Value>,
    ) -> Result<Value> {
        let pdb_cls = pdb_attr(_interp, "Pdb")?;
        let p = call_pos(_interp, pdb_cls, &[])?;
        let runcall_m = _interp.get_attr(&p, "runcall")?;
        let mut expanded: Vec<ExpandedCallArg> = args
            .iter()
            .cloned()
            .map(|value| ExpandedCallArg { name: None, value })
            .collect();
        for (k, v) in kwds {
            expanded.push(ExpandedCallArg {
                name: Some(k),
                value: v,
            });
        }
        _interp.call_function_expanded(runcall_m, &expanded)
    }

    fn set_trace(
        #[varargs] _args: &[Value],
        #[default(OptionalArg::Omitted)] header: OptValueRef<'_>,
    ) -> Result<Value> {
        if !_args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                format!("set_trace() takes 0 positional arguments but {} were given", _args.len()),
            ));
        }
        let pdb_cls = pdb_attr(_interp, "Pdb")?;
        let p = call_pos(_interp, pdb_cls, &[])?;
        if let Some(hdr) = header.as_value().filter(|v| !v.is_none()) {
            call_method(_interp, &p, "message", std::slice::from_ref(hdr))?;
        }
        // Native functions push no Python frame, so depth 0 is the caller's frame.
        let frame = _interp.build_frame_object(0, pyrust_core::get_current_vm_line() as i64);
        call_method(_interp, &p, "set_trace", &[frame])?;
        Ok(Value::none())
    }

    fn post_mortem(
        #[default(OptionalArg::Omitted)] t: OptValueRef<'_>,
    ) -> Result<Value> {
        let mut tb = t.as_value().cloned().unwrap_or_else(Value::none);
        if tb.is_none() {
            let sys_mod = _interp.load_module("sys")?;
            let exc_fn = _interp.get_attr(&sys_mod, "exception")?;
            let exc = call_pos(_interp, exc_fn, &[])?;
            if !exc.is_none() {
                tb = _interp.get_attr(&exc, "__traceback__")?;
            }
        }
        if tb.is_none() {
            return Err(PyError::named(
                "ValueError",
                "A valid traceback must be passed if no exception is being handled",
            ));
        }
        let pdb_cls = pdb_attr(_interp, "Pdb")?;
        let p = call_pos(_interp, pdb_cls, &[])?;
        call_method(_interp, &p, "reset", &[])?;
        call_method(_interp, &p, "interaction", &[Value::none(), tb])?;
        Ok(Value::none())
    }

    fn pm() -> Result<Value> {
        let sys_mod = _interp.load_module("sys")?;
        let tb = if let Ok(last_exc) = _interp.get_attr(&sys_mod, "last_exc") {
            _interp.get_attr(&last_exc, "__traceback__")?
        } else {
            _interp.get_attr(&sys_mod, "last_traceback")?
        };
        let pm_fn = pdb_attr(_interp, "post_mortem")?;
        call_pos(_interp, pm_fn, &[tb])
    }

    fn find_function(funcname: PyStr<'_>, filename: PyStr<'_>) -> Result<Value> {
        match find_function_impl(_interp, funcname.0.as_ref(), filename.0.as_ref())? {
            Some((n, f, l)) => Ok(Value::tuple(vec![
                Value::string(n),
                Value::string(f),
                Value::int(l),
            ])),
            None => Ok(Value::none()),
        }
    }

    fn lasti2lineno(code: PyValue, lasti: PyValue) -> Result<Value> {
        let idx = lasti.0.as_int().unwrap_or(0);
        Ok(Value::int(lasti2lineno_impl(_interp, &code.0, idx)?))
    }

    fn help() -> Result<Value> {
        let sys_mod = _interp.load_module("sys")?;
        let stdout = _interp.get_attr(&sys_mod, "stdout")?;
        let doc = "The Python Debugger (pdb) — use 'help <command>' inside (Pdb) for command details.\n";
        call_method(_interp, &stdout, "write", &[Value::string(doc)])?;
        Ok(Value::none())
    }
}
