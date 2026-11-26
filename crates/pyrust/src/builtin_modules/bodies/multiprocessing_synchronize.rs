// `multiprocessing.synchronize` — 100% native-Rust Python 3.12 cross-process
// synchronization primitives (`Lock`, `RLock`, `Semaphore`, `BoundedSemaphore`,
// `Condition`, `Event`, `Barrier`) (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::set_native_state;
use crate::builtin_modules::mp_rt::broker::BrokerEndpoint;
use crate::builtin_modules::mp_rt::sync::{
    RECURSIVE_MUTEX, SEM_VALUE_MAX, SEMAPHORE, SyncCore, SyncError, SyncHandle,
};
use crate::builtin_modules::mp_rt::{call_pos, parse_timeout};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyStr, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyInstance, PyModule, Value, ValueKind};

struct SemWrapState {
    handle: Arc<SyncHandle>,
    kind_name: &'static str,
    kind: i64,
    maxvalue: i64,
}

struct EventState {
    handle: Arc<SyncHandle>,
}

struct ConditionState {
    lock: Value,
    wait_q: Arc<SyncHandle>,
}

struct BarrierState {
    handle: Arc<SyncHandle>,
    action: Option<Value>,
    default_timeout: Option<std::time::Duration>,
}

fn new_instance_of(interp: &mut Interpreter, mod_name: &str, cls_name: &str) -> Result<Value> {
    let m = interp.load_module(mod_name)?;
    let cls = interp.get_attr(&m, cls_name)?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named(
            "TypeError",
            format!("{cls_name} is not a class"),
        ));
    };
    Ok(Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    }))))
}

