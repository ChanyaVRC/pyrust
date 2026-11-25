// `sched` module — 100% native Rust implementation of Python 3.12's `sched`
// module (issue #3248).
//
// Provides `Event` (namedtuple-compatible `tuple` subclass with fields
// `time`, `priority`, `sequence`, `action`, `argument`, `kwargs`) and
// `scheduler(timefunc=time.monotonic, delayfunc=time.sleep)` with `enterabs`,
// `enter`, `cancel`, `empty`, `run`, and the `queue` property.
//
// Reference: <https://docs.python.org/3/library/sched.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr, value_class};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, Value, ValueKind};

const EVENT_FIELDS: [&str; 6] = [
    "time", "priority", "sequence", "action", "argument", "kwargs",
];

fn event_fields_tuple() -> Value {
    Value::tuple(EVENT_FIELDS.iter().map(|f| Value::string(*f)).collect())
}

fn event_tuple_values(self_val: &Value) -> Result<Vec<Value>> {
    let ValueKind::PyInstance(inst_rc) = self_val.kind() else {
        return Err(PyError::named("TypeError", "expected Event instance"));
    };
    let inst = inst_rc.borrow();
    if let Some(data) = inst.attrs.builtin_data()
        && let ValueKind::Tuple(t) = data.kind()
        && t.len() == 6
    {
        return Ok(t.to_vec());
    }
    Err(PyError::named("TypeError", "invalid Event instance"))
}

fn event_field_at(self_val: &Value, index: usize) -> Result<Value> {
    let vals = event_tuple_values(self_val)?;
    Ok(vals[index].clone())
}

fn make_event_instance(cls: Value, vals: Vec<Value>) -> Result<Value> {
    let ValueKind::PyClass(class_rc) = cls.kind() else {
        return Err(PyError::named(
            "TypeError",
            "Event.__new__ expected a class",
        ));
    };
    let mut attrs = InstanceAttrs::new();
    attrs.set_builtin_data(Value::tuple(vals));
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(class_rc),
        attrs,
    }))))
}

// ── Python-comparison binary min-heap helpers ────────────────────────────────

fn py_lt(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    let res = interp.eval_binary(a.clone(), BinaryOp::Lt, b.clone())?;
    interp.truthy_value(&res)
}

fn py_gt(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    let res = interp.eval_binary(a.clone(), BinaryOp::Gt, b.clone())?;
    interp.truthy_value(&res)
}

fn py_eq(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    let res = interp.eval_binary(a.clone(), BinaryOp::Eq, b.clone())?;
    interp.truthy_value(&res)
}

fn sift_down(
    interp: &mut Interpreter,
    heap: &mut [Value],
    start_pos: usize,
    mut pos: usize,
) -> Result<()> {
    let new_item = heap[pos].clone();
    while pos > start_pos {
        let parent_pos = (pos - 1) >> 1;
        if py_lt(interp, &new_item, &heap[parent_pos])? {
            heap[pos] = heap[parent_pos].clone();
            pos = parent_pos;
        } else {
            break;
        }
    }
    heap[pos] = new_item;
    Ok(())
}

fn sift_up(interp: &mut Interpreter, heap: &mut [Value], mut pos: usize) -> Result<()> {
    let end_pos = heap.len();
    let start_pos = pos;
    let new_item = heap[pos].clone();
    let mut child_pos = 2 * pos + 1;
    while child_pos < end_pos {
        let right_pos = child_pos + 1;
        if right_pos < end_pos && !py_lt(interp, &heap[child_pos], &heap[right_pos])? {
            child_pos = right_pos;
        }
        heap[pos] = heap[child_pos].clone();
        pos = child_pos;
        child_pos = 2 * pos + 1;
    }
    heap[pos] = new_item;
    sift_down(interp, heap, start_pos, pos)
}

fn heap_push(interp: &mut Interpreter, heap: &mut Vec<Value>, item: Value) -> Result<()> {
    heap.push(item);
    let last = heap.len() - 1;
    sift_down(interp, heap, 0, last)
}

