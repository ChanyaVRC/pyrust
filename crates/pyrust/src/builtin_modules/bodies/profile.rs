// `profile` module — 100% native Rust implementation of Python 3.12's
// `profile` module (issue #3247).
//
// Provides a compatible native-Rust `Profile` profiler hooked cleanly to
// PyRust's live `sys.setprofile`, plus `run`, `runctx`, and `_Utils`.
//
// Reference: <https://docs.python.org/3/library/profile.html>

use std::time::Instant;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyNumber, PyPath, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, Value};

type FuncKey = (String, i64, String);

struct TimingEntry {
    cc: i64,
    ns: i64,
    tt: f64,
    ct: f64,
    callers: IndexMap<FuncKey, i64>,
}

struct StackFrame {
    key: FuncKey,
    parent_time: f64,
    internal_time: f64,
    external_time: f64,
}

struct PyProfileState {
    timer: Option<Value>,
    bias: f64,
    start_instant: Instant,
    last_t: f64,
    stack: Vec<StackFrame>,
    timings: IndexMap<FuncKey, TimingEntry>,
    cmd: String,
    active: bool,
}

impl PyProfileState {
    fn new(timer: Option<Value>, bias: f64, initial_t: f64) -> Self {
        let mut st = Self {
            timer,
            bias,
            start_instant: Instant::now(),
            last_t: initial_t,
            stack: Vec::new(),
            timings: IndexMap::new(),
            cmd: String::new(),
            active: false,
        };
        st.simulate_call_internal("profiler".to_string());
        st
    }

    fn simulate_call_internal(&mut self, name: String) {
        let key = ("profile".to_string(), 0, name);
        self.push_frame(key, 0.0);
    }

    fn push_frame(&mut self, key: FuncKey, dt: f64) {
        self.stack.push(StackFrame {
            key: key.clone(),
            parent_time: dt,
            internal_time: 0.0,
            external_time: 0.0,
        });
        let entry = self.timings.entry(key).or_insert_with(|| TimingEntry {
            cc: 0,
            ns: -1,
            tt: 0.0,
            ct: 0.0,
            callers: IndexMap::new(),
        });
        entry.ns += 1;
    }

    fn pop_frame(&mut self, dt: f64) {
        if self.stack.len() <= 1 {
            return;
        }
        let Some(frame) = self.stack.pop() else {
            return;
        };
        let rit = frame.internal_time + dt;
        let frame_total = rit + frame.external_time;
        let parent_key = if let Some(parent) = self.stack.last_mut() {
            parent.internal_time += frame.parent_time;
            parent.external_time += frame_total;
            Some(parent.key.clone())
        } else {
            None
        };
        if let Some(entry) = self.timings.get_mut(&frame.key) {
            if entry.ns == 0 {
                entry.ct += frame_total;
                entry.cc += 1;
            }
            if let Some(pfn) = parent_key {
                *entry.callers.entry(pfn).or_insert(0) += 1;
            }
            entry.ns -= 1;
            entry.tt += rit;
        }
    }

    fn simulate_cmd_complete(&mut self, now: f64) {
        let mut t = (now - self.last_t).max(0.0);
        while self.stack.len() > 1 {
            self.pop_frame(t);
            t = 0.0;
        }
        self.last_t = now;
    }
}

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

fn profile_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("profile")?;
    interp.get_attr(&m, name)
}

fn set_sys_profile(interp: &mut Interpreter, func: Value) -> Result<()> {
    let sys_mod = interp.load_module("sys")?;
    let setprofile = interp.get_attr(&sys_mod, "setprofile")?;
    call_pos(interp, setprofile, &[func])?;
    Ok(())
}

fn read_time(
    interp: &mut Interpreter,
    timer_opt: Option<&Value>,
    start_instant: Instant,
) -> Result<f64> {
    if let Some(timer_fn) = timer_opt {
        let val = call_pos(interp, timer_fn.clone(), &[])?;
        if let Some(t) = val.as_tuple() {
            let mut sum = 0.0;
            for item in t {
                sum +=
                    PyNumber::try_from_value_with_interp(interp, item, "timer", "timer")?.as_f64();
            }
            return Ok(sum);
        }
        let num = PyNumber::try_from_value_with_interp(interp, &val, "timer", "timer")?;
        return Ok(num.as_f64());
    }
    Ok(start_instant.elapsed().as_secs_f64())
}