fn raise_broken_barrier(interp: &mut Interpreter) -> PyError {
    match (|| -> Result<Value> {
        let th = interp.load_module("threading")?;
        let cls = interp.get_attr(&th, "BrokenBarrierError")?;
        call_pos(
            interp,
            cls,
            &[Value::string("Barrier aborted or timed out")],
        )
    })() {
        Ok(inst) => PyError::raised(inst),
        Err(e) => e,
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let _mp = interp.load_module("_multiprocessing")?;
    let sem_cls = interp.get_attr(&_mp, "SemLock")?;
    let sem_unlink = interp.get_attr(&_mp, "sem_unlink")?;
    let mut m = module.borrow_mut();
    m.insert_attr("SemLock".to_string(), sem_cls);
    m.insert_attr("sem_unlink".to_string(), sem_unlink);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("Lock"),
            Value::string("RLock"),
            Value::string("Semaphore"),
            Value::string("BoundedSemaphore"),
            Value::string("Condition"),
            Value::string("Event"),
        ]),
        "RECURSIVE_MUTEX" => Value::int(RECURSIVE_MUTEX),
        "SEMAPHORE" => Value::int(SEMAPHORE),
        "SEM_VALUE_MAX" => Value::int(SEM_VALUE_MAX),
    }

    fn _rebuild_semwrap(
        #[positional_only] cls_name: PyStr<'_>,
        #[positional_only] addr: PyStr<'_>,
        #[positional_only] token: PyBytesLike<'_>,
        #[positional_only] entry_id: PyIndex,
        #[positional_only] kind: PyIndex,
        #[positional_only] maxvalue: PyIndex,
    ) -> Result<Value> {
        let tok_bytes = token.as_bytes();
        if tok_bytes.len() != 16 {
            return Err(PyError::named("ValueError", "invalid sync token"));
        }
        let mut tok = [0u8; 16];
        tok.copy_from_slice(tok_bytes);
        let ep = BrokerEndpoint {
            addr: addr.0.to_string(),
            token: tok,
            entry_id: entry_id.expect_i64(FN_NAME, "entry_id")? as u64,
        };
        let cname: &'static str = match cls_name.0.as_ref() {
            "Lock" => "Lock",
            "RLock" => "RLock",
            "Semaphore" => "Semaphore",
            "BoundedSemaphore" => "BoundedSemaphore",
            _ => "SemLock",
        };
        let inst = new_instance_of(_interp, "multiprocessing.synchronize", cname)?;
        set_native_state(
            &inst,
            SemWrapState {
                handle: Arc::new(SyncHandle::new_remote(ep)),
                kind_name: cname,
                kind: kind.expect_i64(FN_NAME, "kind")?,
                maxvalue: maxvalue.expect_i64(FN_NAME, "maxvalue")?,
            },
        )?;
        Ok(inst)
    }

    fn _rebuild_event(
        #[positional_only] addr: PyStr<'_>,
        #[positional_only] token: PyBytesLike<'_>,
        #[positional_only] entry_id: PyIndex,
    ) -> Result<Value> {
        let tok_bytes = token.as_bytes();
        if tok_bytes.len() != 16 {
            return Err(PyError::named("ValueError", "invalid event token"));
        }
        let mut tok = [0u8; 16];
        tok.copy_from_slice(tok_bytes);
        let ep = BrokerEndpoint {
            addr: addr.0.to_string(),
            token: tok,
            entry_id: entry_id.expect_i64(FN_NAME, "entry_id")? as u64,
        };
        let inst = new_instance_of(_interp, "multiprocessing.synchronize", "Event")?;
        set_native_state(
            &inst,
            EventState {
                handle: Arc::new(SyncHandle::new_remote(ep)),
            },
        )?;
        Ok(inst)
    }

    fn _rebuild_condition(
        #[positional_only] lock: PyValueRef<'_>,
        #[positional_only] addr: PyStr<'_>,
        #[positional_only] token: PyBytesLike<'_>,
        #[positional_only] entry_id: PyIndex,
    ) -> Result<Value> {
        let tok_bytes = token.as_bytes();
        if tok_bytes.len() != 16 {
            return Err(PyError::named("ValueError", "invalid condition token"));
        }
        let mut tok = [0u8; 16];
        tok.copy_from_slice(tok_bytes);
        let ep = BrokerEndpoint {
            addr: addr.0.to_string(),
            token: tok,
            entry_id: entry_id.expect_i64(FN_NAME, "entry_id")? as u64,
        };
        let inst = new_instance_of(_interp, "multiprocessing.synchronize", "Condition")?;
        set_native_state(
            &inst,
            ConditionState {
                lock: lock.0.clone(),
                wait_q: Arc::new(SyncHandle::new_remote(ep)),
            },
        )?;
        Ok(inst)
    }

    fn _rebuild_barrier(
        #[positional_only] addr: PyStr<'_>,
        #[positional_only] token: PyBytesLike<'_>,
        #[positional_only] entry_id: PyIndex,
        #[positional_only] action: PyValueRef<'_>,
        #[positional_only] timeout: PyValueRef<'_>,
    ) -> Result<Value> {
        let tok_bytes = token.as_bytes();
        if tok_bytes.len() != 16 {
            return Err(PyError::named("ValueError", "invalid barrier token"));
        }
        let mut tok = [0u8; 16];
        tok.copy_from_slice(tok_bytes);
        let ep = BrokerEndpoint {
            addr: addr.0.to_string(),
            token: tok,
            entry_id: entry_id.expect_i64(FN_NAME, "entry_id")? as u64,
        };
        let act = if action.0.is_none() {
            None
        } else {
            Some(action.0.clone())
        };
        let dur = parse_timeout(Some(timeout.0), false)?;
        let inst = new_instance_of(_interp, "multiprocessing.synchronize", "Barrier")?;
        set_native_state(
            &inst,
            BarrierState {
                handle: Arc::new(SyncHandle::new_remote(ep)),
                action: act,
                default_timeout: dur,
            },
        )?;
        Ok(inst)
    }

    #[native_state(SemWrapState)]
    class SemLockBase {
        fn acquire(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(true.into())] block: PyBool,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let dur = parse_timeout(timeout.as_value(), false)?;
            let ok = handle.sem_acquire(_interp, block.0, dur)?;
            Ok(Value::bool_(ok))
        }

        fn release(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            handle.sem_release(_interp)?;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let ok = handle.sem_acquire(_interp, true, None)?;
            Ok(Value::bool_(ok))
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            handle.sem_release(_interp)?;
            Ok(Value::none())
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (ep, kind_name, kind, maxvalue) = with_state(self_val.0, |st| {
                let ep = st.handle.export_endpoint()?;
                Ok((ep, st.kind_name, st.kind, st.maxvalue))
            })?;
            let m = _interp.load_module("multiprocessing.synchronize")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_semwrap")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![
                    Value::string(kind_name),
                    Value::string(&ep.addr),
                    Value::bytes(ep.token.to_vec()),
                    Value::int(ep.entry_id as i64),
                    Value::int(kind),
                    Value::int(maxvalue),
                ]),
            ]))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (kind_name, handle) = with_state(self_val.0, |st| {
                Ok((st.kind_name, Arc::clone(&st.handle)))
            })?;
            let val = handle.sem_get_value(_interp).unwrap_or(0);
            Ok(Value::string(format!("<{kind_name}(value={val})>")))
        }
    }

    #[native_state(SemWrapState)]
    class Semaphore : "SemLockBase" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(1i64.into())] value: PyIndex,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            let v = value.expect_i64(FN_NAME, "value")?;
            if v < 0 {
                return Err(PyError::named("ValueError", "semaphore initial value must be >= 0"));
            }
            let core = SyncCore::new_semlock(SEMAPHORE, v, SEM_VALUE_MAX);
            set_state(
                self_val.0,
                SemWrapState {
                    handle: Arc::new(SyncHandle::new_local(core)),
                    kind_name: "Semaphore",
                    kind: SEMAPHORE,
                    maxvalue: SEM_VALUE_MAX,
                },
            )?;
            Ok(Value::none())
        }

        fn get_value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            Ok(Value::int(handle.sem_get_value(_interp)?))
        }
    }

    #[native_state(SemWrapState)]
    class BoundedSemaphore : "Semaphore" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(1i64.into())] value: PyIndex,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            let v = value.expect_i64(FN_NAME, "value")?;
            if v < 0 {
                return Err(PyError::named("ValueError", "semaphore initial value must be >= 0"));
            }
            let core = SyncCore::new_semlock(SEMAPHORE, v, v);
            set_state(
                self_val.0,
                SemWrapState {
                    handle: Arc::new(SyncHandle::new_local(core)),
                    kind_name: "BoundedSemaphore",
                    kind: SEMAPHORE,
                    maxvalue: v,
                },
            )?;
            Ok(Value::none())
        }
    }

    #[native_state(SemWrapState)]
    class Lock : "SemLockBase" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            let core = SyncCore::new_semlock(SEMAPHORE, 1, 1);
            set_state(
                self_val.0,
                SemWrapState {
                    handle: Arc::new(SyncHandle::new_local(core)),
                    kind_name: "Lock",
                    kind: SEMAPHORE,
                    maxvalue: 1,
                },
            )?;
            Ok(Value::none())
        }
    }

    #[native_state(SemWrapState)]
    class RLock : "SemLockBase" {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            let core = SyncCore::new_semlock(RECURSIVE_MUTEX, 1, 1);
            set_state(
                self_val.0,
                SemWrapState {
                    handle: Arc::new(SyncHandle::new_local(core)),
                    kind_name: "RLock",
                    kind: RECURSIVE_MUTEX,
                    maxvalue: 1,
                },
            )?;
            Ok(Value::none())
        }
    }

    #[native_state(EventState)]
    class Event {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            set_state(
                self_val.0,
                EventState {
                    handle: Arc::new(SyncHandle::new_local(SyncCore::new_event())),
                },
            )?;
            Ok(Value::none())
        }

        fn is_set(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            Ok(Value::bool_(handle.event_is_set(_interp)?))
        }

        fn set(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            handle.event_set(_interp)?;
            Ok(Value::none())
        }

        fn clear(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            handle.event_clear(_interp)?;
            Ok(Value::none())
        }

        fn wait(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let dur = parse_timeout(timeout.as_value(), false)?;
            Ok(Value::bool_(handle.event_wait(_interp, dur)?))
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ep = with_state(self_val.0, |st| st.handle.export_endpoint())?;
            let m = _interp.load_module("multiprocessing.synchronize")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_event")?;
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

    #[native_state(ConditionState)]
    class Condition {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] lock: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            let lock_val = match lock.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let m = _interp.load_module("multiprocessing.synchronize")?;
                    let rlock_cls = _interp.get_attr(&m, "RLock")?;
                    call_pos(_interp, rlock_cls, &[])?
                }
            };
            set_state(
                self_val.0,
                ConditionState {
                    lock: lock_val,
                    wait_q: Arc::new(SyncHandle::new_local(SyncCore::new_cond_queue())),
                },
            )?;
            Ok(Value::none())
        }

        fn acquire(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let fn_val = _interp.get_attr(&lock, "acquire")?;
            call_pos(_interp, fn_val, args)
        }

        fn release(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let fn_val = _interp.get_attr(&lock, "release")?;
            call_pos(_interp, fn_val, &[])
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let fn_val = _interp.get_attr(&lock, "__enter__")?;
            call_pos(_interp, fn_val, &[])
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let fn_val = _interp.get_attr(&lock, "__exit__")?;
            call_pos(_interp, fn_val, args)
        }

        fn wait(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let (lock, wait_q) = with_state(self_val.0, |st| {
                Ok((st.lock.clone(), Arc::clone(&st.wait_q)))
            })?;
            let dur = parse_timeout(timeout.as_value(), false)?;
            let rel_fn = _interp.get_attr(&lock, "release")?;
            call_pos(_interp, rel_fn, &[])?;
            let res = wait_q.cond_wait(_interp, dur);
            let acq_fn = _interp.get_attr(&lock, "acquire")?;
            let _ = call_pos(_interp, acq_fn, &[])?;
            Ok(Value::bool_(res?))
        }

        fn wait_for(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] predicate: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let dur = parse_timeout(timeout.as_value(), false)?;
            let deadline = dur.map(|d| Instant::now() + d);
            loop {
                let val = call_pos(_interp, predicate.0.clone(), &[])?;
                if _interp.truthy_value(&val)? {
                    return Ok(val);
                }
                let rem = match deadline {
                    Some(dl) => {
                        let now = Instant::now();
                        if now >= dl {
                            return Ok(val);
                        }
                        Value::float((dl - now).as_secs_f64())
                    }
                    None => Value::none(),
                };
                let wait_fn = _interp.get_attr(self_val.0, "wait")?;
                let _ = call_pos(_interp, wait_fn, &[rem])?;
            }
        }

        fn notify(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(1i64.into())] n: PyIndex,
        ) -> Result<Value> {
            let count = n.expect_i64(FN_NAME, "n")?.max(0) as usize;
            let wait_q = with_state(self_val.0, |st| Ok(Arc::clone(&st.wait_q)))?;
            wait_q.cond_notify(_interp, count)?;
            Ok(Value::none())
        }

        fn notify_all(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let wait_q = with_state(self_val.0, |st| Ok(Arc::clone(&st.wait_q)))?;
            wait_q.cond_notify_all(_interp)?;
            Ok(Value::none())
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (lock, ep) = with_state(self_val.0, |st| {
                let ep = st.wait_q.export_endpoint()?;
                Ok((st.lock.clone(), ep))
            })?;
            let m = _interp.load_module("multiprocessing.synchronize")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_condition")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![
                    lock,
                    Value::string(&ep.addr),
                    Value::bytes(ep.token.to_vec()),
                    Value::int(ep.entry_id as i64),
                ]),
            ]))
        }
    }

    #[native_state(BarrierState)]
    class Barrier {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            parties: PyIndex,
            #[default(OptionalArg::Omitted)] action: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] ctx: OptValueRef<'_>,
        ) -> Result<Value> {
            let _ = ctx;
            let p = parties.expect_i64(FN_NAME, "parties")?;
            if p < 1 {
                return Err(PyError::named("ValueError", "parties must be >= 1"));
            }
            let act = match action.as_value() {
                Some(v) if !v.is_none() => Some(v.clone()),
                _ => None,
            };
            let dur = parse_timeout(timeout.as_value(), false)?;
            set_state(
                self_val.0,
                BarrierState {
                    handle: Arc::new(SyncHandle::new_local(SyncCore::new_barrier(p as usize))),
                    action: act,
                    default_timeout: dur,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn parties(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let (p, _, _) = handle.barrier_info(_interp)?;
            Ok(Value::int(p as i64))
        }

        #[getter]
        fn n_waiting(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let (_, w, _) = handle.barrier_info(_interp)?;
            Ok(Value::int(w as i64))
        }

        #[getter]
        fn broken(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let (_, _, b) = handle.barrier_info(_interp)?;
            Ok(Value::bool_(b))
        }

        fn wait(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let (handle, action, def_to) = with_state(self_val.0, |st| {
                Ok((Arc::clone(&st.handle), st.action.clone(), st.default_timeout))
            })?;
            let dur = match timeout.as_value() {
                None => def_to,
                Some(v) if v.is_none() => def_to,
                Some(v) => parse_timeout(Some(v), false)?,
            };
            let idx = match handle.barrier_wait(_interp, dur) {
                Ok(i) => i,
                Err(SyncError::BrokenBarrier) => return Err(raise_broken_barrier(_interp)),
                Err(SyncError::Io(e)) => return Err(PyError::from_io_error(&e, None)),
                Err(_) => return Err(raise_broken_barrier(_interp)),
            };
            if idx == 0
                && let Some(act) = action
            {
                call_pos(_interp, act, &[])?;
            }
            Ok(Value::int(idx as i64))
        }

        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            handle.barrier_reset(_interp)?;
            Ok(Value::none())
        }

        fn abort(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            handle.barrier_abort(_interp)?;
            Ok(Value::none())
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (ep, action, def_to) = with_state(self_val.0, |st| {
                let ep = st.handle.export_endpoint()?;
                Ok((ep, st.action.clone(), st.default_timeout))
            })?;
            let m = _interp.load_module("multiprocessing.synchronize")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_barrier")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![
                    Value::string(&ep.addr),
                    Value::bytes(ep.token.to_vec()),
                    Value::int(ep.entry_id as i64),
                    action.unwrap_or_else(Value::none),
                    def_to
                        .map(|d| Value::float(d.as_secs_f64()))
                        .unwrap_or_else(Value::none),
                ]),
            ]))
        }
    }
}
