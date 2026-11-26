// `sys.monitoring` namespace (PEP 669, Python 3.12, issue #3246).
//
// Backed by per-interpreter `InterpreterColdState::monitoring_state` so zero
// hot-path cost is incurred unless monitoring tools/events are enabled.
//
// Reference: <https://docs.python.org/3.12/library/sys.monitoring.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyIndex};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, is_builtin_callable_adapter, lookup_class_attr,
    reject_keyword_args_expanded, value_type_name_str,
};
use crate::value::{InstanceAttrs, PyClass, PyDict, PyInstance, PyKey, PyModule, Value, ValueKind};

pub(super) const DEBUGGER_ID: i64 = 0;
pub(super) const COVERAGE_ID: i64 = 1;
pub(super) const PROFILER_ID: i64 = 2;
pub(super) const OPTIMIZER_ID: i64 = 5;

// Python 3.12 PEP 669 event bitmask constants (`Include/internal/pycore_instruments.h`).
pub(super) const NO_EVENTS: i64 = 0;
pub(super) const PY_START: i64 = 1 << 0;
pub(super) const PY_RESUME: i64 = 1 << 1;
pub(super) const PY_RETURN: i64 = 1 << 2;
pub(super) const PY_YIELD: i64 = 1 << 3;
pub(super) const CALL: i64 = 1 << 4;
pub(super) const LINE: i64 = 1 << 5;
pub(super) const INSTRUCTION: i64 = 1 << 6;
pub(super) const JUMP: i64 = 1 << 7;
pub(super) const BRANCH: i64 = 1 << 8;
pub(super) const STOP_ITERATION: i64 = 1 << 9;
pub(super) const RAISE: i64 = 1 << 10;
pub(super) const EXCEPTION_HANDLED: i64 = 1 << 11;
pub(super) const PY_UNWIND: i64 = 1 << 12;
pub(super) const PY_THROW: i64 = 1 << 13;
pub(super) const RERAISE: i64 = 1 << 14;
pub(super) const C_RETURN: i64 = 1 << 15;
pub(super) const C_RAISE: i64 = 1 << 16;

const LOCAL_EVENTS_MASK: i64 = (1 << 10) - 1;
const GLOBAL_SETTABLE_MASK: i64 = (1 << 15) - 1;
const ALL_EVENTS_MASK: i64 = (1 << 17) - 1;
const SYNTHETIC_EVENTS_MASK: i64 = C_RETURN | C_RAISE;

const EVENT_TABLE: &[(&str, i64)] = &[
    ("NO_EVENTS", NO_EVENTS),
    ("PY_START", PY_START),
    ("PY_RESUME", PY_RESUME),
    ("PY_RETURN", PY_RETURN),
    ("PY_YIELD", PY_YIELD),
    ("CALL", CALL),
    ("LINE", LINE),
    ("INSTRUCTION", INSTRUCTION),
    ("JUMP", JUMP),
    ("BRANCH", BRANCH),
    ("STOP_ITERATION", STOP_ITERATION),
    ("RAISE", RAISE),
    ("EXCEPTION_HANDLED", EXCEPTION_HANDLED),
    ("PY_UNWIND", PY_UNWIND),
    ("PY_THROW", PY_THROW),
    ("RERAISE", RERAISE),
    ("C_RETURN", C_RETURN),
    ("C_RAISE", C_RAISE),
];

// Thread-local justification:
// (a) Why `thread_local!`: `Value` is `!Send + !Sync`, so sentinel singletons
//     (`MISSING`, `DISABLE`) and the `events` namespace cannot live in a
//     process-global `OnceLock`.
// (b) Multi-interpreter / multi-threaded impact: `MISSING` and `DISABLE` are
//     immutable identity sentinels safe to share across interpreters on the
//     same thread; mutable tool/event state lives in `InterpreterColdState`.
pyrust_core::gil_local! {
    static MISSING_SENTINEL: Value = make_sentinel();
    static DISABLE_SENTINEL: Value = make_sentinel();
}

