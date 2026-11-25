// `bdb` module — 100% native Rust implementation of Python 3.12's `bdb`
// (Debugger basics) module (issue #3247).
//
// Built directly on PyRust's live `sys.settrace` hooks using `pyrust_module!`,
// `exception`, `class`, `#[staticmethod]`, `#[varargs]`, and `#[varkwargs]`.
//
// Reference: <https://docs.python.org/3/library/bdb.html>

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr, values_are_identical};
use crate::value::{PyDict, Value};

// inspect.CO_GENERATOR (0x20) | CO_COROUTINE (0x80) | CO_ASYNC_GENERATOR (0x200)
const GENERATOR_AND_COROUTINE_FLAGS: i64 = 0x20 | 0x80 | 0x200;

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
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

fn bdb_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("bdb")?;
    interp.get_attr(&m, name)
}

fn raise_bdb_quit(interp: &mut Interpreter) -> PyError {
    match bdb_attr(interp, "BdbQuit").and_then(|cls| call_pos(interp, cls, &[])) {
        Ok(inst) => PyError::raised(inst),
        Err(e) => e,
    }
}

fn is_bdb_quit(interp: &mut Interpreter, err: &PyError) -> bool {
    let Ok(exc_val) = interp.materialize_pyerror(err.clone()) else {
        return false;
    };
    let Ok(cls) = bdb_attr(interp, "BdbQuit") else {
        return false;
    };
    call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[exc_val, cls],
    )
    .and_then(|v| interp.truthy_value(&v))
    .unwrap_or(false)
}

fn is_instance_of_name(interp: &mut Interpreter, obj: &Value, exc_name: &str) -> bool {
    let Some(cls) = crate::interpreter::lookup_exc_class(exc_name) else {
        return false;
    };
    call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), Value::py_class(cls)],
    )
    .and_then(|v| interp.truthy_value(&v))
    .unwrap_or(false)
}

fn set_sys_trace(interp: &mut Interpreter, func: Value) -> Result<()> {
    let sys_mod = interp.load_module("sys")?;
    let settrace = interp.get_attr(&sys_mod, "settrace")?;
    call_pos(interp, settrace, &[func])?;
    Ok(())
}

fn default_main_dict(interp: &mut Interpreter) -> Result<Value> {
    let main_mod = interp.load_module("__main__")?;
    interp.get_attr(&main_mod, "__dict__")
}

fn write_line_to_stream(interp: &mut Interpreter, stream: &Value, line: &str) -> Result<()> {
    call_method(
        interp,
        stream,
        "write",
        &[Value::string(format!("{line}\n"))],
    )?;
    Ok(())
}

fn dict_contains_key(interp: &mut Interpreter, dict: &Value, key: Value) -> Result<bool> {
    let res = call_method(interp, dict, "__contains__", &[key])?;
    interp.truthy_value(&res)
}

fn dict_get_item(interp: &mut Interpreter, dict: &Value, key: Value) -> Result<Value> {
    interp.eval_index(dict, key)
}

fn dict_set_item(interp: &mut Interpreter, dict: &Value, key: Value, val: Value) -> Result<()> {
    call_method(interp, dict, "__setitem__", &[key, val])?;
    Ok(())
}

fn dict_del_item(interp: &mut Interpreter, dict: &Value, key: Value) -> Result<()> {
    call_method(interp, dict, "__delitem__", &[key])?;
    Ok(())
}

fn list_contains(interp: &mut Interpreter, lst: &Value, item: &Value) -> Result<bool> {
    let res = call_method(interp, lst, "__contains__", std::slice::from_ref(item))?;
    interp.truthy_value(&res)
}

fn frame_code_flags(interp: &mut Interpreter, frame: &Value) -> Result<i64> {
    let code = interp.get_attr(frame, "f_code")?;
    let flags = interp.get_attr(&code, "co_flags")?;
    Ok(flags.as_int().unwrap_or(0))
}

fn frame_lineno(interp: &mut Interpreter, frame: &Value) -> Result<i64> {
    let lineno = interp.get_attr(frame, "f_lineno")?;
    Ok(lineno.as_int().unwrap_or(0))
}

fn checkfuncname_impl(interp: &mut Interpreter, b: &Value, frame: &Value) -> Result<bool> {
    let funcname = interp.get_attr(b, "funcname")?;
    if !interp.truthy_value(&funcname)? {
        let b_line = interp.get_attr(b, "line")?;
        let f_line = interp.get_attr(frame, "f_lineno")?;
        let eq = interp.eval_binary(b_line, crate::ast::BinaryOp::Eq, f_line)?;
        return interp.truthy_value(&eq);
    }
    let f_code = interp.get_attr(frame, "f_code")?;
    let co_name = interp.get_attr(&f_code, "co_name")?;
    let name_eq = interp.eval_binary(co_name, crate::ast::BinaryOp::Eq, funcname)?;
    if !interp.truthy_value(&name_eq)? {
        return Ok(false);
    }
    let first_exec = interp.get_attr(b, "func_first_executable_line")?;
    let f_lineno = interp.get_attr(frame, "f_lineno")?;
    if !interp.truthy_value(&first_exec)? {
        interp.assign_attr(b.clone(), "func_first_executable_line", f_lineno.clone())?;
        return Ok(true);
    }
    let eq = interp.eval_binary(first_exec, crate::ast::BinaryOp::Eq, f_lineno)?;
    interp.truthy_value(&eq)
}

