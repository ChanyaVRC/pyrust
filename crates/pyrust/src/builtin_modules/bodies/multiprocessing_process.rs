// `multiprocessing.process` — 100% native-Rust Python 3.12 process objects
// (`BaseProcess`, `AuthenticationString`, `current_process`, `parent_process`,
// `active_children`, `ORIGINAL_DIR`) (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::{
    set_native_state, with_native_state, with_native_state_mut,
};
use crate::builtin_modules::mp_rt::proc::ProcessHandle;
use crate::builtin_modules::mp_rt::{
    MP_STATE, call_pos, call_pos_kw, current_authkey, get_start_method_impl, next_process_number,
    parse_timeout, set_current_authkey,
};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyStr, PyValueRef,
};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, PyModule, Value, ValueKind};

static NEXT_SENTINEL: AtomicI64 = AtomicI64::new(100_000);

pub(crate) struct ProcessState {
    pub target: Option<Value>,
    pub args: Vec<Value>,
    pub kwargs: Vec<(String, Value)>,
    pub name: String,
    pub daemon: bool,
    pub authkey: Vec<u8>,
    pub start_method: Option<String>,
    pub parent_pid: i64,
    pub handle: Option<Arc<ProcessHandle>>,
    pub closed: bool,
    pub is_main: bool,
    pub is_current_child: bool,
    pub fixed_pid: Option<i64>,
    pub sentinel_id: Option<i64>,
}

impl ProcessState {
    fn ensure_not_closed(&self) -> Result<()> {
        if self.closed {
            return Err(PyError::named("ValueError", "process object is closed"));
        }
        Ok(())
    }

    pub(crate) fn effective_pid(&self) -> Option<i64> {
        if self.is_main || self.is_current_child {
            return Some(std::process::id() as i64);
        }
        if let Some(p) = self.fixed_pid {
            return Some(p);
        }
        self.handle.as_ref().map(|h| h.pid)
    }

    pub(crate) fn effective_exitcode(&self) -> Option<i64> {
        if self.is_main || self.is_current_child {
            return None;
        }
        self.handle.as_ref().and_then(|h| h.poll_exitcode())
    }

    pub(crate) fn effective_is_alive(&self) -> bool {
        if self.is_main || self.is_current_child {
            return true;
        }
        self.handle.as_ref().is_some_and(|h| h.is_alive())
    }
}

fn make_auth_string(interp: &mut Interpreter, key: &[u8]) -> Result<Value> {
    let m = interp.load_module("multiprocessing.process")?;
    let cls = interp.get_attr(&m, "AuthenticationString")?;
    call_pos(interp, cls, &[Value::bytes(key.to_vec())])
}

fn extract_bytes_from_val(val: &Value) -> Result<Vec<u8>> {
    if let ValueKind::Bytes(b) = val.kind() {
        return Ok(b.as_slice().to_vec());
    }
    if let Some(backing) = crate::interpreter::builtin_data_backing(val)
        && let ValueKind::Bytes(b) = backing.kind()
    {
        return Ok(b.as_slice().to_vec());
    }
    Err(PyError::named(
        "TypeError",
        "authkey must be a bytes-like object",
    ))
}

/// Ensure `current_process()` is initialized in `MP_STATE` and return it.
pub(crate) fn get_or_init_current_process(interp: &mut Interpreter) -> Result<Value> {
    if let Some(existing) = MP_STATE.with(|s| s.borrow().current_process.clone()) {
        return Ok(existing);
    }
    let m = interp.load_module("multiprocessing.process")?;
    let cls = interp.get_attr(&m, "_MainProcess")?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named("RuntimeError", "_MainProcess invalid"));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        ProcessState {
            target: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            name: "MainProcess".to_string(),
            daemon: false,
            authkey: current_authkey(),
            start_method: None,
            parent_pid: 0,
            handle: None,
            closed: false,
            is_main: true,
            is_current_child: false,
            fixed_pid: None,
            sentinel_id: None,
        },
    )?;
    MP_STATE.with(|s| {
        s.borrow_mut().current_process = Some(inst.clone());
    });
    Ok(inst)
}