fn make_sentinel() -> Value {
    let class = crate::interpreter::object_class_singleton();
    let attrs = InstanceAttrs::new();
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

fn make_events_namespace() -> Value {
    let mut attrs = InstanceAttrs::new();
    for &(name, val) in EVENT_TABLE {
        attrs.insert(name, Value::int(val));
    }
    let mut cls_attrs = IndexMap::new();
    cls_attrs.insert("__module__".to_string(), Value::string("sys.monitoring"));
    let class = Rc::new(RefCell::new(PyClass::new(
        "events", "events", None, cls_attrs,
    )));
    Value::py_instance(Rc::new(RefCell::new(PyInstance { class, attrs })))
}

/// Build the `sys.monitoring` namespace module.
pub(super) fn make_monitoring_module() -> Value {
    let mut attrs = crate::value::ModuleAttrs::default();
    attrs.insert("DEBUGGER_ID".to_string(), Value::int(DEBUGGER_ID));
    attrs.insert("COVERAGE_ID".to_string(), Value::int(COVERAGE_ID));
    attrs.insert("PROFILER_ID".to_string(), Value::int(PROFILER_ID));
    attrs.insert("OPTIMIZER_ID".to_string(), Value::int(OPTIMIZER_ID));
    attrs.insert("MISSING".to_string(), MISSING_SENTINEL.with(Value::clone));
    attrs.insert("DISABLE".to_string(), DISABLE_SENTINEL.with(Value::clone));
    attrs.insert("events".to_string(), make_events_namespace());

    for (name, reg) in [
        ("use_tool_id", "sys.monitoring.use_tool_id"),
        ("free_tool_id", "sys.monitoring.free_tool_id"),
        ("clear_tool_id", "sys.monitoring.clear_tool_id"),
        ("get_tool", "sys.monitoring.get_tool"),
        ("get_events", "sys.monitoring.get_events"),
        ("set_events", "sys.monitoring.set_events"),
        ("get_local_events", "sys.monitoring.get_local_events"),
        ("set_local_events", "sys.monitoring.set_local_events"),
        ("register_callback", "sys.monitoring.register_callback"),
        ("restart_events", "sys.monitoring.restart_events"),
        ("_all_events", "sys.monitoring._all_events"),
    ] {
        attrs.insert(name.to_string(), Value::builtin_function(reg));
    }

    Value::py_module(Rc::new(RefCell::new(PyModule::new(
        "sys.monitoring".to_string(),
        attrs,
    ))))
}

fn parse_tool_id(interp: &mut Interpreter, val: &Value, fn_name: &str) -> Result<usize> {
    let id = PyIndex::try_from_value_with_interp(interp, val, fn_name, "tool_id")?
        .expect_i64(fn_name, "tool_id")?;
    if !(0..=5).contains(&id) {
        return Err(PyError::named(
            "ValueError",
            format!("tool_id must be between 0 and 5, got {id}"),
        ));
    }
    Ok(id as usize)
}

fn require_code_id(val: &Value, fn_name: &str) -> Result<i64> {
    if !pyrust_builtins::code::is_code(val) {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() argument 'code' must be code, not {}",
                value_type_name_str(val)
            ),
        ));
    }
    Ok(val.object_id().as_int().unwrap_or(0))
}

fn value_is_callable(value: &Value) -> bool {
    match value.kind() {
        ValueKind::UserFunction(_)
        | ValueKind::BuiltinFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::PyClass(_) => true,
        ValueKind::BuiltinObject { .. } => is_builtin_callable_adapter(value),
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            lookup_class_attr(&class, "__call__").is_some()
        }
        _ => false,
    }
}

pub(super) fn use_tool_id(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "use_tool_id() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let tid = parse_tool_id(interp, &args[0].value, "use_tool_id")?;
    let name = args[1]
        .value
        .as_str()
        .ok_or_else(|| {
            PyError::named(
                "TypeError",
                format!(
                    "use_tool_id() argument 2 must be str, not {}",
                    value_type_name_str(&args[1].value)
                ),
            )
        })?
        .to_owned();
    let mut st = interp.cold.monitoring_state.borrow_mut();
    if st.tools[tid].is_some() {
        return Err(PyError::named(
            "ValueError",
            format!("tool {tid} is already in use"),
        ));
    }
    st.tools[tid] = Some(name);
    Ok(Value::none())
}

pub(super) fn free_tool_id(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "free_tool_id() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let tid = parse_tool_id(interp, &args[0].value, "free_tool_id")?;
    let mut st = interp.cold.monitoring_state.borrow_mut();
    st.tools[tid] = None;
    Ok(Value::none())
}

pub(super) fn clear_tool_id(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "clear_tool_id() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let tid = parse_tool_id(interp, &args[0].value, "clear_tool_id")?;
    let mut st = interp.cold.monitoring_state.borrow_mut();
    st.global_events[tid] = 0;
    st.local_events[tid].clear();
    st.callbacks[tid].clear();
    Ok(Value::none())
}

