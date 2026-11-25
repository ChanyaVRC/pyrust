// `_tracemalloc` module — 100% native Rust implementation of Python 3.12's
// internal `_tracemalloc` C accelerator module (issue #3246).
//
// Backed by per-interpreter `InterpreterColdState::tracemalloc_state` and the
// cold `VM_HOOK_TRACEMALLOC` bit in `vm_hooks_flags` (zero hot-path overhead
// when tracing is stopped).
//
// Reference: <https://docs.python.org/3.12/library/tracemalloc.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::Value;

pub(crate) fn start_tracing(interp: &mut Interpreter, nframe: i64) -> Result<()> {
    if nframe < 1 {
        return Err(PyError::named(
            "ValueError",
            "the number of frames must be greater than or equal to 1",
        ));
    }
    if nframe > i32::MAX as i64 {
        return Err(PyError::named(
            "ValueError",
            "the number of frames is too large",
        ));
    }
    {
        let mut st = interp.cold.tracemalloc_state.borrow_mut();
        st.tracing = true;
        st.traceback_limit = nframe as usize;
        st.tracked_objects.clear();
        st.ignored_objects.clear();
        st.current_memory = 0;
        st.peak_memory = 0;
    }
    interp.sync_vm_hooks_flags();
    Ok(())
}

pub(crate) fn stop_tracing(interp: &mut Interpreter) {
    {
        let mut st = interp.cold.tracemalloc_state.borrow_mut();
        st.tracing = false;
        st.tracked_objects.clear();
        st.ignored_objects.clear();
        st.current_memory = 0;
        st.peak_memory = 0;
    }
    interp.sync_vm_hooks_flags();
}

pub(crate) fn clear_all_traces(interp: &mut Interpreter) {
    let mut st = interp.cold.tracemalloc_state.borrow_mut();
    let old_keys: Vec<i64> = st.tracked_objects.keys().copied().collect();
    st.ignored_objects.extend(old_keys);
    st.tracked_objects.clear();
    st.current_memory = 0;
    st.peak_memory = 0;
}

pub(crate) fn raw_traces_list(interp: &Interpreter) -> Vec<Value> {
    let st = interp.cold.tracemalloc_state.borrow();
    if !st.tracing {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(st.tracked_objects.len());
    for rec in st.tracked_objects.values() {
        let frames_tuple = Value::tuple(
            rec.frames
                .iter()
                .map(|(fname, lineno)| {
                    Value::tuple(vec![Value::string(fname), Value::int(*lineno)])
                })
                .collect(),
        );
        out.push(Value::tuple(vec![
            Value::int(rec.domain as i64),
            Value::int(rec.size as i64),
            frames_tuple,
            Value::int(rec.total_nframe as i64),
        ]));
    }
    out
}

pub(crate) fn raw_object_traceback(interp: &Interpreter, obj: &Value) -> Value {
    let st = interp.cold.tracemalloc_state.borrow();
    if !st.tracing {
        return Value::none();
    }
    let oid = obj.object_id().as_int().unwrap_or(0);
    let Some(rec) = st.tracked_objects.get(&oid) else {
        return Value::none();
    };
    Value::tuple(
        rec.frames
            .iter()
            .map(|(fname, lineno)| Value::tuple(vec![Value::string(fname), Value::int(*lineno)]))
            .collect(),
    )
}

pub(crate) fn tracemalloc_metadata_bytes(interp: &Interpreter) -> i64 {
    let st = interp.cold.tracemalloc_state.borrow();
    if !st.tracing {
        return 0;
    }
    let per_record = std::mem::size_of::<crate::interpreter::TracemallocRecord>() + 32;
    let frames_bytes: usize = st
        .tracked_objects
        .values()
        .map(|r| r.frames.len() * 40)
        .sum();
    (std::mem::size_of::<crate::interpreter::TracemallocRuntimeState>()
        + st.tracked_objects.len() * per_record
        + frames_bytes) as i64
}

pyrust_module! {
    fn start(#[default(None)] nframe: Option<PyIndex>) -> Result<Value> {
        let limit = match nframe {
            Some(v) => v.expect_i64("start", "nframe")?,
            None => 1,
        };
        start_tracing(_interp, limit)?;
        Ok(Value::none())
    }

    fn stop() -> Result<Value> {
        stop_tracing(_interp);
        Ok(Value::none())
    }

    fn is_tracing() -> Result<Value> {
        Ok(Value::bool_(_interp.cold.tracemalloc_state.borrow().tracing))
    }

    fn clear_traces() -> Result<Value> {
        clear_all_traces(_interp);
        Ok(Value::none())
    }

    fn get_traced_memory() -> Result<Value> {
        let st = _interp.cold.tracemalloc_state.borrow();
        if !st.tracing {
            return Ok(Value::tuple(vec![Value::int(0), Value::int(0)]));
        }
        Ok(Value::tuple(vec![
            Value::int(st.current_memory as i64),
            Value::int(st.peak_memory as i64),
        ]))
    }

    fn reset_peak() -> Result<Value> {
        let mut st = _interp.cold.tracemalloc_state.borrow_mut();
        if st.tracing {
            st.peak_memory = st.current_memory;
        }
        Ok(Value::none())
    }

    fn get_traceback_limit() -> Result<Value> {
        Ok(Value::int(
            _interp.cold.tracemalloc_state.borrow().traceback_limit as i64,
        ))
    }

    fn get_tracemalloc_memory() -> Result<Value> {
        Ok(Value::int(tracemalloc_metadata_bytes(_interp)))
    }

    fn _get_traces() -> Result<Value> {
        Ok(Value::list(raw_traces_list(_interp)))
    }

    fn _get_object_traceback(#[positional_only] obj: PyValue) -> Result<Value> {
        Ok(raw_object_traceback(_interp, &obj.0))
    }
}
