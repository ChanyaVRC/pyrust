// `cProfile` module — 100% native Rust implementation of Python 3.12's
// `cProfile` module (issue #3247).
//
// `cProfile.Profile` inherits from `_lsprof.Profiler` (wired via
// `prepare_module` using `link_class_bases`).
//
// Reference: <https://docs.python.org/3/library/profile.html>

use std::collections::HashMap;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyPath, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, Value, ValueKind};

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

fn cprofile_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("cProfile")?;
    interp.get_attr(&m, name)
}

fn label_impl(interp: &mut Interpreter, code: &Value) -> Result<Value> {
    if let Some(s) = code.as_str() {
        return Ok(Value::tuple(vec![
            Value::string("~"),
            Value::int(0),
            Value::string(s),
        ]));
    }
    let filename = interp.get_attr(code, "co_filename")?;
    let firstlineno = interp.get_attr(code, "co_firstlineno")?;
    let name = interp.get_attr(code, "co_name")?;
    Ok(Value::tuple(vec![filename, firstlineno, name]))
}

fn is_system_exit(interp: &mut Interpreter, err: &PyError) -> bool {
    let Ok(exc) = interp.materialize_pyerror(err.clone()) else {
        return false;
    };
    let Some(cls) = crate::interpreter::lookup_exc_class("SystemExit") else {
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

fn finish_run_util(
    interp: &mut Interpreter,
    prof: &Value,
    filename: Option<&Value>,
    sort: Value,
) -> Result<()> {
    if let Some(fname) = filename.filter(|v| !v.is_none()) {
        call_method(interp, prof, "dump_stats", std::slice::from_ref(fname))?;
    } else {
        call_method(interp, prof, "print_stats", &[sort])?;
    }
    Ok(())
}

/// Wire `cProfile.Profile` inheritance from `_lsprof.Profiler`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let prof_val = mod_rc.borrow().attrs.get("Profile").cloned();
    let Some(prof_val) = prof_val else {
        return;
    };
    let ValueKind::PyClass(prof_cls) = prof_val.kind() else {
        return;
    };
    if let Ok(lsprof_mod) = interp.load_module("_lsprof")
        && let Ok(base_val) = interp.get_attr(&lsprof_mod, "Profiler")
        && let ValueKind::PyClass(base_cls) = base_val.kind()
    {
        crate::builtin_modules::macros::link_class_bases(prof_cls, vec![Rc::clone(base_cls)]);
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("run"),
            Value::string("runctx"),
            Value::string("Profile"),
        ]),
    }

    fn label(code: PyValue) -> Result<Value> {
        label_impl(_interp, &code.0)
    }

    fn run(
        statement: PyValue,
        #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] sort: OptValueRef<'_>,
    ) -> Result<Value> {
        let prof_cls = cprofile_attr(_interp, "Profile")?;
        let prof = call_pos(_interp, prof_cls, &[])?;
        let sort_val = sort.as_value().cloned().unwrap_or_else(|| Value::int(-1));
        let run_res = call_method(_interp, &prof, "run", &[statement.0]);
        let propagate = match run_res {
            Ok(_) => None,
            Err(e) if is_system_exit(_interp, &e) => None,
            Err(e) => Some(e),
        };
        if let Some(e) = propagate {
            return Err(e);
        }
        finish_run_util(_interp, &prof, filename.as_value(), sort_val)?;
        Ok(Value::none())
    }

    fn runctx(
        statement: PyValue,
        globals: PyValue,
        locals: PyValue,
        #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] sort: OptValueRef<'_>,
    ) -> Result<Value> {
        let prof_cls = cprofile_attr(_interp, "Profile")?;
        let prof = call_pos(_interp, prof_cls, &[])?;
        let sort_val = sort.as_value().cloned().unwrap_or_else(|| Value::int(-1));
        let run_res = call_method(_interp, &prof, "runctx", &[statement.0, globals.0, locals.0]);
        let propagate = match run_res {
            Ok(_) => None,
            Err(e) if is_system_exit(_interp, &e) => None,
            Err(e) => Some(e),
        };
        if let Some(e) = propagate {
            return Err(e);
        }
        finish_run_util(_interp, &prof, filename.as_value(), sort_val)?;
        Ok(Value::none())
    }

    class Profile {
        fn print_stats(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] sort: OptValueRef<'_>,
        ) -> Result<Value> {
            let sort_val = sort.as_value().cloned().unwrap_or_else(|| Value::int(-1));
            let pstats_mod = _interp.load_module("pstats")?;
            let stats_cls = _interp.get_attr(&pstats_mod, "Stats")?;
            let st = call_pos(_interp, stats_cls, std::slice::from_ref(self_val.0))?;
            let st = call_method(_interp, &st, "strip_dirs", &[])?;
            let st = call_method(_interp, &st, "sort_stats", &[sort_val])?;
            call_method(_interp, &st, "print_stats", &[])?;
            Ok(Value::none())
        }

        fn dump_stats(
            #[positional_only] self_val: PyValueRef<'_>,
            file: PyPath,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "create_stats", &[])?;
            let stats = _interp.get_attr(self_val.0, "stats")?;
            let bytes = super::pstats::marshal_dump_stats(&stats)?;
            std::fs::write(file.as_path(), bytes)
                .map_err(|e| PyError::from_io_error(&e, Some(file.as_path().to_string_lossy().as_ref())))?;
            Ok(Value::none())
        }

        fn create_stats(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "disable", &[])?;
            call_method(_interp, self_val.0, "snapshot_stats", &[])?;
            Ok(Value::none())
        }

        fn snapshot_stats(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let entries_val = call_method(_interp, self_val.0, "getstats", &[])?;
            let entries = _interp.collect_iterable(&entries_val)?;
            let stats_dict = Value::dict(PyDict::default());
            let mut callers_by_id: HashMap<i64, Value> = HashMap::new();

            for entry in &entries {
                let code = _interp.get_attr(entry, "code")?;
                let func = label_impl(_interp, &code)?;
                let nc = _interp.get_attr(entry, "callcount")?.as_int().unwrap_or(0);
                let rec = _interp
                    .get_attr(entry, "reccallcount")?
                    .as_int()
                    .unwrap_or(0);
                let cc = nc - rec;
                let tt = _interp.get_attr(entry, "inlinetime")?;
                let ct = _interp.get_attr(entry, "totaltime")?;
                let callers = Value::dict(PyDict::default());
                let code_id = code.object_id().as_int().unwrap_or(0);
                callers_by_id.insert(code_id, callers.clone());
                let stat_tuple = Value::tuple(vec![
                    Value::int(cc),
                    Value::int(nc),
                    tt,
                    ct,
                    callers,
                ]);
                call_method(_interp, &stats_dict, "__setitem__", &[func, stat_tuple])?;
            }

            for entry in &entries {
                let calls_val = _interp.get_attr(entry, "calls")?;
                if !_interp.truthy_value(&calls_val)? {
                    continue;
                }
                let code = _interp.get_attr(entry, "code")?;
                let func = label_impl(_interp, &code)?;
                let subentries = _interp.collect_iterable(&calls_val)?;
                for sub in subentries {
                    let sub_code = _interp.get_attr(&sub, "code")?;
                    let sub_id = sub_code.object_id().as_int().unwrap_or(0);
                    let Some(callers) = callers_by_id.get(&sub_id) else {
                        continue;
                    };
                    let mut nc = _interp.get_attr(&sub, "callcount")?.as_int().unwrap_or(0);
                    let rec = _interp
                        .get_attr(&sub, "reccallcount")?
                        .as_int()
                        .unwrap_or(0);
                    let mut cc = nc - rec;
                    let mut tt = match _interp.get_attr(&sub, "inlinetime")?.kind() {
                        ValueKind::Float(f) => f,
                        ValueKind::Int(n) => n as f64,
                        _ => 0.0,
                    };
                    let mut ct = match _interp.get_attr(&sub, "totaltime")?.kind() {
                        ValueKind::Float(f) => f,
                        ValueKind::Int(n) => n as f64,
                        _ => 0.0,
                    };
                    let has = call_method(
                        _interp,
                        callers,
                        "__contains__",
                        std::slice::from_ref(&func),
                    )?;
                    if _interp.truthy_value(&has)? {
                        let prev = _interp.eval_index(callers, func.clone())?;
                        if let Some(pt) = prev.as_tuple()
                            && pt.len() == 4
                        {
                            nc += pt[0].as_int().unwrap_or(0);
                            cc += pt[1].as_int().unwrap_or(0);
                            tt += match pt[2].kind() {
                                ValueKind::Float(f) => f,
                                ValueKind::Int(n) => n as f64,
                                _ => 0.0,
                            };
                            ct += match pt[3].kind() {
                                ValueKind::Float(f) => f,
                                ValueKind::Int(n) => n as f64,
                                _ => 0.0,
                            };
                        }
                    }
                    let new_val = Value::tuple(vec![
                        Value::int(nc),
                        Value::int(cc),
                        Value::float(tt),
                        Value::float(ct),
                    ]);
                    call_method(_interp, callers, "__setitem__", &[func.clone(), new_val])?;
                }
            }

            _interp.assign_attr(self_val.0.clone(), "stats", stats_dict)?;
            Ok(Value::none())
        }

        fn run(#[positional_only] self_val: PyValueRef<'_>, cmd: PyValue) -> Result<Value> {
            let main_mod = _interp.load_module("__main__")?;
            let dict = _interp.get_attr(&main_mod, "__dict__")?;
            call_method(_interp, self_val.0, "runctx", &[cmd.0, dict.clone(), dict])
        }

        fn runctx(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyValue,
            globals: PyValue,
            locals: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "enable", &[])?;
            let res = call_pos(
                _interp,
                Value::builtin_function("exec"),
                &[cmd.0, globals.0, locals.0],
            );
            let _ = call_method(_interp, self_val.0, "disable", &[]);
            res?;
            Ok(self_val.0.clone())
        }

        fn runcall(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] func: PyValue,
            #[varargs] args: &[Value],
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "enable", &[])?;
            let mut expanded: Vec<ExpandedCallArg> = args
                .iter()
                .cloned()
                .map(|value| ExpandedCallArg { name: None, value })
                .collect();
            for (k, v) in kw {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            let res = _interp.call_function_expanded(func.0, &expanded);
            let _ = call_method(_interp, self_val.0, "disable", &[]);
            res
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "enable", &[])?;
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _exc_info: &[Value],
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "disable", &[])?;
            Ok(Value::none())
        }
    }
}
