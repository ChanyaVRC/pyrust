// `multiprocessing.dummy.connection` — 100% native-Rust Python 3.12 in-process
// queue-backed `Pipe`, `Connection`, `Listener`, `Client` (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::set_native_state;
use crate::builtin_modules::mp_rt::call_pos;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyBool, PyIndex, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyInstance, Value, ValueKind};

struct DummyConnState {
    in_q: Option<Value>,
    out_q: Option<Value>,
    closed: bool,
}

struct DummyListenerState {
    backlog_q: Value,
    closed: bool,
}

fn make_dummy_conn(
    interp: &mut Interpreter,
    in_q: Option<Value>,
    out_q: Option<Value>,
) -> Result<Value> {
    let m = interp.load_module("multiprocessing.dummy.connection")?;
    let cls = interp.get_attr(&m, "Connection")?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named("RuntimeError", "dummy Connection invalid"));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        DummyConnState {
            in_q,
            out_q,
            closed: false,
        },
    )?;
    Ok(inst)
}

fn new_thread_queue(interp: &mut Interpreter, maxsize: i64) -> Result<Value> {
    let qm = interp.load_module("queue")?;
    let qcls = interp.get_attr(&qm, "Queue")?;
    if maxsize > 0 {
        call_pos(interp, qcls, &[Value::int(maxsize)])
    } else {
        call_pos(interp, qcls, &[])
    }
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing.dummy"),
        "__all__" => Value::list(vec![
            Value::string("Client"),
            Value::string("Listener"),
            Value::string("Pipe"),
        ]),
        "families" => Value::list(vec![Value::none()]),
    }

    #[py_name = "Pipe"]
    fn pipe(#[default(true.into())] duplex: PyBool) -> Result<Value> {
        let a = new_thread_queue(_interp, 0)?;
        if duplex.0 {
            let b = new_thread_queue(_interp, 0)?;
            let c1 = make_dummy_conn(_interp, Some(a.clone()), Some(b.clone()))?;
            let c2 = make_dummy_conn(_interp, Some(b), Some(a))?;
            Ok(Value::tuple(vec![c1, c2]))
        } else {
            let c1 = make_dummy_conn(_interp, Some(a.clone()), None)?;
            let c2 = make_dummy_conn(_interp, None, Some(a))?;
            Ok(Value::tuple(vec![c1, c2]))
        }
    }

    #[py_name = "Client"]
    fn client(#[positional_only] address: PyValueRef<'_>) -> Result<Value> {
        let a = new_thread_queue(_interp, 0)?;
        let b = new_thread_queue(_interp, 0)?;
        let put_fn = _interp.get_attr(address.0, "put")?;
        call_pos(_interp, put_fn, &[Value::tuple(vec![b.clone(), a.clone()])])?;
        make_dummy_conn(_interp, Some(a), Some(b))
    }

    #[native_state(DummyConnState)]
    class Connection {
        #[getter]
        fn closed(#[state] st: &DummyConnState) -> Result<Value> {
            Ok(Value::bool_(st.closed))
        }

        #[getter]
        fn readable(#[state] st: &DummyConnState) -> Result<Value> {
            Ok(Value::bool_(st.in_q.is_some()))
        }

        #[getter]
        fn writable(#[state] st: &DummyConnState) -> Result<Value> {
            Ok(Value::bool_(st.out_q.is_some()))
        }

        fn send(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] obj: PyValue,
        ) -> Result<Value> {
            let out_q = with_state(self_val.0, |st| {
                if st.closed {
                    return Err(PyError::named("OSError", "handle is closed"));
                }
                st.out_q
                    .clone()
                    .ok_or_else(|| PyError::named("OSError", "connection is read-only"))
            })?;
            let put_fn = _interp.get_attr(&out_q, "put")?;
            call_pos(_interp, put_fn, &[obj.0])
        }

        fn recv(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let in_q = with_state(self_val.0, |st| {
                if st.closed {
                    return Err(PyError::named("OSError", "handle is closed"));
                }
                st.in_q
                    .clone()
                    .ok_or_else(|| PyError::named("OSError", "connection is write-only"))
            })?;
            let get_fn = _interp.get_attr(&in_q, "get")?;
            call_pos(_interp, get_fn, &[])
        }

        fn poll(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let in_q = with_state(self_val.0, |st| {
                if st.closed {
                    return Err(PyError::named("OSError", "handle is closed"));
                }
                st.in_q
                    .clone()
                    .ok_or_else(|| PyError::named("OSError", "connection is write-only"))
            })?;
            let empty_fn = _interp.get_attr(&in_q, "empty")?;
            let empty_val = call_pos(_interp, empty_fn, &[])?;
            let is_empty = _interp.truthy_value(&empty_val)?;
            if !is_empty {
                return Ok(Value::bool_(true));
            }
            if let Some(t) = timeout.as_value()
                && !t.is_none()
            {
                let dur = crate::builtin_modules::mp_rt::parse_timeout(Some(t), false)?;
                if let Some(d) = dur
                    && !d.is_zero()
                {
                    let gil = std::sync::Arc::clone(&_interp.cold.gil_state);
                    _interp.release_gil_during(|| {
                        let _ = gil.sleep_interruptible(d);
                    });
                    let empty_fn2 = _interp.get_attr(&in_q, "empty")?;
                    let empty_val2 = call_pos(_interp, empty_fn2, &[])?;
                    let is_empty2 = _interp.truthy_value(&empty_val2)?;
                    return Ok(Value::bool_(!is_empty2));
                }
            }
            Ok(Value::bool_(false))
        }

        fn close(#[state_mut] st: &mut DummyConnState) -> Result<Value> {
            st.closed = true;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                st.closed = true;
                Ok(())
            })?;
            Ok(Value::none())
        }
    }

    #[native_state(DummyListenerState)]
    class Listener {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _address: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _family: OptValueRef<'_>,
            #[default(1i64.into())] backlog: PyIndex,
        ) -> Result<Value> {
            let bl = backlog.expect_i64(FN_NAME, "backlog")?;
            let q = new_thread_queue(_interp, bl.max(1))?;
            set_state(
                self_val.0,
                DummyListenerState {
                    backlog_q: q,
                    closed: false,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn address(#[state] st: &DummyListenerState) -> Result<Value> {
            Ok(st.backlog_q.clone())
        }

        fn accept(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let q = with_state(self_val.0, |st| {
                if st.closed {
                    return Err(PyError::named("OSError", "listener is closed"));
                }
                Ok(st.backlog_q.clone())
            })?;
            let get_fn = _interp.get_attr(&q, "get")?;
            let pair = call_pos(_interp, get_fn, &[])?;
            let items = _interp.collect_iterable(&pair)?;
            if items.len() != 2 {
                return Err(PyError::named("ValueError", "invalid dummy connection pair"));
            }
            make_dummy_conn(_interp, Some(items[0].clone()), Some(items[1].clone()))
        }

        fn close(#[state_mut] st: &mut DummyListenerState) -> Result<Value> {
            st.closed = true;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                st.closed = true;
                Ok(())
            })?;
            Ok(Value::none())
        }
    }
}