/// Create a lightweight parent-process handle for `parent_process()` inside a
/// spawned child process.
pub(crate) fn make_parent_process_proxy(interp: &mut Interpreter, parent_pid: i64) -> Result<Value> {
    let m = interp.load_module("multiprocessing.process")?;
    let cls = interp.get_attr(&m, "_ParentProcess")?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named("RuntimeError", "_ParentProcess invalid"));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        ProcessState {
            target: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            name: "ParentProcess".to_string(),
            daemon: false,
            authkey: current_authkey(),
            start_method: None,
            parent_pid: 0,
            handle: None,
            closed: false,
            is_main: false,
            is_current_child: false,
            fixed_pid: Some(parent_pid),
            sentinel_id: None,
        },
    )?;
    Ok(inst)
}

/// Set `current_process()` and `parent_process()` inside a spawned child.
pub(crate) fn install_child_process_context(
    interp: &mut Interpreter,
    proc_obj: &Value,
    parent_pid: i64,
) -> Result<()> {
    let auth = with_native_state_mut::<ProcessState, _>(proc_obj, "BaseProcess", |st| {
        st.is_current_child = true;
        st.parent_pid = parent_pid;
        Ok(st.authkey.clone())
    })?;
    set_current_authkey(auth);
    let parent_proxy = make_parent_process_proxy(interp, parent_pid)?;
    MP_STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.current_process = Some(proc_obj.clone());
        st.parent_process = Some(parent_proxy);
        st.active_children.clear();
    });
    Ok(())
}

/// Reap finished children and return the list of currently live child processes.
pub(crate) fn cleanup_and_active_children() -> Vec<Value> {
    MP_STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.active_children.retain(|ch| {
            with_native_state::<ProcessState, _>(ch, "BaseProcess", |ps| {
                Ok(ps.effective_is_alive())
            })
            .unwrap_or(false)
        });
        st.active_children.clone()
    })
}

