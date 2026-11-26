// `multiprocessing.dummy` — 100% native-Rust Python 3.12 thread-based drop-in
// mirror of `multiprocessing` backed by real `threading` (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_core::gil_local;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::builtin_modules::mp_rt::{call_pos, call_pos_kw};
use crate::error::Result;
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr};
use crate::value::{PyModule, Value, ValueKind};

gil_local! {
    static DUMMY_CHILDREN: RefCell<Vec<Value>> = RefCell::new(Vec::new());
}

struct DummyValueState {
    value: Value,
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let th = interp.load_module("threading")?;
    let q_mod = interp.load_module("queue")?;
    let conn_mod = interp.load_module("multiprocessing.dummy.connection")?;

    // Link DummyProcess -> threading.Thread
    let thread_cls = interp.get_attr(&th, "Thread")?;
    if let Some(dp) = module.borrow().get_attr_value("DummyProcess")
        && let ValueKind::PyClass(dp_rc) = dp.kind()
        && let ValueKind::PyClass(th_rc) = thread_cls.kind()
    {
        link_class_bases(dp_rc, vec![Rc::clone(th_rc)]);
    }

    let mut m = module.borrow_mut();
    if let Some(dp) = m.get_attr_value("DummyProcess") {
        m.insert_attr("Process".to_string(), dp);
    }
    m.insert_attr("connection".to_string(), conn_mod.clone());
    if let Ok(pipe) = interp.get_attr(&conn_mod, "Pipe") {
        m.insert_attr("Pipe".to_string(), pipe);
    }
    for name in [
        "Lock",
        "RLock",
        "Semaphore",
        "BoundedSemaphore",
        "Condition",
        "Event",
        "Barrier",
    ] {
        if let Ok(v) = interp.get_attr(&th, name) {
            m.insert_attr(name.to_string(), v);
        }
    }
    if let Ok(ct) = interp.get_attr(&th, "current_thread") {
        m.insert_attr("current_process".to_string(), ct);
    }
    if let Ok(q_cls) = interp.get_attr(&q_mod, "Queue") {
        m.insert_attr("Queue".to_string(), q_cls.clone());
        m.insert_attr("JoinableQueue".to_string(), q_cls);
    }
    m.insert_attr("dict".to_string(), Value::builtin_function("dict"));
    m.insert_attr("list".to_string(), Value::builtin_function("list"));
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing.dummy"),
        "__all__" => Value::list(vec![
            Value::string("Process"),
            Value::string("current_process"),
            Value::string("active_children"),
            Value::string("freeze_support"),
            Value::string("Lock"),
            Value::string("RLock"),
            Value::string("Semaphore"),
            Value::string("BoundedSemaphore"),
            Value::string("Condition"),
            Value::string("Event"),
            Value::string("Barrier"),
            Value::string("Queue"),
            Value::string("Manager"),
            Value::string("Pipe"),
            Value::string("Pool"),
            Value::string("JoinableQueue"),
        ]),
    }

    class DummyProcess {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] group: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] target: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] args: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwargs: OptValueRef<'_>,
        ) -> Result<Value> {
            let th = _interp.load_module("threading")?;
            let th_cls = _interp.get_attr(&th, "Thread")?;
            let init_fn = _interp.get_attr(&th_cls, "__init__")?;
            let mut kw = Vec::new();
            if let Some(g) = group.as_value() {
                kw.push(("group".to_string(), g.clone()));
            }
            if let Some(t) = target.as_value() {
                kw.push(("target".to_string(), t.clone()));
            }
            if let Some(n) = name.as_value() {
                kw.push(("name".to_string(), n.clone()));
            }
            if let Some(a) = args.as_value() {
                kw.push(("args".to_string(), a.clone()));
            }
            if let Some(k) = kwargs.as_value() {
                kw.push(("kwargs".to_string(), k.clone()));
            }
            call_pos_kw(_interp, init_fn, std::slice::from_ref(self_val.0), &kw)?;
            _interp.assign_attr(self_val.0.clone(), "_start_called", Value::bool_(false))?;
            Ok(Value::none())
        }

        fn start(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_start_called", Value::bool_(true))?;
            DUMMY_CHILDREN.with(|c| {
                c.borrow_mut().push(self_val.0.clone());
            });
            let th = _interp.load_module("threading")?;
            let th_cls = _interp.get_attr(&th, "Thread")?;
            let start_fn = _interp.get_attr(&th_cls, "start")?;
            call_pos(_interp, start_fn, std::slice::from_ref(self_val.0))
        }

        #[getter]
        fn exitcode(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let started = _interp
                .get_attr(self_val.0, "_start_called")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if !started {
                return Ok(Value::none());
            }
            let alive_fn = _interp.get_attr(self_val.0, "is_alive")?;
            let alive_val = call_pos(_interp, alive_fn, &[])?;
            let alive = _interp.truthy_value(&alive_val)?;
            if alive {
                Ok(Value::none())
            } else {
                Ok(Value::int(0))
            }
        }
    }

    #[native_state(DummyValueState)]
    class Value {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] _typecode: PyValueRef<'_>,
            #[positional_only] value: PyValue,
            #[default(true.into())] _lock: crate::interpreter::builtin_args::PyBool,
        ) -> Result<Value> {
            set_state(self_val.0, DummyValueState { value: value.0 })?;
            Ok(Value::none())
        }

        #[getter]
        fn value(#[state] st: &DummyValueState) -> Result<Value> {
            Ok(st.value.clone())
        }

        #[setter("value")]
        fn set_value(#[state_mut] st: &mut DummyValueState, v: PyValue) -> Result<Value> {
            st.value = v.0;
            Ok(Value::none())
        }
    }

    class Namespace {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varkwargs] kwds: IndexMap<String, Value>,
        ) -> Result<Value> {
            for (k, v) in kwds {
                _interp.assign_attr(self_val.0.clone(), &k, v)?;
            }
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut items: Vec<(String, Value)> = Vec::new();
            if let ValueKind::PyInstance(rc) = self_val.0.kind() {
                for (k, v) in rc.borrow().attrs.iter() {
                    if !k.starts_with('_') {
                        items.push((k.to_string(), v.clone()));
                    }
                }
            }
            items.sort_by(|a, b| a.0.cmp(&b.0));
            let mut parts = Vec::with_capacity(items.len());
            for (k, v) in items {
                parts.push(format!("{k}={}", render_value_repr(_interp, &v)?));
            }
            Ok(Value::string(format!("Namespace({})", parts.join(", "))))
        }
    }

    #[py_name = "Array"]
    fn array_fn(
        #[positional_only] typecode: PyValueRef<'_>,
        #[positional_only] sequence: PyValueRef<'_>,
        #[default(true.into())] _lock: crate::interpreter::builtin_args::PyBool,
    ) -> Result<Value> {
        let arr_mod = _interp.load_module("array")?;
        let arr_cls = _interp.get_attr(&arr_mod, "array")?;
        call_pos(_interp, arr_cls, &[typecode.0.clone(), sequence.0.clone()])
    }

    #[py_name = "Pool"]
    fn pool_fn(
        #[default(OptionalArg::Omitted)] processes: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] initializer: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] initargs: OptValueRef<'_>,
    ) -> Result<Value> {
        let pm = _interp.load_module("multiprocessing.pool")?;
        let tp = _interp.get_attr(&pm, "ThreadPool")?;
        let p = processes.as_value().cloned().unwrap_or_else(Value::none);
        let init = initializer.as_value().cloned().unwrap_or_else(Value::none);
        let ia = initargs
            .as_value()
            .cloned()
            .unwrap_or_else(|| Value::tuple(Vec::new()));
        call_pos(_interp, tp, &[p, init, ia])
    }

    #[py_name = "Manager"]
    fn manager_fn() -> Result<Value> {
        _interp.load_module("multiprocessing.dummy")
    }

    fn active_children() -> Result<Value> {
        let snapshot: Vec<Value> = DUMMY_CHILDREN.with(|c| c.borrow().clone());
        let mut live = Vec::new();
        for ch in snapshot {
            if let Ok(alive_fn) = _interp.get_attr(&ch, "is_alive")
                && let Ok(v) = call_pos(_interp, alive_fn, &[])
                && _interp.truthy_value(&v).unwrap_or(false)
            {
                live.push(ch);
            }
        }
        DUMMY_CHILDREN.with(|c| {
            *c.borrow_mut() = live.clone();
        });
        Ok(Value::list(live))
    }

    fn freeze_support() -> Result<Value> {
        Ok(Value::none())
    }

    fn shutdown() -> Result<Value> {
        Ok(Value::none())
    }
}