fn heap_pop(interp: &mut Interpreter, heap: &mut Vec<Value>) -> Result<Value> {
    let last = heap
        .pop()
        .ok_or_else(|| PyError::named("IndexError", "index out of range"))?;
    if !heap.is_empty() {
        let return_item = std::mem::replace(&mut heap[0], last);
        sift_up(interp, heap, 0)?;
        return Ok(return_item);
    }
    Ok(last)
}

fn heapify(interp: &mut Interpreter, heap: &mut [Value]) -> Result<()> {
    let n = heap.len();
    for i in (0..n / 2).rev() {
        sift_up(interp, heap, i)?;
    }
    Ok(())
}

struct SchedulerState {
    queue: Vec<Value>,
    next_seq: i64,
}

fn call_action(
    interp: &mut Interpreter,
    action: Value,
    argument: &Value,
    kwargs: &Value,
) -> Result<Value> {
    let pos_items = interp.collect_iterable(argument)?;
    let mut call_args: Vec<ExpandedCallArg> = pos_items
        .into_iter()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    if let Some(dict) = kwargs.as_dict() {
        for (k, v) in dict.iter() {
            let key_str = match k {
                PyKey::Str(sv) => sv.as_str(),
                _ => None,
            }
            .ok_or_else(|| PyError::named("TypeError", "keywords must be strings"))?;
            call_args.push(ExpandedCallArg {
                name: Some(key_str.to_owned()),
                value: v.clone(),
            });
        }
    } else if !kwargs.is_none() {
        // Support custom mapping kwargs via .keys() / []
        let keys_fn = interp.get_attr(kwargs, "keys")?;
        let keys_val = interp.call_function_expanded(keys_fn, &[])?;
        let keys = interp.collect_iterable(&keys_val)?;
        for key in keys {
            let Some(kstr) = key.as_str().map(str::to_owned) else {
                return Err(PyError::named("TypeError", "keywords must be strings"));
            };
            let v = interp.eval_index(kwargs, key)?;
            call_args.push(ExpandedCallArg {
                name: Some(kstr),
                value: v,
            });
        }
    }
    interp.call_function_expanded(action, &call_args)
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![Value::string("scheduler")]),
    }

    class Event("tuple") {
        constants {
            "_fields" => event_fields_tuple(),
            "__match_args__" => event_fields_tuple(),
        }

        fn __new__(
            #[positional_only] cls: PyValueRef<'_>,
            time: PyValue,
            priority: PyValue,
            sequence: PyValue,
            action: PyValue,
            argument: PyValue,
            kwargs: PyValue,
        ) -> Result<Value> {
            make_event_instance(
                cls.0.clone(),
                vec![time.0, priority.0, sequence.0, action.0, argument.0, kwargs.0],
            )
        }

        #[getter]
        fn time(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            event_field_at(self_val.0, 0)
        }

        #[getter]
        fn priority(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            event_field_at(self_val.0, 1)
        }

        #[getter]
        fn sequence(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            event_field_at(self_val.0, 2)
        }

        #[getter]
        fn action(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            event_field_at(self_val.0, 3)
        }

        #[getter]
        fn argument(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            event_field_at(self_val.0, 4)
        }

        #[getter]
        fn kwargs(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            event_field_at(self_val.0, 5)
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let vals = event_tuple_values(self_val.0)?;
            let cls = value_class(self_val.0);
            let name = _interp
                .get_attr(&cls, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "Event".to_string());
            let mut parts = Vec::with_capacity(6);
            for (field, val) in EVENT_FIELDS.iter().zip(vals.iter()) {
                let r = render_value_repr(_interp, val)?;
                parts.push(format!("{field}={r}"));
            }
            Ok(Value::string(format!("{name}({})", parts.join(", "))))
        }

        #[classmethod]
        fn _make(#[positional_only] cls: PyValueRef<'_>, iterable: PyValue) -> Result<Value> {
            let vals = _interp.collect_iterable(&iterable.0)?;
            if vals.len() != 6 {
                return Err(PyError::named(
                    "TypeError",
                    format!("Expected 6 arguments, got {}", vals.len()),
                ));
            }
            let call_args: Vec<ExpandedCallArg> = vals
                .into_iter()
                .map(|value| ExpandedCallArg { name: None, value })
                .collect();
            _interp.call_function_expanded(cls.0.clone(), &call_args)
        }

        fn _asdict(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let vals = event_tuple_values(self_val.0)?;
            let mut d = PyDict::default();
            for (field, val) in EVENT_FIELDS.iter().zip(vals.into_iter()) {
                d.insert(PyKey::str_from(field), val);
            }
            Ok(Value::dict(d))
        }

        fn _replace(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varkwargs] kwds: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut vals = event_tuple_values(self_val.0)?;
            for (k, v) in kwds {
                if let Some(pos) = EVENT_FIELDS.iter().position(|f| *f == k) {
                    vals[pos] = v;
                } else {
                    return Err(PyError::named(
                        "ValueError",
                        format!("Got unexpected field names: [{k:?}]"),
                    ));
                }
            }
            let cls = value_class(self_val.0);
            let call_args: Vec<ExpandedCallArg> = vals
                .into_iter()
                .map(|value| ExpandedCallArg { name: None, value })
                .collect();
            _interp.call_function_expanded(cls, &call_args)
        }
    }

    #[native_state(SchedulerState)]
    class scheduler {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timefunc: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] delayfunc: OptValueRef<'_>,
        ) -> Result<Value> {
            let time_mod = _interp.load_module("time")?;
            let tf = match timefunc.as_value() {
                Some(v) => v.clone(),
                None => _interp.get_attr(&time_mod, "monotonic")?,
            };
            let df = match delayfunc.as_value() {
                Some(v) => v.clone(),
                None => _interp.get_attr(&time_mod, "sleep")?,
            };
            _interp.assign_attr(self_val.0.clone(), "timefunc", tf)?;
            _interp.assign_attr(self_val.0.clone(), "delayfunc", df)?;
            set_state(
                self_val.0,
                SchedulerState {
                    queue: Vec::new(),
                    next_seq: 0,
                },
            )?;
            Ok(Value::none())
        }

        fn enterabs(
            #[positional_only] self_val: PyValueRef<'_>,
            time: PyValue,
            priority: PyValue,
            action: PyValue,
            #[default(OptionalArg::Omitted)] argument: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwargs: OptValueRef<'_>,
        ) -> Result<Value> {
            let arg_val = argument
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::tuple(Vec::new()));
            let kw_val = kwargs
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            let seq = with_state_mut(self_val.0, |st| {
                let s = st.next_seq;
                st.next_seq += 1;
                Ok(s)
            })?;
            let sched_mod = _interp.load_module("sched")?;
            let event_cls = _interp.get_attr(&sched_mod, "Event")?;
            let event = make_event_instance(
                event_cls,
                vec![time.0, priority.0, Value::int(seq), action.0, arg_val, kw_val],
            )?;
            // Take queue out briefly so Python `__lt__` callbacks during heap
            // sift cannot trigger a reentrant RefCell borrow on SchedulerState.
            let mut heap = with_state_mut(self_val.0, |st| Ok(std::mem::take(&mut st.queue)))?;
            let push_res = heap_push(_interp, &mut heap, event.clone());
            let _ = with_state_mut(self_val.0, |st| {
                st.queue = heap;
                Ok(())
            });
            push_res?;
            Ok(event)
        }

        fn enter(
            #[positional_only] self_val: PyValueRef<'_>,
            delay: PyValue,
            priority: PyValue,
            action: PyValue,
            #[default(OptionalArg::Omitted)] argument: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwargs: OptValueRef<'_>,
        ) -> Result<Value> {
            let timefunc = _interp.get_attr(self_val.0, "timefunc")?;
            let now = _interp.call_function_expanded(timefunc, &[])?;
            let abs_time = _interp.eval_binary(now, BinaryOp::Add, delay.0)?;
            let enterabs_fn = _interp.get_attr(self_val.0, "enterabs")?;
            let mut call_args = vec![
                ExpandedCallArg { name: None, value: abs_time },
                ExpandedCallArg { name: None, value: priority.0 },
                ExpandedCallArg { name: None, value: action.0 },
            ];
            if let Some(a) = argument.as_value() {
                call_args.push(ExpandedCallArg { name: None, value: a.clone() });
            } else if kwargs.as_value().is_some() {
                call_args.push(ExpandedCallArg { name: None, value: Value::tuple(Vec::new()) });
            }
            if let Some(kw) = kwargs.as_value() {
                call_args.push(ExpandedCallArg { name: None, value: kw.clone() });
            }
            _interp.call_function_expanded(enterabs_fn, &call_args)
        }

        fn cancel(#[positional_only] self_val: PyValueRef<'_>, event: PyValue) -> Result<Value> {
            let mut heap = with_state_mut(self_val.0, |st| Ok(std::mem::take(&mut st.queue)))?;
            let mut found_idx = None;
            for (idx, item) in heap.iter().enumerate() {
                match py_eq(_interp, item, &event.0) {
                    Ok(true) => {
                        found_idx = Some(idx);
                        break;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        let _ = with_state_mut(self_val.0, |st| {
                            st.queue = heap;
                            Ok(())
                        });
                        return Err(e);
                    }
                }
            }
            let res = if let Some(idx) = found_idx {
                heap.remove(idx);
                heapify(_interp, &mut heap)
            } else {
                Err(PyError::named("ValueError", "list.remove(x): x not in list"))
            };
            let _ = with_state_mut(self_val.0, |st| {
                st.queue = heap;
                Ok(())
            });
            res?;
            Ok(Value::none())
        }

        fn empty(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state(self_val.0, |st| Ok(Value::bool_(st.queue.is_empty())))
        }

        fn run(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] blocking: OptValueRef<'_>,
        ) -> Result<Value> {
            let blocking = match blocking.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => true,
            };
            let delayfunc = _interp.get_attr(self_val.0, "delayfunc")?;
            let timefunc = _interp.get_attr(self_val.0, "timefunc")?;
            loop {
                let top = with_state(self_val.0, |st| Ok(st.queue.first().cloned()))?;
                let Some(top_event) = top else {
                    break;
                };
                let vals = event_tuple_values(&top_event)?;
                let event_time = vals[0].clone();
                let now = _interp.call_function_expanded(timefunc.clone(), &[])?;
                if py_gt(_interp, &event_time, &now)? {
                    let delta = _interp.eval_binary(event_time, BinaryOp::Sub, now)?;
                    if !blocking {
                        return Ok(delta);
                    }
                    _interp.call_function_expanded(
                        delayfunc.clone(),
                        &[ExpandedCallArg { name: None, value: delta }],
                    )?;
                } else {
                    let mut heap =
                        with_state_mut(self_val.0, |st| Ok(std::mem::take(&mut st.queue)))?;
                    let popped = heap_pop(_interp, &mut heap);
                    let _ = with_state_mut(self_val.0, |st| {
                        st.queue = heap;
                        Ok(())
                    });
                    let ev = popped?;
                    let ev_vals = event_tuple_values(&ev)?;
                    call_action(_interp, ev_vals[3].clone(), &ev_vals[4], &ev_vals[5])?;
                    _interp.call_function_expanded(
                        delayfunc.clone(),
                        &[ExpandedCallArg {
                            name: None,
                            value: Value::int(0),
                        }],
                    )?;
                }
            }
            Ok(Value::none())
        }

        #[getter]
        fn queue(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut copy = with_state(self_val.0, |st| Ok(st.queue.clone()))?;
            let mut ordered = Vec::with_capacity(copy.len());
            while !copy.is_empty() {
                ordered.push(heap_pop(_interp, &mut copy)?);
            }
            Ok(Value::list(ordered))
        }
    }
}