pub(super) fn get_tool(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("get_tool() takes exactly 1 argument ({} given)", args.len()),
        ));
    }
    let tid = parse_tool_id(interp, &args[0].value, "get_tool")?;
    let st = interp.cold.monitoring_state.borrow();
    Ok(match &st.tools[tid] {
        Some(name) => Value::string(name),
        None => Value::none(),
    })
}

pub(super) fn get_events(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "get_events() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }
    let tid = parse_tool_id(interp, &args[0].value, "get_events")?;
    Ok(Value::int(
        interp.cold.monitoring_state.borrow().global_events[tid],
    ))
}

pub(super) fn set_events(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "set_events() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let tid = parse_tool_id(interp, &args[0].value, "set_events")?;
    let event_set =
        PyIndex::try_from_value_with_interp(interp, &args[1].value, "set_events", "event_set")?
            .expect_i64("set_events", "event_set")?;
    if event_set & SYNTHETIC_EVENTS_MASK != 0 {
        return Err(PyError::named(
            "ValueError",
            "Cannot set events for C_RETURN and C_RAISE",
        ));
    }
    if event_set < 0 || (event_set & !GLOBAL_SETTABLE_MASK) != 0 {
        return Err(PyError::named("ValueError", "Invalid event set"));
    }
    interp.cold.monitoring_state.borrow_mut().global_events[tid] = event_set;
    Ok(Value::none())
}

pub(super) fn get_local_events(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 2 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "get_local_events() takes exactly 2 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let tid = parse_tool_id(interp, &args[0].value, "get_local_events")?;
    let code_id = require_code_id(&args[1].value, "get_local_events")?;
    let mask = interp.cold.monitoring_state.borrow().local_events[tid]
        .get(&code_id)
        .copied()
        .unwrap_or(0);
    Ok(Value::int(mask))
}

pub(super) fn set_local_events(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "set_local_events() takes exactly 3 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let tid = parse_tool_id(interp, &args[0].value, "set_local_events")?;
    let code_id = require_code_id(&args[1].value, "set_local_events")?;
    let event_set = PyIndex::try_from_value_with_interp(
        interp,
        &args[2].value,
        "set_local_events",
        "event_set",
    )?
    .expect_i64("set_local_events", "event_set")?;
    if event_set < 0 || (event_set & !LOCAL_EVENTS_MASK) != 0 {
        return Err(PyError::named(
            "ValueError",
            "Cannot set non-local events with set_local_events()",
        ));
    }
    let mut st = interp.cold.monitoring_state.borrow_mut();
    if event_set == 0 {
        st.local_events[tid].remove(&code_id);
    } else {
        st.local_events[tid].insert(code_id, event_set);
    }
    Ok(Value::none())
}

pub(super) fn register_callback(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "register_callback() takes exactly 3 arguments ({} given)",
                args.len()
            ),
        ));
    }
    let tid = parse_tool_id(interp, &args[0].value, "register_callback")?;
    let event =
        PyIndex::try_from_value_with_interp(interp, &args[1].value, "register_callback", "event")?
            .expect_i64("register_callback", "event")?;
    if event <= 0 || (event & (event - 1)) != 0 || (event & !ALL_EVENTS_MASK) != 0 {
        return Err(PyError::named(
            "ValueError",
            "Cannot register callback for invalid or multiple events",
        ));
    }
    let func = args[2].value.clone();
    if !func.is_none() && !value_is_callable(&func) {
        return Err(PyError::named(
            "TypeError",
            format!(
                "register_callback() argument 3 must be callable or None, not {}",
                value_type_name_str(&func)
            ),
        ));
    }
    let mut st = interp.cold.monitoring_state.borrow_mut();
    let prev = if func.is_none() {
        st.callbacks[tid].remove(&event)
    } else {
        st.callbacks[tid].insert(event, func)
    };
    Ok(prev.unwrap_or_else(Value::none))
}

pub(super) fn restart_events(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("restart_events() takes no arguments ({} given)", args.len()),
        ));
    }
    Ok(Value::none())
}

pub(super) fn all_events(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if !args.is_empty() {
        return Err(PyError::named(
            "TypeError",
            format!("_all_events() takes no arguments ({} given)", args.len()),
        ));
    }
    let mut d = PyDict::default();
    for &(name, bit) in EVENT_TABLE {
        if bit != 0 {
            d.insert(PyKey::str_from(name), Value::int(bit));
        }
    }
    Ok(Value::dict(d))
}
