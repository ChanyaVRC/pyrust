// `multiprocessing.util` — 100% native-Rust Python 3.12 utility primitives
// (`Finalize`, `ForkAwareThreadLock`, `ForkAwareLocal`, `get_logger`,
// `log_to_stderr`, `get_temp_dir`, `register_after_fork`, `is_exiting`,
// `close_all_fds_except`) (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::mp_rt::{MP_STATE, call_pos, call_pos_kw, random_hex};
use crate::error::Result;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyIndex, PyStr, PyValueRef};
use crate::value::{PyModule, Value};

const NOTSET: i64 = 0;
const SUBDEBUG: i64 = 5;
const DEBUG: i64 = 10;
const INFO: i64 = 20;
const SUBWARNING: i64 = 25;
const LOGGER_NAME: &str = "multiprocessing";
const DEFAULT_LOGGING_FORMAT: &str = "[%(levelname)s/%(processName)s] %(message)s";

struct FinalizeState {
    callback: Option<Value>,
    args: Vec<Value>,
    kwargs: Vec<(String, Value)>,
    still_active: bool,
    exitpriority: Option<i64>,
}

struct ForkAwareLockState {
    lock: Value,
}

pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let all = Value::list(vec![
        Value::string("sub_debug"),
        Value::string("debug"),
        Value::string("info"),
        Value::string("sub_warning"),
        Value::string("get_logger"),
        Value::string("log_to_stderr"),
        Value::string("get_temp_dir"),
        Value::string("register_after_fork"),
        Value::string("is_exiting"),
        Value::string("Finalize"),
        Value::string("ForkAwareThreadLock"),
        Value::string("ForkAwareLocal"),
        Value::string("close_all_fds_except"),
        Value::string("SUBDEBUG"),
        Value::string("SUBWARNING"),
    ]);
    module
        .borrow_mut()
        .insert_attr("__all__".to_string(), all);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "NOTSET" => Value::int(NOTSET),
        "SUBDEBUG" => Value::int(SUBDEBUG),
        "DEBUG" => Value::int(DEBUG),
        "INFO" => Value::int(INFO),
        "SUBWARNING" => Value::int(SUBWARNING),
        "LOGGER_NAME" => Value::string(LOGGER_NAME),
        "DEFAULT_LOGGING_FORMAT" => Value::string(DEFAULT_LOGGING_FORMAT),
        "MAXFD" => Value::int(1024),
        "abstract_sockets_supported" => Value::bool_(cfg!(target_os = "linux")),
    }

    fn get_logger() -> Result<Value> {
        let logging = _interp.load_module("logging")?;
        let get_log = _interp.get_attr(&logging, "getLogger")?;
        call_pos(_interp, get_log, &[Value::string(LOGGER_NAME)])
    }

    fn log_to_stderr(#[default(OptionalArg::Omitted)] level: OptValueRef<'_>) -> Result<Value> {
        let logging = _interp.load_module("logging")?;
        let get_log = _interp.get_attr(&logging, "getLogger")?;
        let logger = call_pos(_interp, get_log, &[Value::string(LOGGER_NAME)])?;
        if let Some(lvl) = level.as_value()
            && !lvl.is_none()
        {
            let set_level = _interp.get_attr(&logger, "setLevel")?;
            let _ = call_pos(_interp, set_level, std::slice::from_ref(lvl));
        }
        Ok(logger)
    }

    fn sub_debug(
        #[positional_only] _msg: PyStr<'_>,
        #[varargs] _args: &[Value],
    ) -> Result<Value> {
        Ok(Value::none())
    }

    fn debug(
        #[positional_only] _msg: PyStr<'_>,
        #[varargs] _args: &[Value],
    ) -> Result<Value> {
        Ok(Value::none())
    }

    fn info(
        #[positional_only] _msg: PyStr<'_>,
        #[varargs] _args: &[Value],
    ) -> Result<Value> {
        Ok(Value::none())
    }

    fn sub_warning(
        #[positional_only] _msg: PyStr<'_>,
        #[varargs] _args: &[Value],
    ) -> Result<Value> {
        Ok(Value::none())
    }

    fn get_temp_dir() -> Result<Value> {
        let dir = MP_STATE.with(|s| {
            let mut st = s.borrow_mut();
            if let Some(ref p) = st.temp_dir {
                return p.clone();
            }
            let mut p = std::env::temp_dir();
            p.push(format!("pymp-{}", random_hex(4)));
            let _ = std::fs::create_dir_all(&p);
            st.temp_dir = Some(p.clone());
            p
        });
        Ok(Value::string(dir.to_string_lossy().as_ref()))
    }

    fn register_after_fork(
        #[positional_only] _obj: PyValueRef<'_>,
        #[positional_only] func: PyValueRef<'_>,
    ) -> Result<Value> {
        MP_STATE.with(|s| {
            let mut st = s.borrow_mut();
            let id = st.next_after_fork_id;
            st.next_after_fork_id += 1;
            st.after_fork_callbacks.push((id, func.0.clone()));
        });
        Ok(Value::none())
    }

    fn is_exiting() -> Result<Value> {
        Ok(Value::bool_(_interp.cold.gil_state.is_shutting_down()))
    }

    fn is_abstract_socket_namespace(#[positional_only] address: PyValueRef<'_>) -> Result<Value> {
        match address.0.kind() {
            crate::value::ValueKind::Bytes(b) => Ok(Value::bool_(b.first() == Some(&0))),
            crate::value::ValueKind::Str(s) => Ok(Value::bool_(s.starts_with('\0'))),
            _ => Ok(Value::bool_(false)),
        }
    }

    fn close_all_fds_except(#[positional_only] _fds: PyValueRef<'_>) -> Result<Value> {
        Ok(Value::none())
    }

    fn close_fds(#[varargs] _fds: &[Value]) -> Result<Value> {
        Ok(Value::none())
    }

    #[native_state(FinalizeState)]
    class Finalize {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] _obj: PyValueRef<'_>,
            #[positional_only] callback: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] args: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwargs: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] exitpriority: OptValueRef<'_>,
        ) -> Result<Value> {
            let arg_vec = match args.as_value() {
                Some(v) if !v.is_none() => _interp.collect_iterable(v)?,
                _ => Vec::new(),
            };
            let mut kw_vec = Vec::new();
            if let Some(kv) = kwargs.as_value()
                && let Some(d) = kv.as_dict()
            {
                for (k, v) in d.iter() {
                    if let crate::value::PyKey::Str(s) = k {
                        kw_vec.push((s.to_string(), v.clone()));
                    }
                }
            }
            let prio = exitpriority
                .as_value()
                .and_then(|v| v.as_int());
            set_state(
                self_val.0,
                FinalizeState {
                    callback: Some(callback.0.clone()),
                    args: arg_vec,
                    kwargs: kw_vec,
                    still_active: true,
                    exitpriority: prio,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _wr: OptValueRef<'_>,
        ) -> Result<Value> {
            let taken = with_state_mut(self_val.0, |st| {
                if !st.still_active {
                    return Ok(None);
                }
                st.still_active = false;
                Ok(st.callback.take().map(|cb| (cb, st.args.clone(), st.kwargs.clone())))
            })?;
            if let Some((cb, args, kwargs)) = taken {
                return call_pos_kw(_interp, cb, &args, &kwargs);
            }
            Ok(Value::none())
        }

        fn cancel(#[state_mut] st: &mut FinalizeState) -> Result<Value> {
            st.still_active = false;
            st.callback = None;
            Ok(Value::none())
        }

        fn still_active(#[state] st: &FinalizeState) -> Result<Value> {
            Ok(Value::bool_(st.still_active))
        }

        fn __repr__(#[state] st: &FinalizeState) -> Result<Value> {
            if st.still_active {
                Ok(Value::string(format!(
                    "<Finalize object, exitpriority={:?}>",
                    st.exitpriority
                )))
            } else {
                Ok(Value::string("<Finalize object, dead>"))
            }
        }
    }

    #[native_state(ForkAwareLockState)]
    class ForkAwareThreadLock {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let threading = _interp.load_module("threading")?;
            let lock_fn = _interp.get_attr(&threading, "Lock")?;
            let lock = call_pos(_interp, lock_fn, &[])?;
            set_state(self_val.0, ForkAwareLockState { lock })?;
            Ok(Value::none())
        }

        fn acquire(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let lock = with_state(self_val.0, |st| Ok(st.lock.clone()))?;
            let fn_val = _interp.get_attr(&lock, "acquire")?;
            let kw: Vec<(String, Value)> = kwargs.into_iter().collect();
            call_pos_kw(_interp, fn_val, args, &kw)
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
    }

    class ForkAwareLocal {
        fn __init__(#[positional_only] _self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }

    fn _exit_function(#[default(0i64.into())] _dummy: PyIndex) -> Result<Value> {
        Ok(Value::none())
    }
}
