// `multiprocessing.connection` — 100% native-Rust Python 3.12 IPC connections
// (`Pipe`, `Connection`, `Listener`, `Client`, `wait`, `deliver_challenge`,
// `answer_challenge`, `address_type`, `arbitrary_address`, `default_family`)
// (issue #3264).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::{
    set_native_state, with_native_state,
};
use crate::builtin_modules::mp_rt::broker::BrokerEndpoint;
use crate::builtin_modules::mp_rt::ipc::{
    ConnectionCore, ListenerAddress, ListenerCore, address_type_str, answer_challenge_core,
    arbitrary_address_str, client_connect, default_family_str, deliver_challenge_core,
};
use crate::builtin_modules::mp_rt::{
    SHUTDOWN_POLL_SLICE, call_pos, parse_timeout, pickle_dumps, pickle_loads,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyBytesLike, PyIndex, PyStr, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, acquire_writable_buffer, current_thread_is_daemon_worker,
};
use crate::value::{InstanceAttrs, PyInstance, PyModule, Value, ValueKind};

pub(crate) struct ConnectionState {
    pub core: Arc<ConnectionCore>,
}

struct ListenerState {
    core: Arc<ListenerCore>,
}

fn wrap_connection(interp: &mut Interpreter, core: Arc<ConnectionCore>) -> Result<Value> {
    let m = interp.load_module("multiprocessing.connection")?;
    let cls = interp.get_attr(&m, "Connection")?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named("TypeError", "Connection class invalid"));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(&inst, ConnectionState { core })?;
    Ok(inst)
}

fn parse_address(val: &Value) -> Result<ListenerAddress> {
    match val.kind() {
        ValueKind::Tuple(items) if items.len() == 2 => {
            let host = items[0]
                .as_str()
                .ok_or_else(|| PyError::named("TypeError", "address host must be str"))?
                .to_string();
            let port = items[1]
                .as_int()
                .ok_or_else(|| PyError::named("TypeError", "address port must be int"))?;
            if !(0..=65535).contains(&port) {
                return Err(PyError::named("ValueError", "port out of range"));
            }
            Ok(ListenerAddress::Inet(host, port as u16))
        }
        ValueKind::Str(s) => Ok(ListenerAddress::Named(s.to_string())),
        ValueKind::Bytes(b) => Ok(ListenerAddress::Named(
            String::from_utf8_lossy(b).into_owned(),
        )),
        _ => Err(PyError::named(
            "TypeError",
            "address must be a (host, port) tuple or string",
        )),
    }
}

fn address_to_py(addr: &ListenerAddress) -> Value {
    match addr {
        ListenerAddress::Inet(h, p) => Value::tuple(vec![Value::string(h), Value::int(*p as i64)]),
        ListenerAddress::Named(s) => Value::string(s),
    }
}