fn original_dir_str() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    _module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("BaseProcess"),
            Value::string("current_process"),
            Value::string("active_children"),
            Value::string("parent_process"),
        ]),
        "ORIGINAL_DIR" => Value::string(original_dir_str()),
    }

    fn current_process() -> Result<Value> {
        get_or_init_current_process(_interp)
    }

    fn parent_process() -> Result<Value> {
        Ok(MP_STATE
            .with(|s| s.borrow().parent_process.clone())
            .unwrap_or_else(Value::none))
    }

    fn active_children() -> Result<Value> {
        Ok(Value::list(cleanup_and_active_children()))
    }

    fn _rebuild_process(
        #[positional_only] cls: PyValueRef<'_>,
        #[positional_only] target: PyValueRef<'_>,
        #[positional_only] args: PyValueRef<'_>,
        #[positional_only] kwargs: PyValueRef<'_>,
        #[positional_only] name: PyStr<'_>,
        #[positional_only] daemon: PyBool,
        #[positional_only] authkey: PyBytesLike<'_>,
        #[positional_only] start_method: PyValueRef<'_>,
        #[positional_only] parent_pid: PyIndex,
        #[positional_only] inst_dict: PyValueRef<'_>,
    ) -> Result<Value> {
        let ValueKind::PyClass(cls_rc) = cls.0.kind() else {
            return Err(PyError::named("TypeError", "invalid process class"));
        };
        let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
            class: Rc::clone(cls_rc),
            attrs: InstanceAttrs::new(),
        })));
        let target_opt = if target.0.is_none() {
            None
        } else {
            Some(target.0.clone())
        };
        let arg_vec = _interp.collect_iterable(args.0)?;
        let mut kw_vec = Vec::new();
        if let Some(d) = kwargs.0.as_dict() {
            for (k, v) in d.iter() {
                if let PyKey::Str(s) = k {
                    kw_vec.push((s.to_string(), v.clone()));
                }
            }
        }
        let sm = start_method.0.as_str().map(str::to_owned);
        set_native_state(
            &inst,
            ProcessState {
                target: target_opt,
                args: arg_vec,
                kwargs: kw_vec,
                name: name.0.to_string(),
                daemon: daemon.0,
                authkey: authkey.as_bytes().to_vec(),
                start_method: sm,
                parent_pid: parent_pid.expect_i64(FN_NAME, "parent_pid")?,
                handle: None,
                closed: false,
                is_main: false,
                is_current_child: false,
                fixed_pid: None,
                sentinel_id: None,
            },
        )?;
        // Restore any custom attributes set by user subclasses of Process.
        if let Some(d) = inst_dict.0.as_dict()
            && let ValueKind::PyInstance(inst_rc) = inst.kind()
        {
            let mut inst_mut = inst_rc.borrow_mut();
            for (k, v) in d.iter() {
                if let PyKey::Str(s) = k {
                    inst_mut.attrs.insert(s.to_string(), v.clone());
                }
            }
        }
        Ok(inst)
    }

    class AuthenticationString : "bytes" {
        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let bytes = extract_bytes_from_val(self_val.0)?;
            let m = _interp.load_module("multiprocessing.process")?;
            let cls = _interp.get_attr(&m, "AuthenticationString")?;
            Ok(Value::tuple(vec![cls, Value::tuple(vec![Value::bytes(bytes)])]))
        }
    }

    #[native_state(ProcessState)]
    class BaseProcess {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] group: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] target: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] args: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwargs: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] daemon: OptValueRef<'_>,
        ) -> Result<Value> {
            if let Some(g) = group.as_value()
                && !g.is_none()
            {
                return Err(PyError::named(
                    "AssertionError",
                    "group argument must be None for now",
                ));
            }
            let target_opt = match target.as_value() {
                Some(v) if !v.is_none() => Some(v.clone()),
                _ => None,
            };
            let arg_vec = match args.as_value() {
                Some(v) if !v.is_none() => _interp.collect_iterable(v)?,
                _ => Vec::new(),
            };
            let mut kw_vec = Vec::new();
            if let Some(kv) = kwargs.as_value()
                && !kv.is_none()
            {
                let Some(d) = kv.as_dict() else {
                    return Err(PyError::named("TypeError", "kwargs must be a dict"));
                };
                for (k, v) in d.iter() {
                    if let PyKey::Str(s) = k {
                        kw_vec.push((s.to_string(), v.clone()));
                    }
                }
            }
            let proc_name = match name.as_value() {
                Some(v) if !v.is_none() => v
                    .as_str()
                    .ok_or_else(|| PyError::named("TypeError", "name must be a str"))?
                    .to_string(),
                _ => {
                    let cls_name = match self_val.0.kind() {
                        ValueKind::PyInstance(rc) => rc.borrow().class.borrow().name.clone(),
                        _ => "Process".to_string(),
                    };
                    format!("{cls_name}-{}", next_process_number())
                }
            };
            let cur = get_or_init_current_process(_interp)?;
            let parent_daemon =
                with_native_state::<ProcessState, _>(&cur, "BaseProcess", |s| Ok(s.daemon))
                    .unwrap_or(false);
            let is_daemon = match daemon.as_value() {
                Some(v) if !v.is_none() => _interp.truthy_value(v)?,
                _ => parent_daemon,
            };
            let sm = _interp
                .get_attr(self_val.0, "_start_method")
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned));

            set_state(
                self_val.0,
                ProcessState {
                    target: target_opt,
                    args: arg_vec,
                    kwargs: kw_vec,
                    name: proc_name,
                    daemon: is_daemon,
                    authkey: current_authkey(),
                    start_method: sm,
                    parent_pid: std::process::id() as i64,
                    handle: None,
                    closed: false,
                    is_main: false,
                    is_current_child: false,
                    fixed_pid: None,
                    sentinel_id: None,
                },
            )?;
            Ok(Value::none())
        }

        fn run(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let call_info = with_state(self_val.0, |st| {
                Ok(st
                    .target
                    .clone()
                    .map(|t| (t, st.args.clone(), st.kwargs.clone())))
            })?;
            if let Some((target, args, kwargs)) = call_info {
                call_pos_kw(_interp, target, &args, &kwargs)?;
            }
            Ok(Value::none())
        }

        fn _bootstrap(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let run_fn = _interp.get_attr(self_val.0, "run")?;
            match call_pos(_interp, run_fn, &[]) {
                Ok(_) => Ok(Value::int(0)),
                Err(err) => {
                    // Handle SystemExit(code) cleanly matching CPython 3.12.
                    if err.class_name_is("SystemExit") {
                        let code = if let pyrust_core::PyErrorKind::Raised(exc_val) = err.kind() {
                            _interp
                                .get_attr(exc_val, "code")
                                .ok()
                                .and_then(|c| {
                                    if c.is_none() {
                                        Some(0)
                                    } else {
                                        c.as_int()
                                    }
                                })
                                .unwrap_or(1)
                        } else {
                            0
                        };
                        return Ok(Value::int(code));
                    }
                    let pname = with_state(self_val.0, |st| Ok(st.name.clone()))
                        .unwrap_or_else(|_| "Process".to_string());
                    eprintln!("Process {pname}:\n{err}");
                    Ok(Value::int(1))
                }
            }
        }

        fn start(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (already_started, is_closed, sm_opt) = with_state(self_val.0, |st| {
                st.ensure_not_closed()?;
                Ok((st.handle.is_some(), st.closed, st.start_method.clone()))
            })?;
            let _ = is_closed;
            if already_started {
                return Err(PyError::named(
                    "AssertionError",
                    "cannot start a process twice",
                ));
            }
            let cur = get_or_init_current_process(_interp)?;
            let cur_is_daemon =
                with_native_state::<ProcessState, _>(&cur, "BaseProcess", |st| Ok(st.daemon))
                    .unwrap_or(false);
            if cur_is_daemon {
                return Err(PyError::named(
                    "AssertionError",
                    "daemonic processes are not allowed to have children",
                ));
            }

            let _ = cleanup_and_active_children();
            let method = sm_opt.unwrap_or_else(|| {
                get_start_method_impl(false)
                    .unwrap_or_else(|| crate::builtin_modules::mp_rt::platform_default_start_method().to_string())
            });

            let handle = match method.as_str() {
                #[cfg(unix)]
                "fork" => ProcessHandle::fork(_interp, self_val.0)?,
                _ => ProcessHandle::spawn(_interp, self_val.0)?,
            };

            let sentinel = NEXT_SENTINEL.fetch_add(1, Ordering::Relaxed);
            with_state_mut(self_val.0, |st| {
                st.handle = Some(handle);
                st.sentinel_id = Some(sentinel);
                Ok(())
            })?;
            MP_STATE.with(|s| {
                s.borrow_mut().active_children.push(self_val.0.clone());
            });
            Ok(Value::none())
        }

        fn join(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let handle = with_state(self_val.0, |st| {
                st.ensure_not_closed()?;
                if st.is_main || st.is_current_child {
                    return Err(PyError::named("AssertionError", "can only join a child process"));
                }
                let Some(ref h) = st.handle else {
                    return Err(PyError::named(
                        "AssertionError",
                        "can only join a started process",
                    ));
                };
                Ok(Arc::clone(h))
            })?;
            let dur = parse_timeout(timeout.as_value(), false)?;
            let res = handle.join(_interp, dur);
            if res.is_some() {
                let _ = cleanup_and_active_children();
            }
            Ok(Value::none())
        }

        fn is_alive(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state(self_val.0, |st| {
                st.ensure_not_closed()?;
                Ok(Value::bool_(st.effective_is_alive()))
            })
        }

        fn terminate(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| {
                st.ensure_not_closed()?;
                Ok(st.handle.as_ref().map(Arc::clone))
            })?;
            if let Some(h) = handle {
                h.terminate()?;
            }
            Ok(Value::none())
        }

        fn kill(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let handle = with_state(self_val.0, |st| {
                st.ensure_not_closed()?;
                Ok(st.handle.as_ref().map(Arc::clone))
            })?;
            if let Some(h) = handle {
                h.kill()?;
            }
            Ok(Value::none())
        }

        fn close(#[state_mut] st: &mut ProcessState) -> Result<Value> {
            if st.closed {
                return Ok(Value::none());
            }
            if st.effective_is_alive() {
                return Err(PyError::named(
                    "ValueError",
                    "Cannot close a process while it is still running. You should first call join() or terminate().",
                ));
            }
            st.handle = None;
            st.closed = true;
            Ok(Value::none())
        }

        #[getter]
        fn name(#[state] st: &ProcessState) -> Result<Value> {
            Ok(Value::string(&st.name))
        }

        #[setter("name")]
        fn set_name(#[state_mut] st: &mut ProcessState, v: PyStr<'_>) -> Result<Value> {
            st.name = v.0.to_string();
            Ok(Value::none())
        }

        #[getter]
        fn daemon(#[state] st: &ProcessState) -> Result<Value> {
            Ok(Value::bool_(st.daemon))
        }

        #[setter("daemon")]
        fn set_daemon(#[state_mut] st: &mut ProcessState, v: PyBool) -> Result<Value> {
            if st.handle.is_some() {
                return Err(PyError::named(
                    "AssertionError",
                    "process has already started",
                ));
            }
            st.daemon = v.0;
            Ok(Value::none())
        }

        #[getter]
        fn authkey(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let key = with_state(self_val.0, |st| Ok(st.authkey.clone()))?;
            make_auth_string(_interp, &key)
        }

        #[setter("authkey")]
        fn set_authkey(
            #[positional_only] self_val: PyValueRef<'_>,
            v: PyValueRef<'_>,
        ) -> Result<Value> {
            let bytes = extract_bytes_from_val(v.0)?;
            with_state_mut(self_val.0, |st| {
                st.authkey = bytes.clone();
                if st.is_main || st.is_current_child {
                    set_current_authkey(bytes);
                }
                Ok(())
            })?;
            Ok(Value::none())
        }

        #[getter]
        fn pid(#[state] st: &ProcessState) -> Result<Value> {
            st.ensure_not_closed()?;
            Ok(st.effective_pid().map(Value::int).unwrap_or_else(Value::none))
        }

        #[getter]
        fn ident(#[state] st: &ProcessState) -> Result<Value> {
            st.ensure_not_closed()?;
            Ok(st.effective_pid().map(Value::int).unwrap_or_else(Value::none))
        }

        #[getter]
        fn exitcode(#[state] st: &ProcessState) -> Result<Value> {
            st.ensure_not_closed()?;
            Ok(st
                .effective_exitcode()
                .map(Value::int)
                .unwrap_or_else(Value::none))
        }

        #[getter]
        fn sentinel(#[state] st: &ProcessState) -> Result<Value> {
            st.ensure_not_closed()?;
            match st.sentinel_id {
                Some(id) => Ok(Value::int(id)),
                None => Err(PyError::named("ValueError", "process not started")),
            }
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (cls_val, inst_dict) = match self_val.0.kind() {
                ValueKind::PyInstance(rc) => {
                    let b = rc.borrow();
                    let d = Value::dict(PyDict::default());
                    for (k, v) in b.attrs.iter() {
                        d.dict_insert(PyKey::str_from(k), v.clone())?;
                    }
                    (Value::py_class(Rc::clone(&b.class)), d)
                }
                _ => return Err(PyError::named("TypeError", "not a Process instance")),
            };
            let (target, args, kwargs, name, daemon, authkey, sm, parent_pid) =
                with_state(self_val.0, |st| {
                    Ok((
                        st.target.clone().unwrap_or_else(Value::none),
                        Value::tuple(st.args.clone()),
                        st.kwargs.clone(),
                        st.name.clone(),
                        st.daemon,
                        st.authkey.clone(),
                        st.start_method
                            .as_deref()
                            .map(Value::string)
                            .unwrap_or_else(Value::none),
                        st.parent_pid,
                    ))
                })?;
            let kw_dict = Value::dict(PyDict::default());
            for (k, v) in kwargs {
                kw_dict.dict_insert(PyKey::str_from(&k), v)?;
            }
            let m = _interp.load_module("multiprocessing.process")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_process")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![
                    cls_val,
                    target,
                    args,
                    kw_dict,
                    Value::string(&name),
                    Value::bool_(daemon),
                    Value::bytes(authkey),
                    sm,
                    Value::int(parent_pid),
                    inst_dict,
                ]),
            ]))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls_name = match self_val.0.kind() {
                ValueKind::PyInstance(rc) => rc.borrow().class.borrow().name.clone(),
                _ => "Process".to_string(),
            };
            with_state(self_val.0, |st| {
                let status = if st.closed {
                    "closed".to_string()
                } else if st.is_main || st.is_current_child {
                    "started".to_string()
                } else if st.handle.is_none() {
                    "initial".to_string()
                } else if let Some(ec) = st.effective_exitcode() {
                    format!("stopped exitcode={ec}")
                } else {
                    "started".to_string()
                };
                let pid_str = st
                    .effective_pid()
                    .map(|p| format!(" pid={p}"))
                    .unwrap_or_default();
                let daemon_str = if st.daemon { " daemon" } else { "" };
                Ok(Value::string(format!(
                    "<{cls_name} name='{}'{pid_str} parent={} {status}{daemon_str}>",
                    st.name, st.parent_pid
                )))
            })
        }
    }

    #[native_state(ProcessState)]
    class _MainProcess : "BaseProcess" {}

    #[native_state(ProcessState)]
    class _ParentProcess : "BaseProcess" {}
}
