// `_lsprof` module — 100% native Rust implementation of Python 3.12's `_lsprof`
// C-profiler extension backing `cProfile` (issue #3247).
//
// Uses `#[native_state(LsProfState)]`, `pyrust_struct_seq!`, and
// `std::time::Instant` hooked cleanly to PyRust's live `sys.setprofile`.
//
// Reference: <https://docs.python.org/3/library/profile.html>

use std::time::Instant;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyNumber, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{Value, ValueKind};

pyrust_struct_seq! {
    pub(crate) profiler_entry in "_lsprof" {
        fields: [code, callcount, reccallcount, totaltime, inlinetime, calls],
    }
}

pyrust_struct_seq! {
    pub(crate) profiler_subentry in "_lsprof" {
        fields: [code, callcount, reccallcount, totaltime, inlinetime],
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum EntryKey {
    PyFunc(String, i64, String),
    Builtin(String),
}

struct SubEntryStats {
    callee_code: Value,
    callcount: i64,
    reccallcount: i64,
    totaltime: f64,
    inlinetime: f64,
    recursion_level: usize,
}

struct EntryStats {
    code: Value,
    callcount: i64,
    reccallcount: i64,
    totaltime: f64,
    inlinetime: f64,
    recursion_level: usize,
    subcalls: IndexMap<EntryKey, SubEntryStats>,
}

struct ActiveFrame {
    key: EntryKey,
    start_time: f64,
    subtime: f64,
    was_recursive: bool,
    caller_key: Option<EntryKey>,
    sub_was_recursive: bool,
}

struct LsProfState {
    timer: Option<Value>,
    timeunit: f64,
    subcalls: bool,
    builtins: bool,
    enabled: bool,
    start_instant: Instant,
    stack: Vec<ActiveFrame>,
    entries: IndexMap<EntryKey, EntryStats>,
}

impl LsProfState {
    fn new(timer: Option<Value>, timeunit: f64, subcalls: bool, builtins: bool) -> Self {
        Self {
            timer,
            timeunit,
            subcalls,
            builtins,
            enabled: false,
            start_instant: Instant::now(),
            stack: Vec::new(),
            entries: IndexMap::new(),
        }
    }

    fn unit_scale(&self) -> f64 {
        if self.timeunit > 0.0 {
            self.timeunit
        } else {
            1.0
        }
    }

    fn push_call(&mut self, key: EntryKey, code_val: Value, now: f64) {
        let caller_key = self.stack.last().map(|f| f.key.clone());

        // Ensure target entry exists and increment its recursion depth.
        let entry = self
            .entries
            .entry(key.clone())
            .or_insert_with(|| EntryStats {
                code: code_val,
                callcount: 0,
                reccallcount: 0,
                totaltime: 0.0,
                inlinetime: 0.0,
                recursion_level: 0,
                subcalls: IndexMap::new(),
            });
        let callee_code = entry.code.clone();
        let was_recursive = entry.recursion_level > 0;
        entry.recursion_level += 1;

        let mut sub_was_recursive = false;
        if self.subcalls
            && let Some(ref ckey) = caller_key
            && let Some(caller_entry) = self.entries.get_mut(ckey)
        {
            let sub = caller_entry
                .subcalls
                .entry(key.clone())
                .or_insert_with(|| SubEntryStats {
                    callee_code,
                    callcount: 0,
                    reccallcount: 0,
                    totaltime: 0.0,
                    inlinetime: 0.0,
                    recursion_level: 0,
                });
            sub_was_recursive = sub.recursion_level > 0;
            sub.recursion_level += 1;
        }

        self.stack.push(ActiveFrame {
            key,
            start_time: now,
            subtime: 0.0,
            was_recursive,
            caller_key,
            sub_was_recursive,
        });
    }

    fn pop_return(&mut self, expected_key: Option<&EntryKey>, now: f64) {
        let idx = match expected_key {
            Some(ek) => self.stack.iter().rposition(|f| &f.key == ek),
            None => self.stack.len().checked_sub(1),
        };
        let Some(target_idx) = idx else {
            return;
        };
        // Unwind any inner unmatched frames down to target_idx.
        while self.stack.len() > target_idx {
            let Some(frame) = self.stack.pop() else {
                break;
            };
            let dt = (now - frame.start_time).max(0.0);
            let inline = (dt - frame.subtime).max(0.0);

            if let Some(entry) = self.entries.get_mut(&frame.key) {
                entry.callcount += 1;
                if frame.was_recursive {
                    entry.reccallcount += 1;
                } else {
                    entry.totaltime += dt;
                }
                entry.inlinetime += inline;
                entry.recursion_level = entry.recursion_level.saturating_sub(1);
            }

            if let Some(parent) = self.stack.last_mut() {
                parent.subtime += dt;
            }

            if self.subcalls
                && let Some(ref ckey) = frame.caller_key
                && let Some(caller_entry) = self.entries.get_mut(ckey)
                && let Some(sub) = caller_entry.subcalls.get_mut(&frame.key)
            {
                sub.callcount += 1;
                if frame.sub_was_recursive {
                    sub.reccallcount += 1;
                } else {
                    sub.totaltime += dt;
                }
                sub.inlinetime += inline;
                sub.recursion_level = sub.recursion_level.saturating_sub(1);
            }
        }
    }

    fn flush_stack(&mut self, now: f64) {
        while !self.stack.is_empty() {
            self.pop_return(None, now);
        }
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

fn set_sys_profile(interp: &mut Interpreter, func: Value) -> Result<()> {
    let sys_mod = interp.load_module("sys")?;
    let setprofile = interp.get_attr(&sys_mod, "setprofile")?;
    call_pos(interp, setprofile, &[func])?;
    Ok(())
}

fn read_timer_now(
    interp: &mut Interpreter,
    timer_opt: Option<&Value>,
    start_instant: Instant,
    scale: f64,
) -> Result<f64> {
    if let Some(timer_fn) = timer_opt {
        let val = call_pos(interp, timer_fn.clone(), &[])?;
        let num = PyNumber::try_from_value_with_interp(interp, &val, "timer", "timer")?;
        Ok(num.as_f64() * scale)
    } else {
        Ok(start_instant.elapsed().as_secs_f64() * scale)
    }
}

fn py_code_key(interp: &mut Interpreter, frame: &Value) -> Result<(EntryKey, Value)> {
    let code = interp.get_attr(frame, "f_code")?;
    let filename = interp
        .get_attr(&code, "co_filename")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let firstlineno = interp
        .get_attr(&code, "co_firstlineno")
        .ok()
        .and_then(|v| v.as_int())
        .unwrap_or(0);
    let name = interp
        .get_attr(&code, "co_name")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    Ok((EntryKey::PyFunc(filename, firstlineno, name), code))
}

fn builtin_func_label(interp: &mut Interpreter, arg: &Value) -> String {
    match arg.kind() {
        ValueKind::BoundMethod { receiver, function } => {
            let type_name = receiver.borrow().class.borrow().name.clone();
            let mname = function.name.to_string();
            format!("<method '{mname}' of '{type_name}' objects>")
        }
        _ => {
            let name = interp
                .get_attr(arg, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned));
            let module = interp
                .get_attr(arg, "__module__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned));
            match (module.as_deref(), name.as_deref()) {
                (Some("builtins") | None, Some(n)) => format!("<built-in function {n}>"),
                (Some(m), Some(n)) => format!("<built-in function {m}.{n}>"),
                _ => arg.repr_raw(),
            }
        }
    }
}

pyrust_module! {
    constants {
        "profiler_entry" => profiler_entry::class(),
        "profiler_subentry" => profiler_subentry::class(),
    }

    #[native_state(LsProfState)]
    class Profiler {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timer: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeunit: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] subcalls: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] builtins: OptValueRef<'_>,
        ) -> Result<Value> {
            let timer_opt = timer.as_value().filter(|v| !v.is_none()).cloned();
            let unit = match timeunit.as_value().filter(|v| !v.is_none()) {
                Some(v) => PyNumber::try_from_value_with_interp(_interp, v, FN_NAME, "timeunit")?.as_f64(),
                None => 0.0,
            };
            let sub = match subcalls.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => true,
            };
            let bi = match builtins.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => true,
            };
            set_state(self_val.0, LsProfState::new(timer_opt, unit, sub, bi))?;
            Ok(Value::none())
        }

        fn enable(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] subcalls: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] builtins: OptValueRef<'_>,
        ) -> Result<Value> {
            let sub_opt = match subcalls.as_value() {
                Some(v) => Some(_interp.truthy_value(v)?),
                None => None,
            };
            let bi_opt = match builtins.as_value() {
                Some(v) => Some(_interp.truthy_value(v)?),
                None => None,
            };
            with_state_mut(self_val.0, |st| {
                if let Some(s) = sub_opt {
                    st.subcalls = s;
                }
                if let Some(b) = bi_opt {
                    st.builtins = b;
                }
                st.enabled = true;
                Ok(())
            })?;
            let cb = _interp.get_attr(self_val.0, "_profile_callback")?;
            set_sys_profile(_interp, cb)?;
            Ok(Value::none())
        }

        fn disable(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (was_enabled, timer_opt, start_instant, scale) = with_state(self_val.0, |st| {
                Ok((st.enabled, st.timer.clone(), st.start_instant, st.unit_scale()))
            })?;
            if was_enabled {
                set_sys_profile(_interp, Value::none())?;
                let now = read_timer_now(_interp, timer_opt.as_ref(), start_instant, scale)?;
                with_state_mut(self_val.0, |st| {
                    st.enabled = false;
                    st.flush_stack(now);
                    Ok(())
                })?;
            }
            Ok(Value::none())
        }

        fn clear(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                st.entries.clear();
                st.stack.clear();
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn getstats(#[state] st: &LsProfState) -> Result<Value> {
            let mut result = Vec::with_capacity(st.entries.len());
            for entry in st.entries.values() {
                let calls_val = if !st.subcalls || entry.subcalls.is_empty() {
                    Value::none()
                } else {
                    let sub_list: Vec<Value> = entry
                        .subcalls
                        .values()
                        .map(|sub| {
                            profiler_subentry::make([
                                sub.callee_code.clone(),
                                Value::int(sub.callcount),
                                Value::int(sub.reccallcount),
                                Value::float(sub.totaltime),
                                Value::float(sub.inlinetime),
                            ])
                        })
                        .collect();
                    Value::list(sub_list)
                };
                result.push(profiler_entry::make([
                    entry.code.clone(),
                    Value::int(entry.callcount),
                    Value::int(entry.reccallcount),
                    Value::float(entry.totaltime),
                    Value::float(entry.inlinetime),
                    calls_val,
                ]));
            }
            Ok(Value::list(result))
        }

        fn _profile_callback(
            #[positional_only] self_val: PyValueRef<'_>,
            frame: PyValue,
            event: PyStr<'_>,
            arg: PyValue,
        ) -> Result<Value> {
            let (enabled, builtins_on, timer_opt, start_instant, scale) =
                with_state(self_val.0, |st| {
                    Ok((
                        st.enabled,
                        st.builtins,
                        st.timer.clone(),
                        st.start_instant,
                        st.unit_scale(),
                    ))
                })?;
            if !enabled {
                return Ok(Value::none());
            }
            let now = read_timer_now(_interp, timer_opt.as_ref(), start_instant, scale)?;
            match event.0.as_ref() {
                "call" => {
                    let (key, code) = py_code_key(_interp, &frame.0)?;
                    with_state_mut(self_val.0, |st| {
                        st.push_call(key, code, now);
                        Ok(())
                    })?;
                }
                "return" => {
                    let (key, _) = py_code_key(_interp, &frame.0)?;
                    with_state_mut(self_val.0, |st| {
                        st.pop_return(Some(&key), now);
                        Ok(())
                    })?;
                }
                "c_call" if builtins_on => {
                    let label = builtin_func_label(_interp, &arg.0);
                    let key = EntryKey::Builtin(label.clone());
                    with_state_mut(self_val.0, |st| {
                        st.push_call(key, Value::string(label), now);
                        Ok(())
                    })?;
                }
                "c_return" | "c_exception" if builtins_on => {
                    let label = builtin_func_label(_interp, &arg.0);
                    let key = EntryKey::Builtin(label);
                    with_state_mut(self_val.0, |st| {
                        st.pop_return(Some(&key), now);
                        Ok(())
                    })?;
                }
                _ => {}
            }
            Ok(Value::none())
        }
    }
}
