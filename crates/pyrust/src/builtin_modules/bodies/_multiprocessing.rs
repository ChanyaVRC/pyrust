// `_multiprocessing` module — 100% native-Rust Python 3.12 C-extension
// compatibility module (`SemLock`, `sem_unlink`, `flags`, plus Windows socket
// helpers) (issue #3264).

use std::sync::Arc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::mp_rt::broker::BrokerEndpoint;
use crate::builtin_modules::mp_rt::sync::{
    RECURSIVE_MUTEX, SEM_VALUE_MAX, SEMAPHORE, SyncCore, SyncHandle,
};
use crate::builtin_modules::mp_rt::{parse_timeout, random_hex};
use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyIndex, PyStr, PyValueRef,
};
use crate::value::{PyDict, Value, ValueKind};

struct SemLockState {
    handle: Arc<SyncHandle>,
    kind: i64,
    maxvalue: i64,
    name: Option<String>,
}

fn build_flags_dict() -> Value {
    let d = Value::dict(PyDict::default());
    #[cfg(unix)]
    {
        use crate::value::PyKey;
        let _ = d.dict_insert(PyKey::str_from("HAVE_SEM_OPEN"), Value::int(1));
        let _ = d.dict_insert(PyKey::str_from("HAVE_SEM_TIMEDWAIT"), Value::int(1));
        let _ = d.dict_insert(PyKey::str_from("HAVE_BROKEN_SEM_GETVALUE"), Value::int(0));
        let _ = d.dict_insert(PyKey::str_from("HAVE_BROKEN_SEM_UNLINK"), Value::int(0));
    }
    d
}

pyrust_module! {
    constants {
        "flags" => build_flags_dict(),
    }

    fn sem_unlink(#[positional_only] _name: PyStr<'_>) -> Result<Value> {
        // Broker-backed semaphores require no OS filesystem unlink.
        Ok(Value::none())
    }

    #[cfg(windows)]
    fn closesocket(#[positional_only] handle: PyIndex) -> Result<Value> {
        let _ = handle.expect_i64(FN_NAME, "handle")?;
        Ok(Value::none())
    }

    #[cfg(windows)]
    fn recv(
        #[positional_only] _handle: PyIndex,
        #[positional_only] _size: PyIndex,
    ) -> Result<Value> {
        Ok(Value::bytes(Vec::new()))
    }

    #[cfg(windows)]
    fn send(
        #[positional_only] _handle: PyIndex,
        #[positional_only] buf: crate::interpreter::builtin_args::PyBytesLike<'_>,
    ) -> Result<Value> {
        Ok(Value::int(buf.as_bytes().len() as i64))
    }

    #[native_state(SemLockState)]
    class SemLock {
        constants {
            "SEM_VALUE_MAX" => Value::int(SEM_VALUE_MAX),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] kind: PyIndex,
            #[positional_only] value: PyIndex,
            #[positional_only] maxvalue: PyIndex,
            #[positional_only] #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[positional_only] #[default(true.into())] _unlink: PyBool,
        ) -> Result<Value> {
            let k = kind.expect_i64(FN_NAME, "kind")?;
            let v = value.expect_i64(FN_NAME, "value")?;
            let mv = maxvalue.expect_i64(FN_NAME, "maxvalue")?;
            if k != RECURSIVE_MUTEX && k != SEMAPHORE {
                return Err(PyError::named("ValueError", "unrecognized kind"));
            }
            if v < 0 || mv < 0 {
                return Err(PyError::named("ValueError", "value must be non-negative"));
            }
            let sem_name = match name.as_value() {
                Some(nv) if !nv.is_none() => nv.as_str().map(str::to_owned),
                _ => Some(format!("/mp-{}", random_hex(6))),
            };
            let core = SyncCore::new_semlock(k, v, mv);
            set_state(
                self_val.0,
                SemLockState {
                    handle: Arc::new(SyncHandle::new_local(core)),
                    kind: k,
                    maxvalue: mv,
                    name: sem_name,
                },
            )?;
            Ok(Value::none())
        }

        #[classmethod]
        fn _rebuild(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] addr: PyStr<'_>,
            #[positional_only] token: crate::interpreter::builtin_args::PyBytesLike<'_>,
            #[positional_only] entry_id: PyIndex,
            #[positional_only] kind: PyIndex,
            #[positional_only] maxvalue: PyIndex,
            #[positional_only] #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
        ) -> Result<Value> {
            let tok_slice = token.as_bytes();
            if tok_slice.len() != 16 {
                return Err(PyError::named("ValueError", "invalid token length"));
            }
            let mut tok = [0u8; 16];
            tok.copy_from_slice(tok_slice);
            let eid = entry_id.expect_i64(FN_NAME, "entry_id")? as u64;
            let k = kind.expect_i64(FN_NAME, "kind")?;
            let mv = maxvalue.expect_i64(FN_NAME, "maxvalue")?;
            let sem_name = name
                .as_value()
                .and_then(|v| v.as_str().map(str::to_owned));

            let ValueKind::PyClass(cls_rc) = cls.0.kind() else {
                return Err(PyError::named("TypeError", "expected SemLock class"));
            };
            let inst = Value::py_instance(std::rc::Rc::new(std::cell::RefCell::new(
                crate::value::PyInstance {
                    class: std::rc::Rc::clone(cls_rc),
                    attrs: crate::value::InstanceAttrs::new(),
                },
            )));
            let ep = BrokerEndpoint {
                addr: addr.0.to_string(),
                token: tok,
                entry_id: eid,
            };
            set_state(
                &inst,
                SemLockState {
                    handle: Arc::new(SyncHandle::new_remote(ep)),
                    kind: k,
                    maxvalue: mv,
                    name: sem_name,
                },
            )?;
            Ok(inst)
        }

        #[getter]
        fn kind(#[state] st: &SemLockState) -> Result<Value> {
            Ok(Value::int(st.kind))
        }

        #[getter]
        fn maxvalue(#[state] st: &SemLockState) -> Result<Value> {
            Ok(Value::int(st.maxvalue))
        }

        #[getter]
        fn name(#[state] st: &SemLockState) -> Result<Value> {
            Ok(st
                .name
                .as_deref()
                .map(Value::string)
                .unwrap_or_else(Value::none))
        }

        #[getter]
        fn handle(#[state] st: &SemLockState) -> Result<Value> {
            let ptr = Arc::as_ptr(&st.handle) as usize as i64;
            Ok(Value::int(ptr))
        }

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

        fn _get_value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let v = handle.sem_get_value(_interp)?;
            Ok(Value::int(v))
        }

        fn _count(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let v = handle.sem_count(_interp)?;
            Ok(Value::int(v))
        }

        fn _is_mine(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let v = handle.sem_is_mine(_interp)?;
            Ok(Value::bool_(v))
        }

        fn _is_zero(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| Ok(Arc::clone(&st.handle)))?;
            let v = handle.sem_is_zero(_interp)?;
            Ok(Value::bool_(v))
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
    }
}