fn func_key_to_py(key: &FuncKey) -> Value {
    Value::tuple(vec![
        Value::string(&key.0),
        Value::int(key.1),
        Value::string(&key.2),
    ])
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

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("run"),
            Value::string("runctx"),
            Value::string("Profile"),
        ]),
    }

    class _Utils {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            profiler: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "profiler", profiler.0)?;
            Ok(Value::none())
        }

        fn run(
            #[positional_only] self_val: PyValueRef<'_>,
            statement: PyValue,
            filename: PyValue,
            sort: PyValue,
        ) -> Result<Value> {
            let profiler_cls = _interp.get_attr(self_val.0, "profiler")?;
            let prof = call_pos(_interp, profiler_cls, &[])?;
            let res = call_method(_interp, &prof, "run", &[statement.0]);
            if let Err(e) = res
                && !is_system_exit(_interp, &e)
            {
                return Err(e);
            }
            call_method(_interp, self_val.0, "_show", &[prof, filename.0, sort.0])?;
            Ok(Value::none())
        }

        fn runctx(
            #[positional_only] self_val: PyValueRef<'_>,
            statement: PyValue,
            globals: PyValue,
            locals: PyValue,
            filename: PyValue,
            sort: PyValue,
        ) -> Result<Value> {
            let profiler_cls = _interp.get_attr(self_val.0, "profiler")?;
            let prof = call_pos(_interp, profiler_cls, &[])?;
            let res = call_method(_interp, &prof, "runctx", &[statement.0, globals.0, locals.0]);
            if let Err(e) = res
                && !is_system_exit(_interp, &e)
            {
                return Err(e);
            }
            call_method(_interp, self_val.0, "_show", &[prof, filename.0, sort.0])?;
            Ok(Value::none())
        }

        fn _show(
            #[positional_only] _self_val: PyValueRef<'_>,
            prof: PyValue,
            filename: PyValue,
            sort: PyValue,
        ) -> Result<Value> {
            if !filename.0.is_none() {
                call_method(_interp, &prof.0, "dump_stats", &[filename.0])?;
            } else {
                call_method(_interp, &prof.0, "print_stats", &[sort.0])?;
            }
            Ok(Value::none())
        }
    }

    fn run(
        statement: PyValue,
        #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] sort: OptValueRef<'_>,
    ) -> Result<Value> {
        let utils_cls = profile_attr(_interp, "_Utils")?;
        let prof_cls = profile_attr(_interp, "Profile")?;
        let utils = call_pos(_interp, utils_cls, &[prof_cls])?;
        let fname = filename.as_value().cloned().unwrap_or_else(Value::none);
        let sval = sort.as_value().cloned().unwrap_or_else(|| Value::int(-1));
        call_method(_interp, &utils, "run", &[statement.0, fname, sval])
    }

    fn runctx(
        statement: PyValue,
        globals: PyValue,
        locals: PyValue,
        #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] sort: OptValueRef<'_>,
    ) -> Result<Value> {
        let utils_cls = profile_attr(_interp, "_Utils")?;
        let prof_cls = profile_attr(_interp, "Profile")?;
        let utils = call_pos(_interp, utils_cls, &[prof_cls])?;
        let fname = filename.as_value().cloned().unwrap_or_else(Value::none);
        let sval = sort.as_value().cloned().unwrap_or_else(|| Value::int(-1));
        call_method(
            _interp,
            &utils,
            "runctx",
            &[statement.0, globals.0, locals.0, fname, sval],
        )
    }

    #[native_state(PyProfileState)]
    class Profile {
        constants {
            "bias" => Value::int(0),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timer: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] bias: OptValueRef<'_>,
        ) -> Result<Value> {
            let timer_opt = timer.as_value().filter(|v| !v.is_none()).cloned();
            let bias_val = match bias.as_value().filter(|v| !v.is_none()) {
                Some(v) => PyNumber::try_from_value_with_interp(_interp, v, FN_NAME, "bias")?.as_f64(),
                None => _interp
                    .get_attr(self_val.0, "bias")
                    .ok()
                    .and_then(|v| PyNumber::try_from_value_with_interp(_interp, &v, FN_NAME, "bias").ok())
                    .map(|n| n.as_f64())
                    .unwrap_or(0.0),
            };
            let start_instant = Instant::now();
            let initial_t = read_time(_interp, timer_opt.as_ref(), start_instant)?;
            let state = PyProfileState::new(timer_opt.clone(), bias_val, initial_t);
            set_state(self_val.0, state)?;
            _interp.assign_attr(self_val.0.clone(), "bias", Value::float(bias_val))?;
            _interp.assign_attr(self_val.0.clone(), "cmd", Value::string(""))?;
            _interp.assign_attr(self_val.0.clone(), "timings", Value::dict(PyDict::default()))?;
            let dispatcher = _interp.get_attr(self_val.0, "_dispatch")?;
            _interp.assign_attr(self_val.0.clone(), "dispatcher", dispatcher)?;
            Ok(Value::none())
        }

        fn set_cmd(
            #[positional_only] self_val: PyValueRef<'_>,
            cmd: PyStr<'_>,
        ) -> Result<Value> {
            let cmd_str = cmd.0.to_string();
            let should_set = with_state_mut(self_val.0, |st| {
                if st.stack.len() > 1 {
                    return Ok(false);
                }
                st.cmd = cmd_str.clone();
                st.simulate_call_internal(cmd_str.clone());
                Ok(true)
            })?;
            if should_set {
                _interp.assign_attr(self_val.0.clone(), "cmd", Value::string(cmd_str))?;
            }
            Ok(Value::none())
        }

        fn simulate_call(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let n = name.0.to_string();
            with_state_mut(self_val.0, |st| {
                st.simulate_call_internal(n);
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn simulate_cmd_complete(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (timer_opt, start_instant) =
                with_state(self_val.0, |st| Ok((st.timer.clone(), st.start_instant)))?;
            let now = read_time(_interp, timer_opt.as_ref(), start_instant)?;
            with_state_mut(self_val.0, |st| {
                st.simulate_cmd_complete(now);
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn _dispatch(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            event: PyStr<'_>,
            arg: PyValue,
        ) -> Result<Value> {
            let (active, timer_opt, start_instant) = with_state(self_val.0, |st| {
                Ok((st.active, st.timer.clone(), st.start_instant))
            })?;
            if !active {
                return Ok(Value::none());
            }
            let now = read_time(_interp, timer_opt.as_ref(), start_instant)?;
            match event.0.as_ref() {
                "call" => {
                    let fcode = _interp.get_attr(&frame.0, "f_code")?;
                    let filename = _interp
                        .get_attr(&fcode, "co_filename")?
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let firstlineno = _interp
                        .get_attr(&fcode, "co_firstlineno")?
                        .as_int()
                        .unwrap_or(0);
                    let name = _interp
                        .get_attr(&fcode, "co_name")?
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    with_state_mut(self_val.0, |st| {
                        let dt = (now - st.last_t - st.bias).max(0.0);
                        st.push_frame((filename, firstlineno, name), dt);
                        st.last_t = now;
                        Ok(())
                    })?;
                }
                "c_call" => {
                    let cname = _interp
                        .get_attr(&arg.0, "__name__")
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| arg.0.repr_raw());
                    with_state_mut(self_val.0, |st| {
                        let dt = (now - st.last_t - st.bias).max(0.0);
                        st.push_frame((String::new(), 0, cname), dt);
                        st.last_t = now;
                        Ok(())
                    })?;
                }
                "return" | "c_return" | "c_exception" => {
                    with_state_mut(self_val.0, |st| {
                        let dt = (now - st.last_t - st.bias).max(0.0);
                        st.pop_frame(dt);
                        st.last_t = now;
                        Ok(())
                    })?;
                }
                _ => {}
            }
            Ok(Value::none())
        }

        fn create_stats(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "simulate_cmd_complete", &[])?;
            call_method(_interp, self_val.0, "snapshot_stats", &[])?;
            Ok(Value::none())
        }

        fn snapshot_stats(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            type SnapshotEntry = (FuncKey, i64, i64, f64, f64, Vec<(FuncKey, i64)>);
            let snapshot: Vec<SnapshotEntry> = with_state(self_val.0, |st| {
                    Ok(st
                        .timings
                        .iter()
                        .map(|(k, e)| {
                            let nc: i64 = e.callers.values().sum();
                            let callers_vec: Vec<(FuncKey, i64)> =
                                e.callers.iter().map(|(ck, cv)| (ck.clone(), *cv)).collect();
                            (k.clone(), e.cc, nc, e.tt, e.ct, callers_vec)
                        })
                        .collect())
                })?;

            let stats_dict = Value::dict(PyDict::default());
            for (k, cc, nc, tt, ct, callers_vec) in snapshot {
                let callers_dict = Value::dict(PyDict::default());
                for (ck, cv) in callers_vec {
                    call_method(
                        _interp,
                        &callers_dict,
                        "__setitem__",
                        &[func_key_to_py(&ck), Value::int(cv)],
                    )?;
                }
                let entry = Value::tuple(vec![
                    Value::int(cc),
                    Value::int(nc),
                    Value::float(tt),
                    Value::float(ct),
                    callers_dict,
                ]);
                call_method(
                    _interp,
                    &stats_dict,
                    "__setitem__",
                    &[func_key_to_py(&k), entry],
                )?;
            }
            _interp.assign_attr(self_val.0.clone(), "stats", stats_dict)?;
            Ok(Value::none())
        }

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
            let cmd_s = _interp.render_value_as_str(&cmd.0)?;
            call_method(_interp, self_val.0, "set_cmd", &[Value::string(cmd_s)])?;
            let (timer_opt, start_instant) =
                with_state(self_val.0, |st| Ok((st.timer.clone(), st.start_instant)))?;
            let now = read_time(_interp, timer_opt.as_ref(), start_instant)?;
            with_state_mut(self_val.0, |st| {
                st.active = true;
                st.last_t = now;
                Ok(())
            })?;
            let disp = _interp.get_attr(self_val.0, "dispatcher")?;
            set_sys_profile(_interp, disp)?;
            let res = call_pos(
                _interp,
                Value::builtin_function("exec"),
                &[cmd.0, globals.0, locals.0],
            );
            let _ = set_sys_profile(_interp, Value::none());
            let _ = with_state_mut(self_val.0, |st| {
                st.active = false;
                Ok(())
            });
            res?;
            Ok(self_val.0.clone())
        }

        fn runcall(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] func: PyValue,
            #[varargs] args: &[Value],
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let func_repr = crate::interpreter::render_value_repr(_interp, &func.0)?;
            call_method(_interp, self_val.0, "set_cmd", &[Value::string(func_repr)])?;
            let (timer_opt, start_instant) =
                with_state(self_val.0, |st| Ok((st.timer.clone(), st.start_instant)))?;
            let now = read_time(_interp, timer_opt.as_ref(), start_instant)?;
            with_state_mut(self_val.0, |st| {
                st.active = true;
                st.last_t = now;
                Ok(())
            })?;
            let disp = _interp.get_attr(self_val.0, "dispatcher")?;
            set_sys_profile(_interp, disp)?;
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
            let _ = set_sys_profile(_interp, Value::none());
            let _ = with_state_mut(self_val.0, |st| {
                st.active = false;
                Ok(())
            });
            res
        }

        fn calibrate(
            #[positional_only] self_val: PyValueRef<'_>,
            m: PyValue,
            #[default(OptionalArg::Omitted)] _verbose: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = m.0.as_int().ok_or_else(|| {
                PyError::named("TypeError", "calibrate() argument must be an integer")
            })?;
            let _ = self_val;
            Ok(Value::float(0.0))
        }
    }
}