fn parse_authkey(val: Option<&Value>) -> Result<Option<Vec<u8>>> {
    match val {
        None => Ok(None),
        Some(v) if v.is_none() => Ok(None),
        Some(v) => match v.kind() {
            ValueKind::Bytes(b) => Ok(Some(b.as_slice().to_vec())),
            _ => {
                if let Some(backing) = crate::interpreter::builtin_data_backing(v)
                    && let ValueKind::Bytes(b) = backing.kind()
                {
                    return Ok(Some(b.as_slice().to_vec()));
                }
                Err(PyError::named("TypeError", "authkey should be a byte string"))
            }
        },
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let ctx_mod = interp.load_module("multiprocessing.context")?;
    let buf_short = interp.get_attr(&ctx_mod, "BufferTooShort")?;
    let auth_err = interp.get_attr(&ctx_mod, "AuthenticationError")?;
    let mut m = module.borrow_mut();
    m.insert_attr("BufferTooShort".to_string(), buf_short);
    m.insert_attr("AuthenticationError".to_string(), auth_err);
    let conn_cls = m.get_attr_value("Connection");
    if let Some(c) = conn_cls {
        m.insert_attr("PipeConnection".to_string(), c);
    }
    let listener_cls = m.get_attr_value("Listener");
    if let Some(l) = listener_cls {
        m.insert_attr("SocketListener".to_string(), l.clone());
        m.insert_attr("PipeListener".to_string(), l);
    }
    let client_fn = m.get_attr_value("Client");
    if let Some(cl) = client_fn {
        m.insert_attr("SocketClient".to_string(), cl.clone());
        m.insert_attr("PipeClient".to_string(), cl);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("multiprocessing"),
        "__all__" => Value::list(vec![
            Value::string("Client"),
            Value::string("Listener"),
            Value::string("Pipe"),
            Value::string("wait"),
        ]),
        "BUFSIZE" => Value::int(8192),
        "CONNECTION_TIMEOUT" => Value::float(20.0),
        "MESSAGE_LENGTH" => Value::int(20),
        "default_family" => Value::string(default_family_str()),
    }

    fn address_type(#[positional_only] address: PyValueRef<'_>) -> Result<Value> {
        match address.0.kind() {
            ValueKind::Tuple(_) => Ok(Value::string("AF_INET")),
            ValueKind::Str(s) => Ok(Value::string(address_type_str(s))),
            ValueKind::Bytes(b) => {
                let s = String::from_utf8_lossy(b);
                Ok(Value::string(address_type_str(&s)))
            }
            _ => Err(PyError::named("ValueError", "address type of argument not recognized")),
        }
    }

    fn arbitrary_address(
        #[positional_only] #[default(OptionalArg::Omitted)] family: OptValueRef<'_>,
    ) -> Result<Value> {
        let fam = family
            .as_value()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| default_family_str().to_string());
        if fam == "AF_INET" {
            return Ok(Value::tuple(vec![Value::string("localhost"), Value::int(0)]));
        }
        Ok(Value::string(arbitrary_address_str(&fam)))
    }

    #[py_name = "Pipe"]
    fn pipe(#[default(true.into())] duplex: PyBool) -> Result<Value> {
        let (c1, c2) =
            ConnectionCore::make_pipe(duplex.0).map_err(|e| PyError::from_io_error(&e, None))?;
        let py1 = wrap_connection(_interp, c1)?;
        let py2 = wrap_connection(_interp, c2)?;
        Ok(Value::tuple(vec![py1, py2]))
    }

    fn _rebuild_connection(
        #[positional_only] addr: PyStr<'_>,
        #[positional_only] token: PyBytesLike<'_>,
        #[positional_only] entry_id: PyIndex,
        #[positional_only] readable: PyBool,
        #[positional_only] writable: PyBool,
    ) -> Result<Value> {
        let tok_bytes = token.as_bytes();
        if tok_bytes.len() != 16 {
            return Err(PyError::named("ValueError", "invalid connection token"));
        }
        let mut tok = [0u8; 16];
        tok.copy_from_slice(tok_bytes);
        let ep = BrokerEndpoint {
            addr: addr.0.to_string(),
            token: tok,
            entry_id: entry_id.expect_i64(FN_NAME, "entry_id")? as u64,
        };
        let core = ConnectionCore::connect_from_endpoint(&ep, readable.0, writable.0)?;
        wrap_connection(_interp, core)
    }

    #[py_name = "Client"]
    fn client(
        address: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] _family: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] authkey: OptValueRef<'_>,
    ) -> Result<Value> {
        let addr = parse_address(address.0)?;
        let key = parse_authkey(authkey.as_value())?;
        let core = client_connect(_interp, addr, key)?;
        wrap_connection(_interp, core)
    }

    fn deliver_challenge(
        #[positional_only] connection: PyValueRef<'_>,
        #[positional_only] authkey: PyBytesLike<'_>,
    ) -> Result<Value> {
        let core = with_native_state::<ConnectionState, _>(connection.0, "Connection", |st| {
            Ok(Arc::clone(&st.core))
        })?;
        deliver_challenge_core(_interp, &core, authkey.as_bytes())?;
        Ok(Value::none())
    }

    fn answer_challenge(
        #[positional_only] connection: PyValueRef<'_>,
        #[positional_only] authkey: PyBytesLike<'_>,
    ) -> Result<Value> {
        let core = with_native_state::<ConnectionState, _>(connection.0, "Connection", |st| {
            Ok(Arc::clone(&st.core))
        })?;
        answer_challenge_core(_interp, &core, authkey.as_bytes())?;
        Ok(Value::none())
    }

    fn wait(
        #[positional_only] object_list: PyValueRef<'_>,
        #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
    ) -> Result<Value> {
        let objs = _interp.collect_iterable(object_list.0)?;
        let dur = parse_timeout(timeout.as_value(), false)?;
        if objs.is_empty() {
            if let Some(d) = dur
                && !d.is_zero()
            {
                let gil = Arc::clone(&_interp.cold.gil_state);
                _interp.release_gil_during(|| {
                    let _ = gil.sleep_interruptible(d);
                });
            }
            return Ok(Value::list(Vec::new()));
        }

        // Snapshot ConnectionCore handles where possible so we can poll
        // without holding the GIL!
        let mut conn_cores: Vec<Option<Arc<ConnectionCore>>> = Vec::with_capacity(objs.len());
        for obj in &objs {
            let c = with_native_state::<ConnectionState, _>(obj, "Connection", |st| {
                Ok(Arc::clone(&st.core))
            })
            .ok();
            conn_cores.push(c);
        }

        let deadline = dur.map(|d| Instant::now() + d);
        let gil = Arc::clone(&_interp.cold.gil_state);

        loop {
            let mut ready = Vec::new();
            for (idx, obj) in objs.iter().enumerate() {
                if let Some(ref core) = conn_cores[idx] {
                    if core.try_poll_now() {
                        ready.push(obj.clone());
                    }
                } else if let Ok(poll_fn) = _interp.get_attr(obj, "poll")
                    && let Ok(res) = call_pos(_interp, poll_fn, &[Value::float(0.0)])
                    && _interp.truthy_value(&res).unwrap_or(false)
                {
                    ready.push(obj.clone());
                } else if let Some(pid_sentinel) = obj.as_int() {
                    // Check if sentinel matches a completed process in active_children
                    if crate::builtin_modules::mp_rt::MP_STATE.with(|s| {
                        s.borrow().active_children.iter().any(|ch| {
                            crate::builtin_modules::macros::native_state::with_native_state::<
                                super::multiprocessing_process::ProcessState,
                                _,
                            >(ch, "BaseProcess", |ps| {
                                Ok(ps.sentinel_id == Some(pid_sentinel)
                                    && ps
                                        .handle
                                        .as_ref()
                                        .is_some_and(|h| !h.is_alive()))
                            })
                            .unwrap_or(false)
                        })
                    }) {
                        ready.push(obj.clone());
                    }
                }
            }
            if !ready.is_empty() {
                return Ok(Value::list(ready));
            }
            if let Some(dl) = deadline
                && Instant::now() >= dl
            {
                return Ok(Value::list(Vec::new()));
            }
            if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                return Ok(Value::list(Vec::new()));
            }
            let sleep_dur = match deadline {
                Some(dl) => (dl.saturating_duration_since(Instant::now())).min(Duration::from_millis(5)),
                None => Duration::from_millis(5).min(SHUTDOWN_POLL_SLICE),
            };
            _interp.release_gil_during(|| {
                thread::sleep(sleep_dur);
            });
        }
    }

    #[native_state(ConnectionState)]
    class Connection {
        #[getter]
        fn closed(#[state] st: &ConnectionState) -> Result<Value> {
            Ok(Value::bool_(st.core.is_closed()))
        }

        #[getter]
        fn readable(#[state] st: &ConnectionState) -> Result<Value> {
            Ok(Value::bool_(st.core.readable()))
        }

        #[getter]
        fn writable(#[state] st: &ConnectionState) -> Result<Value> {
            Ok(Value::bool_(st.core.writable()))
        }

        fn fileno(#[state] st: &ConnectionState) -> Result<Value> {
            Ok(Value::int(st.core.fileno()?))
        }

        fn close(#[state] st: &ConnectionState) -> Result<Value> {
            st.core.close();
            Ok(Value::none())
        }

        fn send(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] obj: PyValueRef<'_>,
        ) -> Result<Value> {
            let core = with_state(self_val.0, |st| Ok(Arc::clone(&st.core)))?;
            let bytes = pickle_dumps(_interp, obj.0.clone())?;
            core.send_bytes_raw(_interp, &bytes)?;
            Ok(Value::none())
        }

        fn recv(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let core = with_state(self_val.0, |st| Ok(Arc::clone(&st.core)))?;
            let Some(bytes) = core.recv_bytes_raw(_interp)? else {
                return Err(PyError::named("EOFError", ""));
            };
            pickle_loads(_interp, &bytes)
        }

        fn send_bytes(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] buf: PyBytesLike<'_>,
            #[default(0i64.into())] offset: PyIndex,
            #[default(OptionalArg::Omitted)] size: OptValueRef<'_>,
        ) -> Result<Value> {
            let core = with_state(self_val.0, |st| Ok(Arc::clone(&st.core)))?;
            let data = buf.as_bytes();
            let off_i64 = offset.expect_i64(FN_NAME, "offset")?;
            if off_i64 < 0 || (off_i64 as usize) > data.len() {
                return Err(PyError::named("ValueError", "offset is out of range"));
            }
            let off = off_i64 as usize;
            let end = match size.as_value() {
                None => data.len(),
                Some(v) if v.is_none() => data.len(),
                Some(v) => {
                    let s = v.as_int().ok_or_else(|| {
                        PyError::named("TypeError", "size must be an integer or None")
                    })?;
                    if s < 0 || off + (s as usize) > data.len() {
                        return Err(PyError::named("ValueError", "size is out of range"));
                    }
                    off + s as usize
                }
            };
            core.send_bytes_raw(_interp, &data[off..end])?;
            Ok(Value::none())
        }

        fn recv_bytes(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] maxlength: OptValueRef<'_>,
        ) -> Result<Value> {
            let core = with_state(self_val.0, |st| Ok(Arc::clone(&st.core)))?;
            let max_len = match maxlength.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => {
                    let n = v.as_int().ok_or_else(|| {
                        PyError::named("TypeError", "maxlength must be an int or None")
                    })?;
                    if n < 0 {
                        return Err(PyError::named("ValueError", "negative maxlength"));
                    }
                    Some(n as usize)
                }
            };
            let Some(bytes) = core.recv_bytes_raw(_interp)? else {
                return Err(PyError::named("EOFError", ""));
            };
            if let Some(lim) = max_len
                && bytes.len() > lim
            {
                return Err(PyError::named("OSError", "bad message length"));
            }
            Ok(Value::bytes(bytes))
        }

        fn recv_bytes_into(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] buf: PyValueRef<'_>,
            #[default(0i64.into())] offset: PyIndex,
        ) -> Result<Value> {
            let core = with_state(self_val.0, |st| Ok(Arc::clone(&st.core)))?;
            let off_i64 = offset.expect_i64(FN_NAME, "offset")?;
            if off_i64 < 0 {
                return Err(PyError::named("ValueError", "negative offset"));
            }
            let off = off_i64 as usize;
            let Some(bytes) = core.recv_bytes_raw(_interp)? else {
                return Err(PyError::named("EOFError", ""));
            };
            let target = acquire_writable_buffer(_interp, buf.0)?;
            let res = (|| -> Result<Value> {
                if off > target.len {
                    return Err(PyError::named("ValueError", "offset is out of range"));
                }
                let avail = target.len - off;
                if bytes.len() > avail {
                    let ctx_mod = _interp.load_module("multiprocessing.context")?;
                    let cls = _interp.get_attr(&ctx_mod, "BufferTooShort")?;
                    let exc = call_pos(_interp, cls, &[Value::bytes(bytes.clone())])?;
                    return Err(PyError::raised(exc));
                }
                let abs_start = target.offset + off;
                let mut dst = target.data.borrow_mut();
                dst[abs_start..abs_start + bytes.len()].copy_from_slice(&bytes);
                Ok(Value::int(bytes.len() as i64))
            })();
            let rel_res = target.release(_interp);
            let out = res?;
            rel_res?;
            Ok(out)
        }

        fn poll(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let core = with_state(self_val.0, |st| Ok(Arc::clone(&st.core)))?;
            let dur = match timeout.as_value() {
                None => Some(Duration::ZERO),
                Some(v) if v.is_none() => None,
                Some(v) => parse_timeout(Some(v), false)?,
            };
            let ready = core.poll(_interp, dur)?;
            Ok(Value::bool_(ready))
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (ep, readable, writable) = with_state(self_val.0, |st| {
                let ep = st.core.export_for_pickle()?;
                Ok((ep, st.core.readable(), st.core.writable()))
            })?;
            let m = _interp.load_module("multiprocessing.connection")?;
            let rebuild = _interp.get_attr(&m, "_rebuild_connection")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![
                    Value::string(&ep.addr),
                    Value::bytes(ep.token.to_vec()),
                    Value::int(ep.entry_id as i64),
                    Value::bool_(readable),
                    Value::bool_(writable),
                ]),
            ]))
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            with_state(self_val.0, |st| {
                st.core.close();
                Ok(())
            })?;
            Ok(Value::none())
        }
    }

    #[native_state(ListenerState)]
    class Listener {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] address: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] family: OptValueRef<'_>,
            #[default(1i64.into())] _backlog: PyIndex,
            #[default(OptionalArg::Omitted)] authkey: OptValueRef<'_>,
        ) -> Result<Value> {
            let addr = match address.as_value() {
                None => None,
                Some(v) if v.is_none() => None,
                Some(v) => Some(parse_address(v)?),
            };
            let fam = family
                .as_value()
                .and_then(|v| v.as_str().map(str::to_owned));
            let key = parse_authkey(authkey.as_value())?;
            let core = ListenerCore::bind(addr, fam, key)?;
            set_state(self_val.0, ListenerState { core })?;
            Ok(Value::none())
        }

        #[getter]
        fn address(#[state] st: &ListenerState) -> Result<Value> {
            Ok(address_to_py(&st.core.address))
        }

        #[getter]
        fn last_accepted(#[state] st: &ListenerState) -> Result<Value> {
            let g = st
                .core
                .last_accepted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(g.as_deref().map(Value::string).unwrap_or_else(Value::none))
        }

        fn accept(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let core = with_state(self_val.0, |st| Ok(Arc::clone(&st.core)))?;
            let (conn_core, _) = core.accept(_interp)?;
            wrap_connection(_interp, conn_core)
        }

        fn close(#[state] st: &ListenerState) -> Result<Value> {
            st.core.close();
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            with_state(self_val.0, |st| {
                st.core.close();
                Ok(())
            })?;
            Ok(Value::none())
        }
    }
}
