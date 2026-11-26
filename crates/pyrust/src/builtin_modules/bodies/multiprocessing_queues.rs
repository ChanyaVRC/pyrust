// `multiprocessing.queues` — 100% native-Rust Python 3.12 cross-process queues
// (`Queue`, `SimpleQueue`, `JoinableQueue`) (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::set_native_state;
use crate::builtin_modules::mp_rt::broker::BrokerEndpoint;
use crate::builtin_modules::mp_rt::queue::{QueueHandle, QueueOpError};
use crate::builtin_modules::mp_rt::{parse_timeout, pickle_dumps, pickle_loads, raise_queue_error};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyStr, PyValueRef,
};
use crate::value::{InstanceAttrs, PyInstance, PyModule, Value, ValueKind};

struct QueueState {
    handle: Arc<QueueHandle>,
    maxsize: i64,
    closed: bool,
    is_joinable: bool,
}

struct SimpleQueueState {
    handle: Arc<QueueHandle>,
    closed: bool,
}

fn map_queue_err(interp: &mut Interpreter, err: QueueOpError) -> PyError {
    match err {
        QueueOpError::Empty => raise_queue_error(interp, "Empty"),
        QueueOpError::Full => raise_queue_error(interp, "Full"),
        QueueOpError::Closed => PyError::named("ValueError", "queue is closed"),
        QueueOpError::TooManyTaskDone => {
            PyError::named("ValueError", "task_done() called too many times")
        }
        QueueOpError::Io(e) => PyError::from_io_error(&e, None),
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let q_mod = interp.load_module("queue")?;
    let empty_cls = interp.get_attr(&q_mod, "Empty")?;
    let full_cls = interp.get_attr(&q_mod, "Full")?;
    let mut m = module.borrow_mut();
    m.insert_attr("Empty".to_string(), empty_cls);
    m.insert_attr("Full".to_string(), full_cls);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("Queue"),
            Value::string("SimpleQueue"),
            Value::string("JoinableQueue"),
        ]),
    }

    fn _rebuild_queue(
        #[positional_only] addr: PyStr<'_>,
        #[positional_only] token: PyBytesLike<'_>,
        #[positional_only] entry_id: PyIndex,
        #[positional_only] maxsize: PyIndex,
        #[positional_only] is_joinable: PyBool,
    ) -> Result<Value> {
        let tok_bytes = token.as_bytes();
        if tok_bytes.len() != 16 {
            return Err(PyError::named("ValueError", "invalid queue token"));
        }
        let mut tok = [0u8; 16];
        tok.copy_from_slice(tok_bytes);
        let ep = BrokerEndpoint {
            addr: addr.0.to_string(),
            token: tok,
            entry_id: entry_id.expect_i64(FN_NAME, "entry_id")? as u64,
        };
        let m = _interp.load_module("multiprocessing.queues")?;
        let cls_name = if is_joinable.0 { "JoinableQueue" } else { "Queue" };
        let cls = _interp.get_attr(&m, cls_name)?;
        let ValueKind::PyClass(cls_rc) = cls.kind() else {
            return Err(PyError::named("TypeError", "Queue class invalid"));
        };
        let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(cls_rc),
            attrs: InstanceAttrs::new(),
        })));
        let ms = maxsize.expect_i64(FN_NAME, "maxsize")?;
        set_native_state(
            &inst,
            QueueState {
                handle: Arc::new(QueueHandle::new_remote(ep)),
                maxsize: ms,
                closed: false,
                is_joinable: is_joinable.0,
            },
        )?;
        Ok(inst)
    }

    fn _rebuild_simple_queue(
        #[positional_only] addr: PyStr<'_>,
        #[positional_only] token: PyBytesLike<'_>,
        #[positional_only] entry_id: PyIndex,
    ) -> Result<Value> {
        let tok_bytes = token.as_bytes();
        if tok_bytes.len() != 16 {
            return Err(PyError::named("ValueError", "invalid simple queue token"));
        }
        let mut tok = [0u8; 16];
        tok.copy_from_slice(tok_bytes);
        let ep = BrokerEndpoint {
            addr: addr.0.to_string(),
            token: tok,
            entry_id: entry_id.expect_i64(FN_NAME, "entry_id")? as u64,
        };
        let m = _interp.load_module("multiprocessing.queues")?;
        let cls = _interp.get_attr(&m, "SimpleQueue")?;
        let ValueKind::PyClass(cls_rc) = cls.kind() else {
            return Err(PyError::named("TypeError", "SimpleQueue class invalid"));
        };
        let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(cls_rc),
            attrs: InstanceAttrs::new(),
        })));
        set_native_state(
            &inst,
            SimpleQueueState {
                handle: Arc::new(QueueHandle::new_remote(ep)),
                closed: false,
            },
        )?;
        Ok(inst)
    }

    #[native_state(QueueState)]
    class Queue {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(0i64.into())] maxsize: PyIndex,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            let ms = maxsize.expect_i64(FN_NAME, "maxsize")?;
            let cap = if ms <= 0 { 0usize } else { ms as usize };
            set_state(
                self_val.0,
                QueueState {
                    handle: Arc::new(QueueHandle::new_local(cap)),
                    maxsize: ms,
                    closed: false,
                    is_joinable: false,
                },
            )?;
            Ok(Value::none())
        }

        fn put(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] obj: PyValueRef<'_>,
            #[default(true.into())] block: PyBool,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let (handle, closed) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.handle), st.closed))
            })?;
            if closed {
                return Err(PyError::named(
                    "ValueError",
                    "Queue is closed",
                ));
            }
            let dur = parse_timeout(timeout.as_value(), true)?;
            let payload = pickle_dumps(_interp, obj.0.clone())?;
            handle
                .put(_interp, payload, block.0, dur)
                .map_err(|e| map_queue_err(_interp, e))?;
            Ok(Value::none())
        }

        fn put_nowait(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] obj: PyValueRef<'_>,
        ) -> Result<Value> {
            let (handle, closed) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.handle), st.closed))
            })?;
            if closed {
                return Err(PyError::named("ValueError", "Queue is closed"));
            }
            let payload = pickle_dumps(_interp, obj.0.clone())?;
            handle
                .put(_interp, payload, false, None)
                .map_err(|e| map_queue_err(_interp, e))?;
            Ok(Value::none())
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(true.into())] block: PyBool,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let (handle, closed) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.handle), st.closed))
            })?;
            if closed {
                return Err(PyError::named("ValueError", "Queue is closed"));
            }
            let dur = parse_timeout(timeout.as_value(), true)?;
            let bytes = handle
                .get(_interp, block.0, dur)
                .map_err(|e| map_queue_err(_interp, e))?;
            pickle_loads(_interp, &bytes)
        }

        fn get_nowait(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (handle, closed) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.handle), st.closed))
            })?;
            if closed {
                return Err(PyError::named("ValueError", "Queue is closed"));
            }
            let bytes = handle
                .get(_interp, false, None)
                .map_err(|e| map_queue_err(_interp, e))?;
            pickle_loads(_interp, &bytes)
        }

        fn qsize(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let n = handle.qsize(_interp)?;
            Ok(Value::int(n as i64))
        }

        fn empty(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let b = handle.empty(_interp)?;
            Ok(Value::bool_(b))
        }

        fn full(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let b = handle.full(_interp)?;
            Ok(Value::bool_(b))
        }

        fn close(#[state_mut] st: &mut QueueState) -> Result<Value> {
            st.closed = true;
            Ok(Value::none())
        }

        fn join_thread(#[state] _st: &QueueState) -> Result<Value> {
            Ok(Value::none())
        }

        fn cancel_join_thread(#[state] _st: &QueueState) -> Result<Value> {
            Ok(Value::none())
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (ep, maxsize, is_joinable) = with_state(self_val.0, |st| {
                let ep = st.handle.export_endpoint()?;
                Ok((ep, st.maxsize, st.is_joinable))
            })?;
            let m = _interp.load_module("multiprocessing.queues")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_queue")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![
                    Value::string(&ep.addr),
                    Value::bytes(ep.token.to_vec()),
                    Value::int(ep.entry_id as i64),
                    Value::int(maxsize),
                    Value::bool_(is_joinable),
                ]),
            ]))
        }
    }

    #[native_state(QueueState)]
    class JoinableQueue : "Queue" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(0i64.into())] maxsize: PyIndex,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            let ms = maxsize.expect_i64(FN_NAME, "maxsize")?;
            let cap = if ms <= 0 { 0usize } else { ms as usize };
            set_state(
                self_val.0,
                QueueState {
                    handle: Arc::new(QueueHandle::new_local(cap)),
                    maxsize: ms,
                    closed: false,
                    is_joinable: true,
                },
            )?;
            Ok(Value::none())
        }

        fn task_done(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            handle
                .task_done(_interp)
                .map_err(|e| map_queue_err(_interp, e))?;
            Ok(Value::none())
        }

        fn join(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            handle.join(_interp)?;
            Ok(Value::none())
        }
    }

    #[native_state(SimpleQueueState)]
    class SimpleQueue {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            set_state(
                self_val.0,
                SimpleQueueState {
                    handle: Arc::new(QueueHandle::new_local(0)),
                    closed: false,
                },
            )?;
            Ok(Value::none())
        }

        fn put(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] obj: PyValueRef<'_>,
        ) -> Result<Value> {
            let (handle, closed) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.handle), st.closed))
            })?;
            if closed {
                return Err(PyError::named("ValueError", "SimpleQueue is closed"));
            }
            let payload = pickle_dumps(_interp, obj.0.clone())?;
            handle
                .put(_interp, payload, true, None)
                .map_err(|e| map_queue_err(_interp, e))?;
            Ok(Value::none())
        }

        fn get(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (handle, closed) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.handle), st.closed))
            })?;
            if closed {
                return Err(PyError::named("ValueError", "SimpleQueue is closed"));
            }
            let bytes = handle
                .get(_interp, true, None)
                .map_err(|e| map_queue_err(_interp, e))?;
            pickle_loads(_interp, &bytes)
        }

        fn empty(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let b = handle.empty(_interp)?;
            Ok(Value::bool_(b))
        }

        fn close(#[state_mut] st: &mut SimpleQueueState) -> Result<Value> {
            st.closed = true;
            Ok(Value::none())
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ep = with_state(self_val.0, |st| st.handle.export_endpoint())?;
            let m = _interp.load_module("multiprocessing.queues")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_simple_queue")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![
                    Value::string(&ep.addr),
                    Value::bytes(ep.token.to_vec()),
                    Value::int(ep.entry_id as i64),
                ]),
            ]))
        }
    }
}