fn effective_impl(
    interp: &mut Interpreter,
    file: Value,
    line: Value,
    frame: &Value,
) -> Result<(Value, Value)> {
    let bp_cls = bdb_attr(interp, "Breakpoint")?;
    let bplist = interp.get_attr(&bp_cls, "bplist")?;
    let key = Value::tuple(vec![file, line]);
    let possibles_val = dict_get_item(interp, &bplist, key)?;
    let possibles = interp.collect_iterable(&possibles_val)?;

    for b in possibles {
        let enabled = interp.get_attr(&b, "enabled")?;
        if !interp.truthy_value(&enabled)? {
            continue;
        }
        if !checkfuncname_impl(interp, &b, frame)? {
            continue;
        }
        let hits = interp.get_attr(&b, "hits")?.as_int().unwrap_or(0);
        interp.assign_attr(b.clone(), "hits", Value::int(hits + 1))?;

        let cond = interp.get_attr(&b, "cond")?;
        if !interp.truthy_value(&cond)? {
            let ignore = interp.get_attr(&b, "ignore")?.as_int().unwrap_or(0);
            if ignore > 0 {
                interp.assign_attr(b.clone(), "ignore", Value::int(ignore - 1))?;
                continue;
            }
            return Ok((b, Value::bool_(true)));
        }
        let f_globals = interp.get_attr(frame, "f_globals")?;
        let f_locals = interp.get_attr(frame, "f_locals")?;
        match call_pos(
            interp,
            Value::builtin_function("eval"),
            &[cond, f_globals, f_locals],
        ) {
            Ok(val) => {
                if interp.truthy_value(&val)? {
                    let ignore = interp.get_attr(&b, "ignore")?.as_int().unwrap_or(0);
                    if ignore > 0 {
                        interp.assign_attr(b.clone(), "ignore", Value::int(ignore - 1))?;
                    } else {
                        return Ok((b, Value::bool_(true)));
                    }
                }
            }
            Err(_) => {
                return Ok((b, Value::bool_(false)));
            }
        }
    }
    Ok((Value::none(), Value::none()))
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("BdbQuit"),
            Value::string("Bdb"),
            Value::string("Breakpoint"),
        ]),
        "GENERATOR_AND_COROUTINE_FLAGS" => Value::int(GENERATOR_AND_COROUTINE_FLAGS),
    }

    /// Exception to give up completely.
    exception BdbQuit;

    class _EnterFrameCM {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            bdb_obj: PyValue,
            frame: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_bdb", bdb_obj.0)?;
            _interp.assign_attr(self_val.0.clone(), "_frame", frame.0)?;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let bdb_obj = _interp.get_attr(self_val.0, "_bdb")?;
            let frame = _interp.get_attr(self_val.0, "_frame")?;
            _interp.assign_attr(bdb_obj, "enterframe", frame)?;
            Ok(Value::none())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _exc: &[Value],
        ) -> Result<Value> {
            let bdb_obj = _interp.get_attr(self_val.0, "_bdb")?;
            _interp.assign_attr(bdb_obj, "enterframe", Value::none())?;
            Ok(Value::bool_(false))
        }
    }

    class Breakpoint {
        constants {
            "next" => Value::int(1),
            "bplist" => Value::dict(PyDict::default()),
            "bpbynumber" => Value::list(vec![Value::none()]),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            file: PyValue,
            line: PyValue,
            #[default(OptionalArg::Omitted)] temporary: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] cond: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] funcname: OptValueRef<'_>,
        ) -> Result<Value> {
            let temp_val = temporary
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::bool_(false));
            let cond_val = cond.as_value().cloned().unwrap_or_else(Value::none);
            let func_val = funcname.as_value().cloned().unwrap_or_else(Value::none);

            _interp.assign_attr(self_val.0.clone(), "funcname", func_val)?;
            _interp.assign_attr(self_val.0.clone(), "func_first_executable_line", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "file", file.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "line", line.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "temporary", temp_val)?;
            _interp.assign_attr(self_val.0.clone(), "cond", cond_val)?;
            _interp.assign_attr(self_val.0.clone(), "enabled", Value::bool_(true))?;
            _interp.assign_attr(self_val.0.clone(), "ignore", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "hits", Value::int(0))?;

            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            let next_num = _interp.get_attr(&bp_cls, "next")?.as_int().unwrap_or(1);
            _interp.assign_attr(self_val.0.clone(), "number", Value::int(next_num))?;
            _interp.assign_attr(bp_cls, "next", Value::int(next_num + 1))?;

            let bpbynumber = _interp.get_attr(self_val.0, "bpbynumber")?;
            call_method(
                _interp,
                &bpbynumber,
                "append",
                std::slice::from_ref(self_val.0),
            )?;

            let bplist = _interp.get_attr(self_val.0, "bplist")?;
            let key = Value::tuple(vec![file.0, line.0]);
            if dict_contains_key(_interp, &bplist, key.clone())? {
                let entry = dict_get_item(_interp, &bplist, key)?;
                call_method(_interp, &entry, "append", std::slice::from_ref(self_val.0))?;
            } else {
                dict_set_item(
                    _interp,
                    &bplist,
                    key,
                    Value::list(vec![self_val.0.clone()]),
                )?;
            }
            Ok(Value::none())
        }

        #[staticmethod]
        fn clearBreakpoints() -> Result<Value> {
            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            _interp.assign_attr(bp_cls.clone(), "next", Value::int(1))?;
            _interp.assign_attr(bp_cls.clone(), "bplist", Value::dict(PyDict::default()))?;
            _interp.assign_attr(bp_cls, "bpbynumber", Value::list(vec![Value::none()]))?;
            Ok(Value::none())
        }

        fn deleteMe(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let file = _interp.get_attr(self_val.0, "file")?;
            let line = _interp.get_attr(self_val.0, "line")?;
            let number = _interp.get_attr(self_val.0, "number")?;
            let bpbynumber = _interp.get_attr(self_val.0, "bpbynumber")?;
            call_method(&mut *_interp, &bpbynumber, "__setitem__", &[number, Value::none()])?;

            let bplist = _interp.get_attr(self_val.0, "bplist")?;
            let index = Value::tuple(vec![file, line]);
            let entry = dict_get_item(_interp, &bplist, index.clone())?;
            call_method(_interp, &entry, "remove", std::slice::from_ref(self_val.0))?;
            if !_interp.truthy_value(&entry)? {
                dict_del_item(_interp, &bplist, index)?;
            }
            Ok(Value::none())
        }

        fn enable(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "enabled", Value::bool_(true))?;
            Ok(Value::none())
        }

        fn disable(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "enabled", Value::bool_(false))?;
            Ok(Value::none())
        }

        fn bpprint(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] out: OptValueRef<'_>,
        ) -> Result<Value> {
            let stream = match out.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => {
                    let sys_mod = _interp.load_module("sys")?;
                    _interp.get_attr(&sys_mod, "stdout")?
                }
            };
            let formatted = call_method(_interp, self_val.0, "bpformat", &[])?;
            let text = _interp.render_value_as_str(&formatted)?;
            write_line_to_stream(_interp, &stream, &text)?;
            Ok(Value::none())
        }

        fn bpformat(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let temporary = _interp.get_attr(self_val.0, "temporary")?;
            let enabled = _interp.get_attr(self_val.0, "enabled")?;
            let number = _interp.get_attr(self_val.0, "number")?.as_int().unwrap_or(0);
            let file_val = _interp.get_attr(self_val.0, "file")?;
            let file = _interp.render_value_as_str(&file_val)?;
            let line = _interp.get_attr(self_val.0, "line")?.as_int().unwrap_or(0);
            let cond = _interp.get_attr(self_val.0, "cond")?;
            let ignore = _interp.get_attr(self_val.0, "ignore")?.as_int().unwrap_or(0);
            let hits = _interp.get_attr(self_val.0, "hits")?.as_int().unwrap_or(0);

            let disp = if _interp.truthy_value(&temporary)? {
                "del  "
            } else {
                "keep "
            };
            let enb = if _interp.truthy_value(&enabled)? {
                "yes  "
            } else {
                "no   "
            };
            let mut ret = format!("{number:<4}breakpoint   {disp}{enb} at {file}:{line}");
            if _interp.truthy_value(&cond)? {
                let cond_s = _interp.render_value_as_str(&cond)?;
                ret.push_str(&format!("\n\tstop only if {cond_s}"));
            }
            if ignore != 0 {
                ret.push_str(&format!("\n\tignore next {ignore} hits"));
            }
            if hits != 0 {
                let ss = if hits > 1 { "s" } else { "" };
                ret.push_str(&format!("\n\tbreakpoint already hit {hits} time{ss}"));
            }
            Ok(Value::string(ret))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let number = _interp.get_attr(self_val.0, "number")?;
            let file = _interp.get_attr(self_val.0, "file")?;
            let line = _interp.get_attr(self_val.0, "line")?;
            let num_s = _interp.render_value_as_str(&number)?;
            let file_s = _interp.render_value_as_str(&file)?;
            let line_s = _interp.render_value_as_str(&line)?;
            Ok(Value::string(format!(
                "breakpoint {num_s} at {file_s}:{line_s}"
            )))
        }
    }

    class Bdb {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] skip: OptValueRef<'_>,
        ) -> Result<Value> {
            let skip_val = match skip.as_value() {
                Some(v) if _interp.truthy_value(v)? => {
                    let set_cls = crate::interpreter::primitive_class_by_name("set")
                        .map(Value::py_class)
                        .unwrap_or_else(|| Value::builtin_function("set"));
                    call_pos(_interp, set_cls, std::slice::from_ref(v))?
                }
                _ => Value::none(),
            };
            _interp.assign_attr(self_val.0.clone(), "skip", skip_val)?;
            _interp.assign_attr(self_val.0.clone(), "breaks", Value::dict(PyDict::default()))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "fncache",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "frame_returning", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "enterframe", Value::none())?;
            call_method(_interp, self_val.0, "_load_breaks", &[])?;
            Ok(Value::none())
        }

        fn canonic(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
        ) -> Result<Value> {
            let fname: &str = filename.0.as_ref();
            if fname.len() >= 2 && fname.starts_with('<') && fname.ends_with('>') {
                return Ok(Value::string(fname));
            }
            let fncache = _interp.get_attr(self_val.0, "fncache")?;
            let key = Value::string(fname);
            if dict_contains_key(_interp, &fncache, key.clone())? {
                let cached = dict_get_item(_interp, &fncache, key.clone())?;
                if _interp.truthy_value(&cached)? {
                    return Ok(cached);
                }
            }
            let os_path = _interp.load_module("os.path")?;
            let abspath = _interp.get_attr(&os_path, "abspath")?;
            let normcase = _interp.get_attr(&os_path, "normcase")?;
            let abs_val = call_pos(_interp, abspath, std::slice::from_ref(&key))?;
            let canonic = call_pos(_interp, normcase, &[abs_val])?;
            dict_set_item(_interp, &fncache, key, canonic.clone())?;
            Ok(canonic)
        }

        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            if let Ok(lc) = _interp.load_module("linecache")
                && let Ok(cc) = _interp.get_attr(&lc, "checkcache")
            {
                let _ = call_pos(_interp, cc, &[]);
            }
            _interp.assign_attr(self_val.0.clone(), "botframe", Value::none())?;
            call_method(
                _interp,
                self_val.0,
                "_set_stopinfo",
                &[Value::none(), Value::none()],
            )?;
            Ok(Value::none())
        }

        fn set_enterframe(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            let cm_cls = bdb_attr(_interp, "_EnterFrameCM")?;
            call_pos(_interp, cm_cls, &[self_val.0.clone(), frame.0])
        }

        fn trace_dispatch(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            event: PyStr<'_>,
            arg: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "enterframe", frame.0.clone())?;
            let res = (|| -> Result<Value> {
                let quitting = _interp
                    .get_attr(self_val.0, "quitting")
                    .unwrap_or_else(|_| Value::bool_(false));
                if _interp.truthy_value(&quitting)? {
                    return Ok(Value::none());
                }
                match event.0.as_ref() {
                    "line" => call_method(
                        _interp,
                        self_val.0,
                        "dispatch_line",
                        std::slice::from_ref(&frame.0),
                    ),
                    "call" => call_method(
                        _interp,
                        self_val.0,
                        "dispatch_call",
                        &[frame.0.clone(), arg.0],
                    ),
                    "return" => call_method(
                        _interp,
                        self_val.0,
                        "dispatch_return",
                        &[frame.0.clone(), arg.0],
                    ),
                    "exception" => call_method(
                        _interp,
                        self_val.0,
                        "dispatch_exception",
                        &[frame.0.clone(), arg.0],
                    ),
                    "c_call" | "c_exception" | "c_return" => {
                        _interp.get_attr(self_val.0, "trace_dispatch")
                    }
                    other => {
                        let msg = format!(
                            "bdb.Bdb.dispatch: unknown debugging event: {}",
                            render_value_repr(_interp, &Value::string(other))?
                        );
                        let sys_mod = _interp.load_module("sys")?;
                        let stdout = _interp.get_attr(&sys_mod, "stdout")?;
                        write_line_to_stream(_interp, &stdout, &msg)?;
                        _interp.get_attr(self_val.0, "trace_dispatch")
                    }
                }
            })();
            let _ = _interp.assign_attr(self_val.0.clone(), "enterframe", Value::none());
            res
        }

        fn dispatch_line(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            let stop = call_method(
                _interp,
                self_val.0,
                "stop_here",
                std::slice::from_ref(&frame.0),
            )?;
            let should_stop = if _interp.truthy_value(&stop)? {
                true
            } else {
                let brk = call_method(
                    _interp,
                    self_val.0,
                    "break_here",
                    std::slice::from_ref(&frame.0),
                )?;
                _interp.truthy_value(&brk)?
            };
            if should_stop {
                call_method(
                    _interp,
                    self_val.0,
                    "user_line",
                    std::slice::from_ref(&frame.0),
                )?;
                let quitting = _interp.get_attr(self_val.0, "quitting")?;
                if _interp.truthy_value(&quitting)? {
                    return Err(raise_bdb_quit(_interp));
                }
            }
            _interp.get_attr(self_val.0, "trace_dispatch")
        }

        fn dispatch_call(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            arg: PyValue,
        ) -> Result<Value> {
            let botframe = _interp.get_attr(self_val.0, "botframe")?;
            if botframe.is_none() {
                let f_back = _interp.get_attr(&frame.0, "f_back")?;
                _interp.assign_attr(self_val.0.clone(), "botframe", f_back)?;
                return _interp.get_attr(self_val.0, "trace_dispatch");
            }
            let stop = call_method(
                _interp,
                self_val.0,
                "stop_here",
                std::slice::from_ref(&frame.0),
            )?;
            let should_trace = if _interp.truthy_value(&stop)? {
                true
            } else {
                let brk = call_method(
                    _interp,
                    self_val.0,
                    "break_anywhere",
                    std::slice::from_ref(&frame.0),
                )?;
                _interp.truthy_value(&brk)?
            };
            if !should_trace {
                return Ok(Value::none());
            }
            let stopframe = _interp.get_attr(self_val.0, "stopframe")?;
            if _interp.truthy_value(&stopframe)?
                && (frame_code_flags(_interp, &frame.0)? & GENERATOR_AND_COROUTINE_FLAGS) != 0
            {
                return _interp.get_attr(self_val.0, "trace_dispatch");
            }
            call_method(_interp, self_val.0, "user_call", &[frame.0, arg.0])?;
            let quitting = _interp.get_attr(self_val.0, "quitting")?;
            if _interp.truthy_value(&quitting)? {
                return Err(raise_bdb_quit(_interp));
            }
            _interp.get_attr(self_val.0, "trace_dispatch")
        }

        fn dispatch_return(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            arg: PyValue,
        ) -> Result<Value> {
            let stop = call_method(
                _interp,
                self_val.0,
                "stop_here",
                std::slice::from_ref(&frame.0),
            )?;
            let returnframe = _interp.get_attr(self_val.0, "returnframe")?;
            let matches_ret = _interp.truthy_value(&stop)?
                || values_are_identical(&frame.0, &returnframe)
                || _interp
                    .eval_binary(frame.0.clone(), crate::ast::BinaryOp::Eq, returnframe)
                    .and_then(|v| _interp.truthy_value(&v))
                    .unwrap_or(false);

            if matches_ret {
                let stopframe = _interp.get_attr(self_val.0, "stopframe")?;
                if _interp.truthy_value(&stopframe)?
                    && (frame_code_flags(_interp, &frame.0)? & GENERATOR_AND_COROUTINE_FLAGS) != 0
                {
                    return _interp.get_attr(self_val.0, "trace_dispatch");
                }
                _interp.assign_attr(self_val.0.clone(), "frame_returning", frame.0.clone())?;
                let user_res =
                    call_method(_interp, self_val.0, "user_return", &[frame.0.clone(), arg.0]);
                let _ = _interp.assign_attr(self_val.0.clone(), "frame_returning", Value::none());
                user_res?;

                let quitting = _interp.get_attr(self_val.0, "quitting")?;
                if _interp.truthy_value(&quitting)? {
                    return Err(raise_bdb_quit(_interp));
                }
                let stopframe_now = _interp.get_attr(self_val.0, "stopframe")?;
                let stoplineno = _interp
                    .get_attr(self_val.0, "stoplineno")?
                    .as_int()
                    .unwrap_or(0);
                if values_are_identical(&stopframe_now, &frame.0) && stoplineno != -1 {
                    call_method(
                        _interp,
                        self_val.0,
                        "_set_stopinfo",
                        &[Value::none(), Value::none()],
                    )?;
                }
                let stoplineno_after = _interp
                    .get_attr(self_val.0, "stoplineno")?
                    .as_int()
                    .unwrap_or(0);
                if stoplineno_after != -1 {
                    call_method(
                        _interp,
                        self_val.0,
                        "_set_caller_tracefunc",
                        std::slice::from_ref(&frame.0),
                    )?;
                }
            }
            _interp.get_attr(self_val.0, "trace_dispatch")
        }

        fn dispatch_exception(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            arg: PyValue,
        ) -> Result<Value> {
            let stop = call_method(
                _interp,
                self_val.0,
                "stop_here",
                std::slice::from_ref(&frame.0),
            )?;
            let exc_type = arg
                .0
                .as_tuple()
                .and_then(|t| t.first().cloned())
                .unwrap_or_else(Value::none);
            let exc_tb = arg
                .0
                .as_tuple()
                .and_then(|t| t.get(2).cloned())
                .unwrap_or_else(Value::none);
            let stop_iter_cls = crate::interpreter::lookup_exc_class("StopIteration");
            let gen_exit_cls = crate::interpreter::lookup_exc_class("GeneratorExit");
            let is_stop_iter = stop_iter_cls
                .as_ref()
                .is_some_and(|c| values_are_identical(&exc_type, &Value::py_class(c.clone())));
            let is_gen_exit = gen_exit_cls
                .as_ref()
                .is_some_and(|c| values_are_identical(&exc_type, &Value::py_class(c.clone())));

            if _interp.truthy_value(&stop)? {
                let is_gen =
                    (frame_code_flags(_interp, &frame.0)? & GENERATOR_AND_COROUTINE_FLAGS) != 0;
                if !(is_gen && is_stop_iter && exc_tb.is_none()) {
                    call_method(_interp, self_val.0, "user_exception", &[frame.0, arg.0])?;
                    let quitting = _interp.get_attr(self_val.0, "quitting")?;
                    if _interp.truthy_value(&quitting)? {
                        return Err(raise_bdb_quit(_interp));
                    }
                }
            } else {
                let stopframe = _interp.get_attr(self_val.0, "stopframe")?;
                if _interp.truthy_value(&stopframe)?
                    && !values_are_identical(&frame.0, &stopframe)
                    && (frame_code_flags(_interp, &stopframe)? & GENERATOR_AND_COROUTINE_FLAGS) != 0
                    && (is_stop_iter || is_gen_exit)
                {
                    call_method(_interp, self_val.0, "user_exception", &[frame.0, arg.0])?;
                    let quitting = _interp.get_attr(self_val.0, "quitting")?;
                    if _interp.truthy_value(&quitting)? {
                        return Err(raise_bdb_quit(_interp));
                    }
                }
            }
            _interp.get_attr(self_val.0, "trace_dispatch")
        }

        fn is_skipped_module(
            #[positional_only] self_val: PyValueRef<'_>,
            module_name: PyValue,
        ) -> Result<Value> {
            if module_name.0.is_none() {
                return Ok(Value::bool_(false));
            }
            let skip = _interp.get_attr(self_val.0, "skip")?;
            if !_interp.truthy_value(&skip)? {
                return Ok(Value::bool_(false));
            }
            let fnmatch_mod = _interp.load_module("fnmatch")?;
            let fnmatch_fn = _interp.get_attr(&fnmatch_mod, "fnmatch")?;
            let patterns = _interp.collect_iterable(&skip)?;
            for pattern in patterns {
                let matched = call_pos(
                    _interp,
                    fnmatch_fn.clone(),
                    &[module_name.0.clone(), pattern],
                )?;
                if _interp.truthy_value(&matched)? {
                    return Ok(Value::bool_(true));
                }
            }
            Ok(Value::bool_(false))
        }

        fn stop_here(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            let skip = _interp.get_attr(self_val.0, "skip")?;
            if _interp.truthy_value(&skip)? {
                let f_globals = _interp.get_attr(&frame.0, "f_globals")?;
                let mod_name = call_method(
                    _interp,
                    &f_globals,
                    "get",
                    &[Value::string("__name__")],
                )?;
                let skipped = call_method(
                    _interp,
                    self_val.0,
                    "is_skipped_module",
                    &[mod_name],
                )?;
                if _interp.truthy_value(&skipped)? {
                    return Ok(Value::bool_(false));
                }
            }
            let stopframe = _interp.get_attr(self_val.0, "stopframe")?;
            if values_are_identical(&frame.0, &stopframe) {
                let stoplineno = _interp
                    .get_attr(self_val.0, "stoplineno")?
                    .as_int()
                    .unwrap_or(0);
                if stoplineno == -1 {
                    return Ok(Value::bool_(false));
                }
                let fline = frame_lineno(_interp, &frame.0)?;
                return Ok(Value::bool_(fline >= stoplineno));
            }
            if !_interp.truthy_value(&stopframe)? {
                return Ok(Value::bool_(true));
            }
            Ok(Value::bool_(false))
        }

        fn break_here(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            let f_code = _interp.get_attr(&frame.0, "f_code")?;
            let co_filename = _interp.get_attr(&f_code, "co_filename")?;
            let filename = call_method(_interp, self_val.0, "canonic", &[co_filename])?;
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            if !dict_contains_key(_interp, &breaks, filename.clone())? {
                return Ok(Value::bool_(false));
            }
            let file_breaks = dict_get_item(_interp, &breaks, filename.clone())?;
            let mut lineno = _interp.get_attr(&frame.0, "f_lineno")?;
            if !list_contains(_interp, &file_breaks, &lineno)? {
                lineno = _interp.get_attr(&f_code, "co_firstlineno")?;
                if !list_contains(_interp, &file_breaks, &lineno)? {
                    return Ok(Value::bool_(false));
                }
            }
            let (bp, flag) = effective_impl(_interp, filename, lineno, &frame.0)?;
            if _interp.truthy_value(&bp)? {
                let num = _interp.get_attr(&bp, "number")?;
                _interp.assign_attr(self_val.0.clone(), "currentbp", num.clone())?;
                let temp = _interp.get_attr(&bp, "temporary")?;
                if _interp.truthy_value(&flag)? && _interp.truthy_value(&temp)? {
                    let num_str = _interp.render_value_as_str(&num)?;
                    call_method(_interp, self_val.0, "do_clear", &[Value::string(num_str)])?;
                }
                Ok(Value::bool_(true))
            } else {
                Ok(Value::bool_(false))
            }
        }

        fn do_clear(
            #[positional_only] _self_val: PyValueRef<'_>,
            _arg: PyValue,
        ) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "subclass of bdb must implement do_clear()",
            ))
        }

        fn break_anywhere(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            let f_code = _interp.get_attr(&frame.0, "f_code")?;
            let co_filename = _interp.get_attr(&f_code, "co_filename")?;
            let filename = call_method(_interp, self_val.0, "canonic", &[co_filename])?;
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            Ok(Value::bool_(dict_contains_key(_interp, &breaks, filename)?))
        }

        fn user_call(
            #[positional_only] _self_val: PyValueRef<'_>,
            _frame: PyValue,
            _argument_list: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn user_line(
            #[positional_only] _self_val: PyValueRef<'_>,
            _frame: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn user_return(
            #[positional_only] _self_val: PyValueRef<'_>,
            _frame: PyValue,
            _return_value: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn user_exception(
            #[positional_only] _self_val: PyValueRef<'_>,
            _frame: PyValue,
            _exc_info: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn _set_stopinfo(
            #[positional_only] self_val: PyValueRef<'_>,
            stopframe: PyValue,
            returnframe: PyValue,
            #[default(OptionalArg::Omitted)] stoplineno: OptValueRef<'_>,
        ) -> Result<Value> {
            let sl = stoplineno
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::int(0));
            _interp.assign_attr(self_val.0.clone(), "stopframe", stopframe.0)?;
            _interp.assign_attr(self_val.0.clone(), "returnframe", returnframe.0)?;
            _interp.assign_attr(self_val.0.clone(), "quitting", Value::bool_(false))?;
            _interp.assign_attr(self_val.0.clone(), "stoplineno", sl)?;
            Ok(Value::none())
        }

        fn _set_caller_tracefunc(
            #[positional_only] self_val: PyValueRef<'_>,
            current_frame: PyValue,
        ) -> Result<Value> {
            let caller_frame = _interp.get_attr(&current_frame.0, "f_back")?;
            if caller_frame.is_none() {
                return Ok(Value::none());
            }
            let caller_trace = _interp.get_attr(&caller_frame, "f_trace")?;
            let botframe = _interp.get_attr(self_val.0, "botframe")?;
            if !_interp.truthy_value(&caller_trace)?
                && !values_are_identical(&caller_frame, &botframe)
            {
                let td = _interp.get_attr(self_val.0, "trace_dispatch")?;
                _interp.assign_attr(caller_frame, "f_trace", td)?;
            }
            Ok(Value::none())
        }

        fn set_until(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            #[default(OptionalArg::Omitted)] lineno: OptValueRef<'_>,
        ) -> Result<Value> {
            let target_line = match lineno.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => Value::int(frame_lineno(_interp, &frame.0)? + 1),
            };
            call_method(
                _interp,
                self_val.0,
                "_set_stopinfo",
                &[frame.0.clone(), frame.0, target_line],
            )?;
            Ok(Value::none())
        }

        fn set_step(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_set_stopinfo",
                &[Value::none(), Value::none()],
            )?;
            Ok(Value::none())
        }

        fn set_next(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            call_method(
                _interp,
                self_val.0,
                "_set_stopinfo",
                &[frame.0, Value::none()],
            )?;
            Ok(Value::none())
        }

        fn set_return(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
        ) -> Result<Value> {
            if (frame_code_flags(_interp, &frame.0)? & GENERATOR_AND_COROUTINE_FLAGS) != 0 {
                call_method(
                    _interp,
                    self_val.0,
                    "_set_stopinfo",
                    &[frame.0, Value::none(), Value::int(-1)],
                )?;
            } else {
                let f_back = _interp.get_attr(&frame.0, "f_back")?;
                call_method(_interp, self_val.0, "_set_stopinfo", &[f_back, frame.0])?;
            }
            Ok(Value::none())
        }

        fn set_trace(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] frame: OptValueRef<'_>,
        ) -> Result<Value> {
            set_sys_trace(_interp, Value::none())?;
            let mut cur_frame = match frame.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => {
                    // Native methods do not push a Python frame, so depth 0 is the caller's frame.
                    _interp.build_frame_object(0, pyrust_core::get_current_vm_line() as i64)
                }
            };
            call_method(_interp, self_val.0, "reset", &[])?;
            _interp.assign_attr(self_val.0.clone(), "enterframe", cur_frame.clone())?;
            let td = _interp.get_attr(self_val.0, "trace_dispatch")?;
            let walk_res: Result<()> = (|| {
                while !cur_frame.is_none() {
                    _interp.assign_attr(cur_frame.clone(), "f_trace", td.clone())?;
                    _interp.assign_attr(self_val.0.clone(), "botframe", cur_frame.clone())?;
                    cur_frame = _interp.get_attr(&cur_frame, "f_back")?;
                }
                call_method(_interp, self_val.0, "set_step", &[])?;
                Ok(())
            })();
            let _ = _interp.assign_attr(self_val.0.clone(), "enterframe", Value::none());
            walk_res?;
            set_sys_trace(_interp, td)?;
            Ok(Value::none())
        }

        fn set_continue(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let botframe = _interp.get_attr(self_val.0, "botframe")?;
            call_method(
                _interp,
                self_val.0,
                "_set_stopinfo",
                &[botframe.clone(), Value::none(), Value::int(-1)],
            )?;
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            if !_interp.truthy_value(&breaks)? {
                set_sys_trace(_interp, Value::none())?;
                let mut frame =
                    _interp.build_frame_object(0, pyrust_core::get_current_vm_line() as i64);
                while !frame.is_none() && !values_are_identical(&frame, &botframe) {
                    let _ = _interp.assign_attr(frame.clone(), "f_trace", Value::none());
                    frame = _interp.get_attr(&frame, "f_back")?;
                }
            }
            Ok(Value::none())
        }

        fn set_quit(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let botframe = _interp.get_attr(self_val.0, "botframe")?;
            _interp.assign_attr(self_val.0.clone(), "stopframe", botframe)?;
            _interp.assign_attr(self_val.0.clone(), "returnframe", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "quitting", Value::bool_(true))?;
            set_sys_trace(_interp, Value::none())?;
            Ok(Value::none())
        }

        fn _add_to_breaks(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            lineno: PyValue,
        ) -> Result<Value> {
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            let bp_linenos = call_method(
                _interp,
                &breaks,
                "setdefault",
                &[filename.0, Value::list(Vec::new())],
            )?;
            if !list_contains(_interp, &bp_linenos, &lineno.0)? {
                call_method(_interp, &bp_linenos, "append", &[lineno.0])?;
            }
            Ok(Value::none())
        }

        fn set_break(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            lineno: PyIndex,
            #[default(OptionalArg::Omitted)] temporary: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] cond: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] funcname: OptValueRef<'_>,
        ) -> Result<Value> {
            let line_num = lineno.expect_i64(FN_NAME, "lineno")?;
            let canon = call_method(_interp, self_val.0, "canonic", &[filename.0])?;
            let canon_str = _interp.render_value_as_str(&canon)?;

            let lc = _interp.load_module("linecache")?;
            let getline = _interp.get_attr(&lc, "getline")?;
            let line = call_pos(_interp, getline, &[canon.clone(), Value::int(line_num)])?;
            if !_interp.truthy_value(&line)? {
                return Ok(Value::string(format!(
                    "Line {canon_str}:{line_num} does not exist"
                )));
            }
            call_method(
                _interp,
                self_val.0,
                "_add_to_breaks",
                &[canon.clone(), Value::int(line_num)],
            )?;
            let temp_val = temporary
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::bool_(false));
            let cond_val = cond.as_value().cloned().unwrap_or_else(Value::none);
            let func_val = funcname.as_value().cloned().unwrap_or_else(Value::none);
            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            call_pos(
                _interp,
                bp_cls,
                &[canon, Value::int(line_num), temp_val, cond_val, func_val],
            )?;

            let mut frame = _interp.get_attr(self_val.0, "enterframe")?;
            let td = _interp.get_attr(self_val.0, "trace_dispatch")?;
            while !frame.is_none() {
                let any_brk = call_method(
                    _interp,
                    self_val.0,
                    "break_anywhere",
                    std::slice::from_ref(&frame),
                )?;
                if _interp.truthy_value(&any_brk)? {
                    _interp.assign_attr(frame.clone(), "f_trace", td.clone())?;
                }
                frame = _interp.get_attr(&frame, "f_back")?;
            }
            Ok(Value::none())
        }

        fn _load_breaks(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            let bplist = _interp.get_attr(&bp_cls, "bplist")?;
            let keys_val = call_method(_interp, &bplist, "keys", &[])?;
            let keys = _interp.collect_iterable(&keys_val)?;
            for key in keys {
                if let Some(pair) = key.as_tuple()
                    && pair.len() == 2
                {
                    call_method(
                        _interp,
                        self_val.0,
                        "_add_to_breaks",
                        &[pair[0].clone(), pair[1].clone()],
                    )?;
                }
            }
            Ok(Value::none())
        }

        fn _prune_breaks(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            lineno: PyValue,
        ) -> Result<Value> {
            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            let bplist = _interp.get_attr(&bp_cls, "bplist")?;
            let key = Value::tuple(vec![filename.0.clone(), lineno.0.clone()]);
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            if !dict_contains_key(_interp, &bplist, key)? {
                let entry = dict_get_item(_interp, &breaks, filename.0.clone())?;
                call_method(_interp, &entry, "remove", &[lineno.0])?;
            }
            let entry = dict_get_item(_interp, &breaks, filename.0.clone())?;
            if !_interp.truthy_value(&entry)? {
                dict_del_item(_interp, &breaks, filename.0)?;
            }
            Ok(Value::none())
        }

        fn clear_break(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            lineno: PyIndex,
        ) -> Result<Value> {
            let line_num = lineno.expect_i64(FN_NAME, "lineno")?;
            let canon = call_method(_interp, self_val.0, "canonic", &[filename.0])?;
            let canon_str = _interp.render_value_as_str(&canon)?;
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            if !dict_contains_key(_interp, &breaks, canon.clone())? {
                return Ok(Value::string(format!(
                    "There are no breakpoints in {canon_str}"
                )));
            }
            let file_breaks = dict_get_item(_interp, &breaks, canon.clone())?;
            let line_val = Value::int(line_num);
            if !list_contains(_interp, &file_breaks, &line_val)? {
                return Ok(Value::string(format!(
                    "There is no breakpoint at {canon_str}:{line_num}"
                )));
            }
            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            let bplist = _interp.get_attr(&bp_cls, "bplist")?;
            let entry = dict_get_item(
                _interp,
                &bplist,
                Value::tuple(vec![canon.clone(), line_val.clone()]),
            )?;
            let bps = _interp.collect_iterable(&entry)?;
            for bp in bps {
                call_method(_interp, &bp, "deleteMe", &[])?;
            }
            call_method(_interp, self_val.0, "_prune_breaks", &[canon, line_val])?;
            Ok(Value::none())
        }

        fn clear_bpbynumber(
            #[positional_only] self_val: PyValueRef<'_>,
            arg: PyValue,
        ) -> Result<Value> {
            let bp = match call_method(_interp, self_val.0, "get_bpbynumber", &[arg.0]) {
                Ok(bp) => bp,
                Err(err) => {
                    if let Ok(exc) = _interp.materialize_pyerror(err.clone())
                        && is_instance_of_name(_interp, &exc, "ValueError")
                    {
                        let s = _interp.render_value_as_str(&exc)?;
                        return Ok(Value::string(s));
                    }
                    return Err(err);
                }
            };
            call_method(_interp, &bp, "deleteMe", &[])?;
            let bp_file = _interp.get_attr(&bp, "file")?;
            let bp_line = _interp.get_attr(&bp, "line")?;
            call_method(_interp, self_val.0, "_prune_breaks", &[bp_file, bp_line])?;
            Ok(Value::none())
        }

        fn clear_all_file_breaks(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
        ) -> Result<Value> {
            let canon = call_method(_interp, self_val.0, "canonic", &[filename.0])?;
            let canon_str = _interp.render_value_as_str(&canon)?;
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            if !dict_contains_key(_interp, &breaks, canon.clone())? {
                return Ok(Value::string(format!(
                    "There are no breakpoints in {canon_str}"
                )));
            }
            let lines_val = dict_get_item(_interp, &breaks, canon.clone())?;
            let lines = _interp.collect_iterable(&lines_val)?;
            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            let bplist = _interp.get_attr(&bp_cls, "bplist")?;
            for line in lines {
                let entry = dict_get_item(_interp, &bplist, Value::tuple(vec![canon.clone(), line]))?;
                let bps = _interp.collect_iterable(&entry)?;
                for bp in bps {
                    call_method(_interp, &bp, "deleteMe", &[])?;
                }
            }
            dict_del_item(_interp, &breaks, canon)?;
            Ok(Value::none())
        }

        fn clear_all_breaks(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            if !_interp.truthy_value(&breaks)? {
                return Ok(Value::string("There are no breakpoints"));
            }
            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            let bpbynumber = _interp.get_attr(&bp_cls, "bpbynumber")?;
            let bps = _interp.collect_iterable(&bpbynumber)?;
            for bp in bps {
                if _interp.truthy_value(&bp)? {
                    call_method(_interp, &bp, "deleteMe", &[])?;
                }
            }
            _interp.assign_attr(self_val.0.clone(), "breaks", Value::dict(PyDict::default()))?;
            Ok(Value::none())
        }

        fn get_bpbynumber(
            #[positional_only] _self_val: PyValueRef<'_>,
            arg: PyValue,
        ) -> Result<Value> {
            if !_interp.truthy_value(&arg.0)? {
                return Err(PyError::named("ValueError", "Breakpoint number expected"));
            }
            let int_cls = crate::interpreter::primitive_class_by_name("int")
                .map(Value::py_class)
                .unwrap_or_else(|| Value::builtin_function("int"));
            let num_val = match call_pos(_interp, int_cls, std::slice::from_ref(&arg.0)) {
                Ok(v) => v,
                Err(_) => {
                    let arg_s = _interp.render_value_as_str(&arg.0)?;
                    return Err(PyError::named(
                        "ValueError",
                        format!("Non-numeric breakpoint number {arg_s}"),
                    ));
                }
            };
            let number = num_val.as_int().unwrap_or(0);
            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            let bpbynumber = _interp.get_attr(&bp_cls, "bpbynumber")?;
            let bp = match _interp.eval_index(&bpbynumber, Value::int(number)) {
                Ok(v) => v,
                Err(_) => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("Breakpoint number {number} out of range"),
                    ));
                }
            };
            if bp.is_none() {
                return Err(PyError::named(
                    "ValueError",
                    format!("Breakpoint {number} already deleted"),
                ));
            }
            Ok(bp)
        }

        fn get_break(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            lineno: PyValue,
        ) -> Result<Value> {
            let canon = call_method(_interp, self_val.0, "canonic", &[filename.0])?;
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            if !dict_contains_key(_interp, &breaks, canon.clone())? {
                return Ok(Value::bool_(false));
            }
            let file_breaks = dict_get_item(_interp, &breaks, canon)?;
            Ok(Value::bool_(list_contains(_interp, &file_breaks, &lineno.0)?))
        }

        fn get_breaks(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            lineno: PyValue,
        ) -> Result<Value> {
            let canon = call_method(_interp, self_val.0, "canonic", &[filename.0])?;
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            if !dict_contains_key(_interp, &breaks, canon.clone())? {
                return Ok(Value::list(Vec::new()));
            }
            let file_breaks = dict_get_item(_interp, &breaks, canon.clone())?;
            if !list_contains(_interp, &file_breaks, &lineno.0)? {
                return Ok(Value::list(Vec::new()));
            }
            let bp_cls = bdb_attr(_interp, "Breakpoint")?;
            let bplist = _interp.get_attr(&bp_cls, "bplist")?;
            dict_get_item(_interp, &bplist, Value::tuple(vec![canon, lineno.0]))
        }

        fn get_file_breaks(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
        ) -> Result<Value> {
            let canon = call_method(_interp, self_val.0, "canonic", &[filename.0])?;
            let breaks = _interp.get_attr(self_val.0, "breaks")?;
            if dict_contains_key(_interp, &breaks, canon.clone())? {
                dict_get_item(_interp, &breaks, canon)
            } else {
                Ok(Value::list(Vec::new()))
            }
        }

        fn get_all_breaks(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "breaks")
        }

        fn get_stack(
            #[positional_only] self_val: PyValueRef<'_>,
            f: PyValue,
            t: PyValue,
        ) -> Result<Value> {
            let mut stack: Vec<Value> = Vec::new();
            let mut cur_f = f.0;
            let mut cur_t = t.0;
            let botframe = _interp
                .get_attr(self_val.0, "botframe")
                .unwrap_or_else(|_| Value::none());

            if !cur_t.is_none() {
                let tb_frame = _interp.get_attr(&cur_t, "tb_frame")?;
                if values_are_identical(&tb_frame, &cur_f) {
                    cur_t = _interp.get_attr(&cur_t, "tb_next")?;
                }
            }
            while !cur_f.is_none() {
                let fline = _interp.get_attr(&cur_f, "f_lineno")?;
                stack.push(Value::tuple(vec![cur_f.clone(), fline]));
                if values_are_identical(&cur_f, &botframe) {
                    break;
                }
                cur_f = _interp.get_attr(&cur_f, "f_back")?;
            }
            stack.reverse();
            let mut idx = stack.len().saturating_sub(1);
            while !cur_t.is_none() {
                let tb_frame = _interp.get_attr(&cur_t, "tb_frame")?;
                let tb_lineno = _interp.get_attr(&cur_t, "tb_lineno")?;
                stack.push(Value::tuple(vec![tb_frame, tb_lineno]));
                cur_t = _interp.get_attr(&cur_t, "tb_next")?;
            }
            if cur_f.is_none() {
                idx = stack.len().saturating_sub(1);
            }
            Ok(Value::tuple(vec![Value::list(stack), Value::int(idx as i64)]))
        }

        fn format_stack_entry(
            #[positional_only] self_val: PyValueRef<'_>,
            frame_lineno: PyValue,
            #[default(OptionalArg::Omitted)] lprefix: OptValueRef<'_>,
        ) -> Result<Value> {
            let prefix = match lprefix.as_value() {
                Some(v) => _interp.render_value_as_str(v)?,
                None => ": ".to_string(),
            };
            let pair = _interp.collect_iterable(&frame_lineno.0)?;
            if pair.len() != 2 {
                return Err(PyError::named(
                    "ValueError",
                    "frame_lineno must be a 2-tuple (frame, lineno)",
                ));
            }
            let frame = &pair[0];
            let lineno = &pair[1];
            let f_code = _interp.get_attr(frame, "f_code")?;
            let co_filename = _interp.get_attr(&f_code, "co_filename")?;
            let filename_val = call_method(_interp, self_val.0, "canonic", &[co_filename])?;
            let filename = _interp.render_value_as_str(&filename_val)?;
            let lineno_repr = render_value_repr(_interp, lineno)?;
            let mut s = format!("{filename}({lineno_repr})");
            let co_name = _interp.get_attr(&f_code, "co_name")?;
            let co_name_str = _interp.render_value_as_str(&co_name)?;
            if !co_name_str.is_empty() {
                s.push_str(&co_name_str);
            } else {
                s.push_str("<lambda>");
            }
            s.push_str("()");

            let f_locals = _interp.get_attr(frame, "f_locals")?;
            let ret_key = Value::string("__return__");
            if dict_contains_key(_interp, &f_locals, ret_key.clone())? {
                let rv = dict_get_item(_interp, &f_locals, ret_key)?;
                let reprlib_mod = _interp.load_module("reprlib")?;
                let repr_fn = _interp.get_attr(&reprlib_mod, "repr")?;
                let rv_str = call_pos(_interp, repr_fn, &[rv])?;
                s.push_str("->");
                s.push_str(&_interp.render_value_as_str(&rv_str)?);
            }

            if !lineno.is_none() {
                let lc = _interp.load_module("linecache")?;
                let getline = _interp.get_attr(&lc, "getline")?;
                let f_globals = _interp.get_attr(frame, "f_globals")?;
                let line = call_pos(_interp, getline, &[filename_val, lineno.clone(), f_globals])?;
                if _interp.truthy_value(&line)? {
                    let line_str = _interp.render_value_as_str(&line)?;
                    s.push_str(&prefix);
                    s.push_str(line_str.trim());
                }
            } else {
                s.push_str(&format!("{prefix}Warning: lineno is None"));
            }
            Ok(Value::string(s))
        }

        fn run(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyValue,
            #[default(OptionalArg::Omitted)] globals: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] locals: OptValueRef<'_>,
        ) -> Result<Value> {
            let g = match globals.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => default_main_dict(_interp)?,
            };
            let l = match locals.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => g.clone(),
            };
            call_method(_interp, self_val.0, "reset", &[])?;
            let code_obj = if cmd.0.as_str().is_some() {
                call_pos(
                    _interp,
                    Value::builtin_function("compile"),
                    &[cmd.0, Value::string("<string>"), Value::string("exec")],
                )?
            } else {
                cmd.0
            };
            let td = _interp.get_attr(self_val.0, "trace_dispatch")?;
            set_sys_trace(_interp, td)?;
            let exec_res = call_pos(
                _interp,
                Value::builtin_function("exec"),
                &[code_obj, g, l],
            );
            let _ = _interp.assign_attr(self_val.0.clone(), "quitting", Value::bool_(true));
            let _ = set_sys_trace(_interp, Value::none());
            match exec_res {
                Ok(_) => Ok(Value::none()),
                Err(e) if is_bdb_quit(_interp, &e) => Ok(Value::none()),
                Err(e) => Err(e),
            }
        }

        fn runeval(
            #[positional_only] self_val: PyValueRef<'_>,
            expr: PyValue,
            #[default(OptionalArg::Omitted)] globals: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] locals: OptValueRef<'_>,
        ) -> Result<Value> {
            let g = match globals.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => default_main_dict(_interp)?,
            };
            let l = match locals.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => g.clone(),
            };
            call_method(_interp, self_val.0, "reset", &[])?;
            let td = _interp.get_attr(self_val.0, "trace_dispatch")?;
            set_sys_trace(_interp, td)?;
            let eval_res = call_pos(
                _interp,
                Value::builtin_function("eval"),
                &[expr.0, g, l],
            );
            let _ = _interp.assign_attr(self_val.0.clone(), "quitting", Value::bool_(true));
            let _ = set_sys_trace(_interp, Value::none());
            match eval_res {
                Ok(v) => Ok(v),
                Err(e) if is_bdb_quit(_interp, &e) => Ok(Value::none()),
                Err(e) => Err(e),
            }
        }

        fn runctx(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyValue,
            globals: PyValue,
            locals: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "run", &[cmd.0, globals.0, locals.0])
        }

        fn runcall(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] func: PyValue,
            #[varargs] args: &[Value],
            #[varkwargs] kwds: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "reset", &[])?;
            let td = _interp.get_attr(self_val.0, "trace_dispatch")?;
            set_sys_trace(_interp, td)?;

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
            let call_res = _interp.call_function_expanded(func.0, &expanded);
            let _ = _interp.assign_attr(self_val.0.clone(), "quitting", Value::bool_(true));
            let _ = set_sys_trace(_interp, Value::none());
            match call_res {
                Ok(v) => Ok(v),
                Err(e) if is_bdb_quit(_interp, &e) => Ok(Value::none()),
                Err(e) => Err(e),
            }
        }
    }

    fn checkfuncname(b: PyValue, frame: PyValue) -> Result<Value> {
        Ok(Value::bool_(checkfuncname_impl(_interp, &b.0, &frame.0)?))
    }

    fn effective(file: PyValue, line: PyValue, frame: PyValue) -> Result<Value> {
        let (bp, flag) = effective_impl(_interp, file.0, line.0, &frame.0)?;
        Ok(Value::tuple(vec![bp, flag]))
    }

    fn set_trace() -> Result<Value> {
        let bdb_cls = bdb_attr(_interp, "Bdb")?;
        let inst = call_pos(_interp, bdb_cls, &[])?;
        let frame = _interp.build_frame_object(0, pyrust_core::get_current_vm_line() as i64);
        call_method(_interp, &inst, "set_trace", &[frame])?;
        Ok(Value::none())
    }
}
