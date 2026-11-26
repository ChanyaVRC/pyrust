// `multiprocessing.managers` — 100% native-Rust Python 3.12 shared-object
// managers (`BaseManager`, `SyncManager`, `SharedMemoryManager`, `BaseProxy`,
// `Token`, `RemoteError`, `Namespace`, `DictProxy`, `ListProxy`, `ValueProxy`,
// `ArrayProxy`, `NamespaceProxy`, `AcquirerProxy`, `EventProxy`,
// `ConditionProxy`, `BarrierProxy`, `QueueProxy`, `AutoProxy`, `MakeProxyType`,
// `RebuildProxy`) (issue #3264).
//
// Starts a real authenticated loopback TCP RPC server in a Python daemon thread
// (spawning one daemon Python handler thread per connected client so blocking
// proxy methods like `Queue.get()` or `Lock.acquire()` never starve other
// clients). Works seamlessly across both `'spawn'` (Windows & Unix) and
// `'fork'` (Unix) child processes!

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::native_state::{set_native_state, with_native_state};
use crate::builtin_modules::mp_rt::{
    SHUTDOWN_POLL_SLICE, call_pos, call_pos_kw, current_authkey, indexmap_to_pydict, pickle_dumps,
    pickle_loads, py_contains, py_delitem, py_eq, py_getitem, py_setitem, random_hex,
};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBytesLike, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, NativeIterFrame, current_thread_is_daemon_worker,
    render_value_repr,
};
use crate::value::{InstanceAttrs, PyDict, PyInstance, PyKey, PyModule, Value, ValueKind};

const RPC_OP_CALL: u8 = 1;
const RPC_OP_CREATE: u8 = 2;

struct ManagerRegistry {
    /// Registered factory entries: `typeid -> (callable, proxy_cls_name)`
    factories: HashMap<String, (Option<Value>, Option<Value>)>,
}

struct ManagerStore {
    next_id: u64,
    objects: HashMap<String, Value>,
}

struct ManagerShared {
    addr: String,
    token: Vec<u8>,
    stop: AtomicBool,
    listener: Mutex<Option<TcpListener>>,
    /// Incoming accepted client streams are popped by the Python server loop.
    accepted: Mutex<Vec<TcpStream>>,
}

struct ManagerState {
    authkey: Vec<u8>,
    address_hint: Option<String>,
    registry: Rc<RefCell<ManagerRegistry>>,
    store: Rc<RefCell<ManagerStore>>,
    shared: Option<Arc<ManagerShared>>,
    server_thread: Option<Value>,
    started: bool,
    shutdown: bool,
    /// Tracked SharedMemory objects for `SharedMemoryManager`.
    shms: Vec<Value>,
}

struct ServerRunnerState {
    shared: Arc<ManagerShared>,
    store: Rc<RefCell<ManagerStore>>,
    registry: Rc<RefCell<ManagerRegistry>>,
}

struct ConnHandlerState {
    stream: Mutex<Option<TcpStream>>,
    token: Vec<u8>,
    store: Rc<RefCell<ManagerStore>>,
    registry: Rc<RefCell<ManagerRegistry>>,
    stop: Arc<ManagerShared>,
}

struct TokenState {
    typeid: String,
    address: String,
    id: String,
}

struct ProxyState {
    typeid: String,
    address: String,
    id: String,
    authkey: Vec<u8>,
}

struct ValueBoxState {
    typecode: String,
    value: Value,
}

fn write_rpc_frame(stream: &mut TcpStream, data: &[u8]) -> io::Result<()> {
    let hdr = (data.len() as u32).to_be_bytes();
    stream.write_all(&hdr)?;
    if !data.is_empty() {
        stream.write_all(data)?;
    }
    stream.flush()
}

fn read_rpc_frame(
    stream: &mut TcpStream,
    stop: Option<&AtomicBool>,
    gil_shutting_down: impl Fn() -> bool,
) -> io::Result<Option<Vec<u8>>> {
    let _ = stream.set_read_timeout(Some(SHUTDOWN_POLL_SLICE));
    let mut hdr = [0u8; 4];
    let mut got = 0usize;
    while got < 4 {
        if stop.is_some_and(|s| s.load(Ordering::Acquire)) || gil_shutting_down() {
            return Ok(None);
        }
        match stream.read(&mut hdr[got..]) {
            Ok(0) => return Ok(None),
            Ok(n) => got += n,
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    let len = u32::from_be_bytes(hdr) as usize;
    let mut buf = vec![0u8; len];
    let mut read_n = 0usize;
    while read_n < len {
        if stop.is_some_and(|s| s.load(Ordering::Acquire)) || gil_shutting_down() {
            return Ok(None);
        }
        match stream.read(&mut buf[read_n..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "truncated RPC frame",
                ));
            }
            Ok(n) => read_n += n,
            Err(ref e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Ok(Some(buf))
}

fn client_rpc_call(
    interp: &mut Interpreter,
    addr: &str,
    authkey: &[u8],
    op: u8,
    req_val: Value,
) -> Result<Value> {
    let req_bytes = pickle_dumps(interp, req_val)?;
    let addr_owned = addr.to_string();
    let key_owned = authkey.to_vec();
    let gil = Arc::clone(&interp.cold.gil_state);

    let reply_res: io::Result<Option<Vec<u8>>> = interp.release_gil_during(|| {
        let mut stream = TcpStream::connect(&addr_owned)?;
        let _ = stream.set_nodelay(true);
        // Send authkey frame, then op byte + request frame.
        write_rpc_frame(&mut stream, &key_owned)?;
        stream.write_all(&[op])?;
        write_rpc_frame(&mut stream, &req_bytes)?;
        read_rpc_frame(&mut stream, None, || {
            gil.is_shutting_down() && current_thread_is_daemon_worker()
        })
    });

    let Some(reply_bytes) = reply_res.map_err(|e| PyError::from_io_error(&e, None))? else {
        return Err(PyError::named("EOFError", "manager connection closed"));
    };

    let reply_val = pickle_loads(interp, &reply_bytes)?;
    let ValueKind::Tuple(items) = reply_val.kind() else {
        return Err(PyError::named("RuntimeError", "invalid manager reply"));
    };
    if items.len() != 2 {
        return Err(PyError::named("RuntimeError", "invalid manager reply tuple"));
    }
    let ok = interp.truthy_value(&items[0])?;
    if ok {
        Ok(items[1].clone())
    } else {
        let exc = items[1].clone();
        let is_inst = matches!(exc.kind(), ValueKind::PyInstance(_));
        if is_inst {
            Err(PyError::raised(exc))
        } else {
            Err(PyError::named(
                "RemoteError",
                exc.as_str().unwrap_or("remote error").to_string(),
            ))
        }
    }
}

fn proxy_callmethod(
    interp: &mut Interpreter,
    proxy_val: &Value,
    method: &str,
    args: &[Value],
    kwargs: &[(String, Value)],
) -> Result<Value> {
    let (addr, id, authkey) = with_native_state::<ProxyState, _>(proxy_val, "BaseProxy", |st| {
        Ok((st.address.clone(), st.id.clone(), st.authkey.clone()))
    })?;
    let kw_dict = Value::dict(PyDict::default());
    for (k, v) in kwargs {
        kw_dict.dict_insert(PyKey::str_from(k), v.clone())?;
    }
    let req = Value::tuple(vec![
        Value::string(&id),
        Value::string(method),
        Value::tuple(args.to_vec()),
        kw_dict,
    ]);
    client_rpc_call(interp, &addr, &authkey, RPC_OP_CALL, req)
}

fn create_remote_object_on_server(
    interp: &mut Interpreter,
    store: &Rc<RefCell<ManagerStore>>,
    registry: &Rc<RefCell<ManagerRegistry>>,
    typeid: &str,
    args: &[Value],
    kwargs: &[(String, Value)],
) -> Result<String> {
    // Built-in SyncManager types or custom registered callables.
    let custom_factory = registry
        .borrow()
        .factories
        .get(typeid)
        .and_then(|(f, _)| f.clone());

    let obj = if let Some(factory) = custom_factory {
        call_pos_kw(interp, factory, args, kwargs)?
    } else {
        match typeid {
            "dict" => {
                let d_cls = Value::builtin_function("dict");
                call_pos_kw(interp, d_cls, args, kwargs)?
            }
            "list" => {
                let l_cls = Value::builtin_function("list");
                call_pos_kw(interp, l_cls, args, kwargs)?
            }
            "Namespace" => {
                let m = interp.load_module("multiprocessing.managers")?;
                let cls = interp.get_attr(&m, "Namespace")?;
                call_pos_kw(interp, cls, args, kwargs)?
            }
            "Value" => {
                let m = interp.load_module("multiprocessing.managers")?;
                let cls = interp.get_attr(&m, "_ValueBox")?;
                call_pos_kw(interp, cls, args, kwargs)?
            }
            "Array" => {
                let arr_mod = interp.load_module("array")?;
                let arr_cls = interp.get_attr(&arr_mod, "array")?;
                call_pos_kw(interp, arr_cls, args, kwargs)?
            }
            "Queue" => {
                let q_mod = interp.load_module("queue")?;
                let q_cls = interp.get_attr(&q_mod, "Queue")?;
                call_pos_kw(interp, q_cls, args, kwargs)?
            }
            "JoinableQueue" => {
                let q_mod = interp.load_module("queue")?;
                let q_cls = interp.get_attr(&q_mod, "Queue")?;
                call_pos_kw(interp, q_cls, args, kwargs)?
            }
            "Lock" => {
                let th = interp.load_module("threading")?;
                let f = interp.get_attr(&th, "Lock")?;
                call_pos_kw(interp, f, args, kwargs)?
            }
            "RLock" => {
                let th = interp.load_module("threading")?;
                let f = interp.get_attr(&th, "RLock")?;
                call_pos_kw(interp, f, args, kwargs)?
            }
            "Semaphore" => {
                let th = interp.load_module("threading")?;
                let f = interp.get_attr(&th, "Semaphore")?;
                call_pos_kw(interp, f, args, kwargs)?
            }
            "BoundedSemaphore" => {
                let th = interp.load_module("threading")?;
                let f = interp.get_attr(&th, "BoundedSemaphore")?;
                call_pos_kw(interp, f, args, kwargs)?
            }
            "Event" => {
                let th = interp.load_module("threading")?;
                let f = interp.get_attr(&th, "Event")?;
                call_pos_kw(interp, f, args, kwargs)?
            }
            "Condition" => {
                let th = interp.load_module("threading")?;
                let f = interp.get_attr(&th, "Condition")?;
                call_pos_kw(interp, f, args, kwargs)?
            }
            "Barrier" => {
                let th = interp.load_module("threading")?;
                let f = interp.get_attr(&th, "Barrier")?;
                call_pos_kw(interp, f, args, kwargs)?
            }
            other => {
                return Err(PyError::named(
                    "KeyError",
                    format!("unregistered manager typeid '{other}'"),
                ));
            }
        }
    };

    let mut st = store.borrow_mut();
    st.next_id += 1;
    let id = format!("obj_{}", st.next_id);
    st.objects.insert(id.clone(), obj);
    Ok(id)
}

fn dispatch_server_method(
    interp: &mut Interpreter,
    store: &Rc<RefCell<ManagerStore>>,
    obj_id: &str,
    method: &str,
    args: &[Value],
    kwargs: &[(String, Value)],
) -> Result<Value> {
    let target = {
        let st = store.borrow();
        st.objects.get(obj_id).cloned()
    }
    .ok_or_else(|| PyError::named("KeyError", format!("unknown managed object '{obj_id}'")))?;

    match method {
        "__getattribute__" => {
            let attr = args
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| PyError::named("TypeError", "attribute name must be str"))?;
            interp.get_attr(&target, attr)
        }
        "__setattr__" => {
            let attr = args
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| PyError::named("TypeError", "attribute name must be str"))?;
            let val = args.get(1).cloned().unwrap_or_else(Value::none);
            interp.assign_attr(target, attr, val)?;
            Ok(Value::none())
        }
        "__delattr__" => {
            let attr = args
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| PyError::named("TypeError", "attribute name must be str"))?;
            interp.delete_attr(target, attr)?;
            Ok(Value::none())
        }
        "__repr__" => Ok(Value::string(&render_value_repr(interp, &target)?)),
        "__str__" => {
            let s = call_pos(interp, Value::builtin_function("str"), &[target])?;
            Ok(s)
        }
        "__len__" => call_pos(interp, Value::builtin_function("len"), &[target]),
        "__contains__" => {
            let item = args.first().cloned().unwrap_or_else(Value::none);
            let b = py_contains(interp, target, item)?;
            Ok(Value::bool_(b))
        }
        "__getitem__" => {
            let key = args.first().cloned().unwrap_or_else(Value::none);
            py_getitem(interp, target, key)
        }
        "__setitem__" => {
            let key = args.first().cloned().unwrap_or_else(Value::none);
            let val = args.get(1).cloned().unwrap_or_else(Value::none);
            py_setitem(interp, target, key, val)?;
            Ok(Value::none())
        }
        "__delitem__" => {
            let key = args.first().cloned().unwrap_or_else(Value::none);
            py_delitem(interp, target, key)?;
            Ok(Value::none())
        }
        "__eq__" => {
            let other = args.first().cloned().unwrap_or_else(Value::none);
            let eq = py_eq(interp, target, other)?;
            Ok(Value::bool_(eq))
        }
        "_iter_snapshot" => {
            let items = interp.collect_iterable(&target)?;
            Ok(Value::list(items))
        }
        "_dict_copy" => {
            if let Ok(copy_fn) = interp.get_attr(&target, "copy") {
                call_pos(interp, copy_fn, &[])
            } else {
                Ok(target)
            }
        }
        "_get_value_attr" => interp.get_attr(&target, "value"),
        "_set_value_attr" => {
            let val = args.first().cloned().unwrap_or_else(Value::none);
            interp.assign_attr(target, "value", val)?;
            Ok(Value::none())
        }
        _ => {
            let meth = interp.get_attr(&target, method)?;
            call_pos_kw(interp, meth, args, kwargs)
        }
    }
}

fn encode_server_reply(interp: &mut Interpreter, res: Result<Value>) -> Vec<u8> {
    let tuple = match res {
        Ok(val) => Value::tuple(vec![Value::bool_(true), val]),
        Err(err) => {
            let exc_val = interp
                .materialize_pyerror(err)
                .unwrap_or_else(|e| Value::string(e.to_string()));
            Value::tuple(vec![Value::bool_(false), exc_val])
        }
    };
    match pickle_dumps(interp, tuple) {
        Ok(b) => b,
        Err(err) => {
            let fallback = Value::tuple(vec![Value::bool_(false), Value::string(err.to_string())]);
            pickle_dumps(interp, fallback).unwrap_or_default()
        }
    }
}

fn make_proxy_instance(
    interp: &mut Interpreter,
    proxy_cls_name: &str,
    typeid: String,
    address: String,
    id: String,
    authkey: Vec<u8>,
) -> Result<Value> {
    let m = interp.load_module("multiprocessing.managers")?;
    let cls = interp.get_attr(&m, proxy_cls_name)?;
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Err(PyError::named("TypeError", "invalid proxy class"));
    };
    let inst = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class: Rc::clone(cls_rc),
        attrs: InstanceAttrs::new(),
    })));
    set_native_state(
        &inst,
        ProxyState {
            typeid,
            address,
            id,
            authkey,
        },
    )?;
    Ok(inst)
}

fn manager_create_proxy(
    interp: &mut Interpreter,
    mgr_val: &Value,
    typeid: &str,
    proxy_cls_name: &str,
    args: &[Value],
    kwargs: IndexMap<String, Value>,
) -> Result<Value> {
    let (addr, authkey, started) =
        with_native_state::<ManagerState, _>(mgr_val, "BaseManager", |st| {
            let addr = st
                .shared
                .as_ref()
                .map(|s| s.addr.clone())
                .or_else(|| st.address_hint.clone())
                .unwrap_or_default();
            Ok((addr, st.authkey.clone(), st.started))
        })?;
    if !started || addr.is_empty() {
        return Err(PyError::named(
            "AssertionError",
            "manager has not been started",
        ));
    }
    let kw_dict = indexmap_to_pydict(kwargs)?;
    let req = Value::tuple(vec![
        Value::string(typeid),
        Value::tuple(args.to_vec()),
        kw_dict,
    ]);
    let id_val = client_rpc_call(interp, &addr, &authkey, RPC_OP_CREATE, req)?;
    let id = id_val
        .as_str()
        .ok_or_else(|| PyError::named("RuntimeError", "manager did not return object id"))?
        .to_string();
    make_proxy_instance(interp, proxy_cls_name, typeid.to_string(), addr, id, authkey)
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
            Value::string("BaseManager"),
            Value::string("SyncManager"),
            Value::string("BaseProxy"),
            Value::string("Token"),
            Value::string("SharedMemoryManager"),
        ]),
        "HAS_SHMEM" => Value::bool_(true),
    }

    exception RemoteError;

    #[native_state(ValueBoxState)]
    class _ValueBox {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] typecode: PyValueRef<'_>,
            #[positional_only] value: PyValueRef<'_>,
            #[default(true.into())] _lock: crate::interpreter::builtin_args::PyBool,
        ) -> Result<Value> {
            let tc = typecode
                .0
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| "i".to_string());
            set_state(
                self_val.0,
                ValueBoxState {
                    typecode: tc,
                    value: value.0.clone(),
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn value(#[state] st: &ValueBoxState) -> Result<Value> {
            Ok(st.value.clone())
        }

        #[setter("value")]
        fn set_value(#[state_mut] st: &mut ValueBoxState, v: PyValue) -> Result<Value> {
            st.value = v.0;
            Ok(Value::none())
        }

        fn get(#[state] st: &ValueBoxState) -> Result<Value> {
            Ok(st.value.clone())
        }

        fn set(#[state_mut] st: &mut ValueBoxState, #[positional_only] v: PyValue) -> Result<Value> {
            st.value = v.0;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (tc, v) = with_state(self_val.0, |st| Ok((st.typecode.clone(), st.value.clone())))?;
            let vr = render_value_repr(_interp, &v)?;
            Ok(Value::string(format!("Value({tc:?}, {vr})")))
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

    #[native_state(TokenState)]
    class Token {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] typeid: PyStr<'_>,
            #[positional_only] address: PyValueRef<'_>,
            #[positional_only] id: PyValueRef<'_>,
        ) -> Result<Value> {
            let addr_s = address.0.as_str().unwrap_or("").to_string();
            let id_s = id.0.as_str().unwrap_or("").to_string();
            set_state(
                self_val.0,
                TokenState {
                    typeid: typeid.0.to_string(),
                    address: addr_s,
                    id: id_s,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn typeid(#[state] st: &TokenState) -> Result<Value> {
            Ok(Value::string(&st.typeid))
        }

        #[getter]
        fn address(#[state] st: &TokenState) -> Result<Value> {
            Ok(Value::string(&st.address))
        }

        #[getter]
        fn id(#[state] st: &TokenState) -> Result<Value> {
            Ok(Value::string(&st.id))
        }
    }

    #[native_state(ConnHandlerState)]
    class _ManagerConnHandler {
        fn __call__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (stream_opt, expected_token, store, registry, shared) =
                with_state(self_val.0, |st| {
                    let s = st
                        .stream
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    Ok((
                        s,
                        st.token.clone(),
                        Rc::clone(&st.store),
                        Rc::clone(&st.registry),
                        Arc::clone(&st.stop),
                    ))
                })?;
            let Some(mut stream) = stream_opt else {
                return Ok(Value::none());
            };
            let gil = Arc::clone(&_interp.cold.gil_state);

            // Read authkey frame
            let auth = _interp.release_gil_during(|| {
                read_rpc_frame(&mut stream, Some(&shared.stop), || {
                    gil.is_shutting_down() && current_thread_is_daemon_worker()
                })
            });
            let Ok(Some(client_key)) = auth else {
                return Ok(Value::none());
            };
            if client_key != expected_token {
                return Ok(Value::none());
            }

            // Read op byte + request frame
            let req_read = _interp.release_gil_during(|| -> io::Result<Option<(u8, Vec<u8>)>> {
                let mut op = [0u8; 1];
                stream.read_exact(&mut op)?;
                let Some(payload) = read_rpc_frame(&mut stream, Some(&shared.stop), || {
                    gil.is_shutting_down() && current_thread_is_daemon_worker()
                })?
                else {
                    return Ok(None);
                };
                Ok(Some((op[0], payload)))
            });

            let Ok(Some((op, payload))) = req_read else {
                return Ok(Value::none());
            };

            let result = (|| -> Result<Value> {
                let req_val = pickle_loads(_interp, &payload)?;
                let ValueKind::Tuple(items) = req_val.kind() else {
                    return Err(PyError::named("ValueError", "malformed manager request"));
                };
                match op {
                    RPC_OP_CREATE => {
                        if items.len() != 3 {
                            return Err(PyError::named("ValueError", "malformed create request"));
                        }
                        let typeid = items[0]
                            .as_str()
                            .ok_or_else(|| PyError::named("TypeError", "typeid must be str"))?;
                        let args = _interp.collect_iterable(&items[1])?;
                        let mut kw = Vec::new();
                        if let Some(d) = items[2].as_dict() {
                            for (k, v) in d.iter() {
                                if let PyKey::Str(s) = k {
                                    kw.push((s.to_string(), v.clone()));
                                }
                            }
                        }
                        let id = create_remote_object_on_server(
                            _interp, &store, &registry, typeid, &args, &kw,
                        )?;
                        Ok(Value::string(&id))
                    }
                    RPC_OP_CALL => {
                        if items.len() != 4 {
                            return Err(PyError::named("ValueError", "malformed call request"));
                        }
                        let obj_id = items[0]
                            .as_str()
                            .ok_or_else(|| PyError::named("TypeError", "obj_id must be str"))?;
                        let meth = items[1]
                            .as_str()
                            .ok_or_else(|| PyError::named("TypeError", "method must be str"))?;
                        let args = _interp.collect_iterable(&items[2])?;
                        let mut kw = Vec::new();
                        if let Some(d) = items[3].as_dict() {
                            for (k, v) in d.iter() {
                                if let PyKey::Str(s) = k {
                                    kw.push((s.to_string(), v.clone()));
                                }
                            }
                        }
                        dispatch_server_method(_interp, &store, obj_id, meth, &args, &kw)
                    }
                    _ => Err(PyError::named("ValueError", "unknown RPC opcode")),
                }
            })();

            let reply_bytes = encode_server_reply(_interp, result);
            let _ = _interp.release_gil_during(|| write_rpc_frame(&mut stream, &reply_bytes));
            Ok(Value::none())
        }
    }

    #[native_state(ServerRunnerState)]
    class _ManagerServerRunner {
        fn __call__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (shared, store, registry) = with_state(self_val.0, |st| {
                Ok((
                    Arc::clone(&st.shared),
                    Rc::clone(&st.store),
                    Rc::clone(&st.registry),
                ))
            })?;
            let gil = Arc::clone(&_interp.cold.gil_state);

            loop {
                let incoming_stream: Option<TcpStream> = _interp.release_gil_during(|| {
                    loop {
                        if shared.stop.load(Ordering::Acquire) {
                            return None;
                        }
                        if gil.is_shutting_down() && current_thread_is_daemon_worker() {
                            return None;
                        }
                        let guard = shared
                            .listener
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let listener = (*guard).as_ref()?;
                        match listener.accept() {
                            Ok((stream, _)) => {
                                let _ = stream.set_nonblocking(false);
                                let _ = stream.set_nodelay(true);
                                return Some(stream);
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                drop(guard);
                                thread::sleep(Duration::from_millis(5));
                            }
                            Err(_) => return None,
                        }
                    }
                });

                let Some(stream) = incoming_stream else {
                    break;
                };

                // Spawn a daemon Python handler thread per RPC client so
                // blocking proxy operations (e.g. Queue.get, Lock.acquire)
                // never block concurrent clients.
                let mgr_mod = _interp.load_module("multiprocessing.managers")?;
                let handler_cls = _interp.get_attr(&mgr_mod, "_ManagerConnHandler")?;
                let handler = call_pos(_interp, handler_cls, &[])?;
                set_native_state(
                    &handler,
                    ConnHandlerState {
                        stream: Mutex::new(Some(stream)),
                        token: shared.token.clone(),
                        store: Rc::clone(&store),
                        registry: Rc::clone(&registry),
                        stop: Arc::clone(&shared),
                    },
                )?;
                let th_mod = _interp.load_module("threading")?;
                let th_cls = _interp.get_attr(&th_mod, "Thread")?;
                let th = _interp.call_function_expanded(
                    th_cls,
                    &[
                        ExpandedCallArg {
                            name: Some("target".to_string()),
                            value: handler,
                        },
                        ExpandedCallArg {
                            name: Some("daemon".to_string()),
                            value: Value::bool_(true),
                        },
                    ],
                )?;
                let start_fn = _interp.get_attr(&th, "start")?;
                let _ = call_pos(_interp, start_fn, &[]);
            }

            shared
                .accepted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            Ok(Value::none())
        }
    }

    #[py_name = "RebuildProxy"]
    fn rebuild_proxy(
        #[positional_only] proxy_cls_name: PyStr<'_>,
        #[positional_only] typeid: PyStr<'_>,
        #[positional_only] address: PyStr<'_>,
        #[positional_only] id: PyStr<'_>,
        #[positional_only] authkey: PyBytesLike<'_>,
    ) -> Result<Value> {
        make_proxy_instance(
            _interp,
            &proxy_cls_name.0,
            typeid.0.to_string(),
            address.0.to_string(),
            id.0.to_string(),
            authkey.as_bytes().to_vec(),
        )
    }

    #[py_name = "AutoProxy"]
    fn auto_proxy(
        #[positional_only] token: PyValueRef<'_>,
        #[varargs] _args: &[Value],
        #[varkwargs] _kwargs: IndexMap<String, Value>,
    ) -> Result<Value> {
        let (typeid, address, id) =
            with_native_state::<TokenState, _>(token.0, "Token", |st| {
                Ok((st.typeid.clone(), st.address.clone(), st.id.clone()))
            })?;
        make_proxy_instance(
            _interp,
            "BaseProxy",
            typeid,
            address,
            id,
            current_authkey(),
        )
    }

    #[py_name = "MakeProxyType"]
    fn make_proxy_type(
        #[positional_only] _name: PyStr<'_>,
        #[positional_only] _exposed: PyValueRef<'_>,
    ) -> Result<Value> {
        let m = _interp.load_module("multiprocessing.managers")?;
        _interp.get_attr(&m, "BaseProxy")
    }

    #[native_state(ProxyState)]
    class BaseProxy {
        fn _callmethod(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] methodname: PyStr<'_>,
            #[default(OptionalArg::Omitted)] args: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] kwds: OptValueRef<'_>,
        ) -> Result<Value> {
            let arg_vec = match args.as_value() {
                Some(v) if !v.is_none() => _interp.collect_iterable(v)?,
                _ => Vec::new(),
            };
            let mut kw_vec = Vec::new();
            if let Some(kv) = kwds.as_value()
                && let Some(d) = kv.as_dict()
            {
                for (k, v) in d.iter() {
                    if let PyKey::Str(s) = k {
                        kw_vec.push((s.to_string(), v.clone()));
                    }
                }
            }
            proxy_callmethod(_interp, self_val.0, &methodname.0, &arg_vec, &kw_vec)
        }

        fn _getvalue(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "_dict_copy", &[], &[])
        }

        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
        ) -> Result<Value> {
            let attr = name.0;
            if attr.starts_with("__") && attr.ends_with("__") {
                return Err(PyError::named("AttributeError", attr));
            }
            // Return a bound proxy method callable for arbitrary remote methods.
            let m = _interp.load_module("multiprocessing.managers")?;
            let mcls = _interp.get_attr(&m, "_BoundProxyMethod")?;
            call_pos(_interp, mcls, &[self_val.0.clone(), Value::string(attr)])
        }

        fn __reduce__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls_name = match self_val.0.kind() {
                ValueKind::PyInstance(rc) => rc.borrow().class.borrow().name.clone(),
                _ => "BaseProxy".to_string(),
            };
            let (typeid, address, id, authkey) = with_state(self_val.0, |st| {
                Ok((
                    st.typeid.clone(),
                    st.address.clone(),
                    st.id.clone(),
                    st.authkey.clone(),
                ))
            })?;
            let m = _interp.load_module("multiprocessing.managers")?;
            let rebuild = _interp.get_attr(&m, "RebuildProxy")?;
            Ok(Value::tuple(vec![
                rebuild,
                Value::tuple(vec![
                    Value::string(&cls_name),
                    Value::string(&typeid),
                    Value::string(&address),
                    Value::string(&id),
                    Value::bytes(authkey),
                ]),
            ]))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__repr__", &[], &[])
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__str__", &[], &[])
        }
    }

    class _BoundProxyMethod {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] proxy: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_proxy", proxy.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "_name", Value::string(name.0))?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let proxy = _interp.get_attr(self_val.0, "_proxy")?;
            let name_val = _interp.get_attr(self_val.0, "_name")?;
            let method = name_val
                .as_str()
                .ok_or_else(|| PyError::named("TypeError", "invalid proxy method name"))?;
            let kw: Vec<(String, Value)> = kwargs.into_iter().collect();
            proxy_callmethod(_interp, &proxy, method, args, &kw)
        }
    }

    #[native_state(ProxyState)]
    class DictProxy : "BaseProxy" {
        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__len__", &[], &[])
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__getitem__", &[key.0], &[])
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
            #[positional_only] val: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__setitem__", &[key.0, val.0], &[])
        }

        fn __delitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__delitem__", &[key.0], &[])
        }

        fn __contains__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__contains__", &[key.0], &[])
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let snap = proxy_callmethod(_interp, self_val.0, "_iter_snapshot", &[], &[])?;
            let items = _interp.collect_iterable(&snap)?;
            Ok(Value::generator(Box::new(NativeIterFrame::generator(items))))
        }

        fn __eq__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] other: PyValue,
        ) -> Result<Value> {
            let rhs = if with_native_state::<ProxyState, _>(&other.0, "BaseProxy", |_| Ok(())).is_ok() {
                proxy_callmethod(_interp, &other.0, "_dict_copy", &[], &[])?
            } else {
                other.0
            };
            proxy_callmethod(_interp, self_val.0, "__eq__", &[rhs], &[])
        }

        fn keys(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let snap = proxy_callmethod(_interp, self_val.0, "_iter_snapshot", &[], &[])?;
            Ok(snap)
        }

        fn values(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let copied = proxy_callmethod(_interp, self_val.0, "_dict_copy", &[], &[])?;
            let vfn = _interp.get_attr(&copied, "values")?;
            let vals = call_pos(_interp, vfn, &[])?;
            Ok(Value::list(_interp.collect_iterable(&vals)?))
        }

        fn items(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let copied = proxy_callmethod(_interp, self_val.0, "_dict_copy", &[], &[])?;
            let ifn = _interp.get_attr(&copied, "items")?;
            let its = call_pos(_interp, ifn, &[])?;
            Ok(Value::list(_interp.collect_iterable(&its)?))
        }

        fn copy(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "_dict_copy", &[], &[])
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "get", args, &[])
        }

        fn pop(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "pop", args, &[])
        }

        fn setdefault(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "setdefault", args, &[])
        }

        fn update(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let kw: Vec<(String, Value)> = kwargs.into_iter().collect();
            proxy_callmethod(_interp, self_val.0, "update", args, &kw)
        }

        fn clear(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "clear", &[], &[])
        }
    }

    #[native_state(ProxyState)]
    class ListProxy : "BaseProxy" {
        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__len__", &[], &[])
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__getitem__", &[key.0], &[])
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
            #[positional_only] val: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__setitem__", &[key.0, val.0], &[])
        }

        fn __delitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__delitem__", &[key.0], &[])
        }

        fn __contains__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] item: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__contains__", &[item.0], &[])
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let snap = proxy_callmethod(_interp, self_val.0, "_iter_snapshot", &[], &[])?;
            let items = _interp.collect_iterable(&snap)?;
            Ok(Value::generator(Box::new(NativeIterFrame::generator(items))))
        }

        fn __eq__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] other: PyValue,
        ) -> Result<Value> {
            let rhs = if with_native_state::<ProxyState, _>(&other.0, "BaseProxy", |_| Ok(())).is_ok() {
                proxy_callmethod(_interp, &other.0, "_iter_snapshot", &[], &[])?
            } else {
                other.0
            };
            proxy_callmethod(_interp, self_val.0, "__eq__", &[rhs], &[])
        }

        fn __add__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] other: PyValue,
        ) -> Result<Value> {
            let snap = proxy_callmethod(_interp, self_val.0, "_iter_snapshot", &[], &[])?;
            let mut items = _interp.collect_iterable(&snap)?;
            items.extend(_interp.collect_iterable(&other.0)?);
            Ok(Value::list(items))
        }

        fn __iadd__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] other: PyValue,
        ) -> Result<Value> {
            let items = Value::list(_interp.collect_iterable(&other.0)?);
            proxy_callmethod(_interp, self_val.0, "extend", &[items], &[])?;
            Ok(self_val.0.clone())
        }

        fn append(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] item: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "append", &[item.0], &[])
        }

        fn extend(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] iterable: PyValue,
        ) -> Result<Value> {
            let items = Value::list(_interp.collect_iterable(&iterable.0)?);
            proxy_callmethod(_interp, self_val.0, "extend", &[items], &[])
        }

        fn insert(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] idx: PyValue,
            #[positional_only] item: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "insert", &[idx.0, item.0], &[])
        }

        fn pop(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "pop", args, &[])
        }

        fn remove(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] item: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "remove", &[item.0], &[])
        }

        fn sort(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let kw: Vec<(String, Value)> = kwargs.into_iter().collect();
            proxy_callmethod(_interp, self_val.0, "sort", &[], &kw)
        }

        fn reverse(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "reverse", &[], &[])
        }

        fn count(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] item: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "count", &[item.0], &[])
        }

        fn index(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "index", args, &[])
        }

        fn clear(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "clear", &[], &[])
        }

        fn copy(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "_iter_snapshot", &[], &[])
        }
    }

    #[native_state(ProxyState)]
    class NamespaceProxy : "BaseProxy" {
        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
        ) -> Result<Value> {
            let attr = name.0;
            if attr.starts_with('_') {
                return Err(PyError::named("AttributeError", attr));
            }
            proxy_callmethod(
                _interp,
                self_val.0,
                "__getattribute__",
                &[Value::string(attr)],
                &[],
            )
        }

        fn __setattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
            #[positional_only] val: PyValue,
        ) -> Result<Value> {
            let attr = name.0;
            if attr.starts_with('_') {
                if let ValueKind::PyInstance(rc) = self_val.0.kind() {
                    rc.borrow_mut().attrs.insert(&attr, val.0);
                }
                return Ok(Value::none());
            }
            proxy_callmethod(
                _interp,
                self_val.0,
                "__setattr__",
                &[Value::string(attr), val.0],
                &[],
            )
        }

        fn __delattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
        ) -> Result<Value> {
            proxy_callmethod(
                _interp,
                self_val.0,
                "__delattr__",
                &[Value::string(name.0)],
                &[],
            )
        }
    }

    #[native_state(ProxyState)]
    class ValueProxy : "BaseProxy" {
        #[getter]
        fn value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "_get_value_attr", &[], &[])
        }

        #[setter("value")]
        fn set_value(
            #[positional_only] self_val: PyValueRef<'_>,
            val: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "_set_value_attr", &[val.0], &[])
        }

        fn get(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "get", &[], &[])
        }

        fn set(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] val: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "set", &[val.0], &[])
        }
    }

    #[native_state(ProxyState)]
    class ArrayProxy : "BaseProxy" {
        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__len__", &[], &[])
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__getitem__", &[key.0], &[])
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] key: PyValue,
            #[positional_only] val: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__setitem__", &[key.0, val.0], &[])
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let snap = proxy_callmethod(_interp, self_val.0, "_iter_snapshot", &[], &[])?;
            let items = _interp.collect_iterable(&snap)?;
            Ok(Value::generator(Box::new(NativeIterFrame::generator(items))))
        }
    }

    #[native_state(ProxyState)]
    class AcquirerProxy : "BaseProxy" {
        fn acquire(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let kw: Vec<(String, Value)> = kwargs.into_iter().collect();
            proxy_callmethod(_interp, self_val.0, "acquire", args, &kw)
        }

        fn release(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "release", &[], &[])
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "acquire", &[], &[])
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "release", &[], &[])?;
            Ok(Value::none())
        }
    }

    #[native_state(ProxyState)]
    class EventProxy : "BaseProxy" {
        fn is_set(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "is_set", &[], &[])
        }

        fn set(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "set", &[], &[])
        }

        fn clear(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "clear", &[], &[])
        }

        fn wait(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "wait", args, &[])
        }
    }

    #[native_state(ProxyState)]
    class ConditionProxy : "AcquirerProxy" {
        fn wait(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "wait", args, &[])
        }

        fn notify(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "notify", args, &[])
        }

        fn notify_all(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "notify_all", &[], &[])
        }
    }

    #[native_state(ProxyState)]
    class BarrierProxy : "BaseProxy" {
        fn wait(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "wait", args, &[])
        }

        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "reset", &[], &[])
        }

        fn abort(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "abort", &[], &[])
        }

        #[getter]
        fn parties(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__getattribute__", &[Value::string("parties")], &[])
        }

        #[getter]
        fn n_waiting(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__getattribute__", &[Value::string("n_waiting")], &[])
        }

        #[getter]
        fn broken(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "__getattribute__", &[Value::string("broken")], &[])
        }
    }

    #[native_state(ProxyState)]
    class QueueProxy : "BaseProxy" {
        fn put(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let kw: Vec<(String, Value)> = kwargs.into_iter().collect();
            proxy_callmethod(_interp, self_val.0, "put", args, &kw)
        }

        fn put_nowait(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] item: PyValue,
        ) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "put_nowait", &[item.0], &[])
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let kw: Vec<(String, Value)> = kwargs.into_iter().collect();
            proxy_callmethod(_interp, self_val.0, "get", args, &kw)
        }

        fn get_nowait(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "get_nowait", &[], &[])
        }

        fn qsize(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "qsize", &[], &[])
        }

        fn empty(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "empty", &[], &[])
        }

        fn full(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "full", &[], &[])
        }

        fn task_done(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "task_done", &[], &[])
        }

        fn join(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            proxy_callmethod(_interp, self_val.0, "join", &[], &[])
        }
    }

    #[native_state(ManagerState)]
    class BaseManager {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] address: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] authkey: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _serializer: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _ctx: OptValueRef<'_>,
            #[keyword_only] #[default(OptionalArg::Omitted)] _shutdown_timeout: OptValueRef<'_>,
        ) -> Result<Value> {
            let key = match authkey.as_value() {
                Some(v) if !v.is_none() => match v.kind() {
                    ValueKind::Bytes(b) => b.as_slice().to_vec(),
                    _ => current_authkey(),
                },
                _ => current_authkey(),
            };
            let addr_hint = match address.as_value() {
                Some(v) if !v.is_none() => match v.kind() {
                    ValueKind::Tuple(t) if t.len() == 2 => {
                        let h = t[0].as_str().unwrap_or("127.0.0.1");
                        let p = t[1].as_int().unwrap_or(0);
                        Some(format!("{h}:{p}"))
                    }
                    ValueKind::Str(s) => Some(s.to_string()),
                    _ => None,
                },
                _ => None,
            };
            // Copy any class-level registered factories from `_registry` dict if set.
            let mut factories = HashMap::new();
            if let Ok(reg_val) = _interp.get_attr(self_val.0, "_cls_registry")
                && let Some(d) = reg_val.as_dict()
            {
                for (k, v) in d.iter() {
                    if let PyKey::Str(s) = k
                        && let ValueKind::Tuple(pair) = v.kind()
                        && pair.len() == 2
                    {
                        let f = if pair[0].is_none() {
                            None
                        } else {
                            Some(pair[0].clone())
                        };
                        let p = if pair[1].is_none() {
                            None
                        } else {
                            Some(pair[1].clone())
                        };
                        factories.insert(s.to_string(), (f, p));
                    }
                }
            }
            set_state(
                self_val.0,
                ManagerState {
                    authkey: key,
                    address_hint: addr_hint,
                    registry: Rc::new(RefCell::new(ManagerRegistry { factories })),
                    store: Rc::new(RefCell::new(ManagerStore {
                        next_id: 0,
                        objects: HashMap::new(),
                    })),
                    shared: None,
                    server_thread: None,
                    started: false,
                    shutdown: false,
                    shms: Vec::new(),
                },
            )?;
            Ok(Value::none())
        }

        #[classmethod]
        fn register(
            #[positional_only] cls: PyValueRef<'_>,
            #[positional_only] typeid: PyStr<'_>,
            #[default(OptionalArg::Omitted)] callable: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] proxytype: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _exposed: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _method_to_typeid: OptValueRef<'_>,
            #[default(true.into())] _create_method: crate::interpreter::builtin_args::PyBool,
        ) -> Result<Value> {
            let ValueKind::PyClass(cls_rc) = cls.0.kind() else {
                return Err(PyError::named("TypeError", "register expects a class"));
            };
            let reg_dict = {
                let existing = cls_rc.borrow().attrs.get("_cls_registry").cloned();
                match existing {
                    Some(d) => d,
                    None => {
                        let d = Value::dict(PyDict::default());
                        cls_rc
                            .borrow_mut()
                            .attrs
                            .insert("_cls_registry".to_string(), d.clone());
                        d
                    }
                }
            };
            let f_val = callable
                .as_value()
                .cloned()
                .unwrap_or_else(Value::none);
            let p_val = proxytype
                .as_value()
                .cloned()
                .unwrap_or_else(Value::none);
            reg_dict.dict_insert(
                PyKey::str_from(typeid.0),
                Value::tuple(vec![f_val, p_val]),
            )?;
            Ok(Value::none())
        }

        fn start(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] initializer: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] initargs: OptValueRef<'_>,
        ) -> Result<Value> {
            if let Some(init_fn) = initializer.as_value()
                && !init_fn.is_none()
            {
                let args = match initargs.as_value() {
                    Some(v) if !v.is_none() => _interp.collect_iterable(v)?,
                    _ => Vec::new(),
                };
                call_pos(_interp, init_fn.clone(), &args)?;
            }

            let (bind_addr, authkey, store, registry) = with_state(self_val.0, |st| {
                if st.started {
                    return Err(PyError::named("AssertionError", "manager already started"));
                }
                let a = st
                    .address_hint
                    .clone()
                    .unwrap_or_else(|| "127.0.0.1:0".to_string());
                Ok((
                    a,
                    st.authkey.clone(),
                    Rc::clone(&st.store),
                    Rc::clone(&st.registry),
                ))
            })?;

            let listener = TcpListener::bind(&bind_addr)
                .map_err(|e| PyError::from_io_error(&e, None))?;
            let _ = listener.set_nonblocking(true);
            let local_addr = listener
                .local_addr()
                .map_err(|e| PyError::from_io_error(&e, None))?
                .to_string();

            let shared = Arc::new(ManagerShared {
                addr: local_addr,
                token: authkey,
                stop: AtomicBool::new(false),
                listener: Mutex::new(Some(listener)),
                accepted: Mutex::new(Vec::new()),
            });

            let m = _interp.load_module("multiprocessing.managers")?;
            let runner_cls = _interp.get_attr(&m, "_ManagerServerRunner")?;
            let runner = call_pos(_interp, runner_cls, &[])?;
            set_native_state(
                &runner,
                ServerRunnerState {
                    shared: Arc::clone(&shared),
                    store,
                    registry,
                },
            )?;

            let th_mod = _interp.load_module("threading")?;
            let th_cls = _interp.get_attr(&th_mod, "Thread")?;
            let th = _interp.call_function_expanded(
                th_cls,
                &[
                    ExpandedCallArg {
                        name: Some("name".to_string()),
                        value: Value::string(format!("ManagerServer-{}", random_hex(3))),
                    },
                    ExpandedCallArg {
                        name: Some("target".to_string()),
                        value: runner,
                    },
                    ExpandedCallArg {
                        name: Some("daemon".to_string()),
                        value: Value::bool_(true),
                    },
                ],
            )?;
            let start_fn = _interp.get_attr(&th, "start")?;
            call_pos(_interp, start_fn, &[])?;

            with_state_mut(self_val.0, |st| {
                st.shared = Some(shared);
                st.server_thread = Some(th);
                st.started = true;
                st.shutdown = false;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn connect(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            with_state_mut(self_val.0, |st| {
                st.started = true;
                Ok(())
            })?;
            Ok(Value::none())
        }

        fn shutdown(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (shared_opt, th_opt, shms) = with_state_mut(self_val.0, |st| {
                st.shutdown = true;
                let s = st.shared.take();
                let t = st.server_thread.take();
                let sh = std::mem::take(&mut st.shms);
                Ok((s, t, sh))
            })?;
            for shm_val in shms {
                if let Ok(close_fn) = _interp.get_attr(&shm_val, "close") {
                    let _ = call_pos(_interp, close_fn, &[]);
                }
                if let Ok(unlink_fn) = _interp.get_attr(&shm_val, "unlink") {
                    let _ = call_pos(_interp, unlink_fn, &[]);
                }
            }
            if let Some(shared) = shared_opt {
                shared.stop.store(true, Ordering::Release);
                let _ = shared
                    .listener
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
            }
            if let Some(th) = th_opt {
                let join_fn = _interp.get_attr(&th, "join")?;
                let _ = call_pos(_interp, join_fn, &[Value::float(2.0)]);
            }
            Ok(Value::none())
        }

        #[getter]
        fn address(#[state] st: &ManagerState) -> Result<Value> {
            if let Some(ref s) = st.shared {
                return Ok(Value::string(&s.addr));
            }
            Ok(st
                .address_hint
                .as_deref()
                .map(Value::string)
                .unwrap_or_else(Value::none))
        }

        fn __getattr__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] name: PyStr<'_>,
        ) -> Result<Value> {
            let attr: &str = &name.0;
            let has_reg = with_state(self_val.0, |st| {
                Ok(st.registry.borrow().factories.contains_key(attr))
            })?;
            if has_reg {
                let m = _interp.load_module("multiprocessing.managers")?;
                let factory_cls = _interp.get_attr(&m, "_RegisteredFactory")?;
                return call_pos(_interp, factory_cls, &[self_val.0.clone(), Value::string(attr)]);
            }
            Err(PyError::named("AttributeError", attr))
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let already = with_state(self_val.0, |st| Ok(st.started))?;
            if !already {
                let start_fn = _interp.get_attr(self_val.0, "start")?;
                call_pos(_interp, start_fn, &[])?;
            }
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            let shut = _interp.get_attr(self_val.0, "shutdown")?;
            let _ = call_pos(_interp, shut, &[]);
            Ok(Value::none())
        }
    }

    class _RegisteredFactory {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] mgr: PyValueRef<'_>,
            #[positional_only] typeid: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_mgr", mgr.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "_typeid", Value::string(typeid.0))?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mgr = _interp.get_attr(self_val.0, "_mgr")?;
            let tid = _interp.get_attr(self_val.0, "_typeid")?;
            let typeid = tid
                .as_str()
                .ok_or_else(|| PyError::named("TypeError", "typeid must be str"))?;
            manager_create_proxy(_interp, &mgr, typeid, "BaseProxy", args, kwargs)
        }
    }

    #[native_state(ManagerState)]
    class SyncManager : "BaseManager" {
        fn dict(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(_interp, self_val.0, "dict", "DictProxy", args, kwargs)
        }

        fn list(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(_interp, self_val.0, "list", "ListProxy", args, kwargs)
        }

        #[py_name = "Namespace"]
        fn namespace(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(
                _interp,
                self_val.0,
                "Namespace",
                "NamespaceProxy",
                args,
                kwargs,
            )
        }

        #[py_name = "Value"]
        fn value(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(_interp, self_val.0, "Value", "ValueProxy", args, kwargs)
        }

        #[py_name = "Array"]
        fn array(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(_interp, self_val.0, "Array", "ArrayProxy", args, kwargs)
        }

        #[py_name = "Queue"]
        fn queue(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(_interp, self_val.0, "Queue", "QueueProxy", args, kwargs)
        }

        #[py_name = "JoinableQueue"]
        fn joinable_queue(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(
                _interp,
                self_val.0,
                "JoinableQueue",
                "QueueProxy",
                args,
                kwargs,
            )
        }

        #[py_name = "Lock"]
        fn lock(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(_interp, self_val.0, "Lock", "AcquirerProxy", args, kwargs)
        }

        #[py_name = "RLock"]
        fn rlock(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(_interp, self_val.0, "RLock", "AcquirerProxy", args, kwargs)
        }

        #[py_name = "Semaphore"]
        fn semaphore(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(
                _interp,
                self_val.0,
                "Semaphore",
                "AcquirerProxy",
                args,
                kwargs,
            )
        }

        #[py_name = "BoundedSemaphore"]
        fn bounded_semaphore(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(
                _interp,
                self_val.0,
                "BoundedSemaphore",
                "AcquirerProxy",
                args,
                kwargs,
            )
        }

        #[py_name = "Event"]
        fn event(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(_interp, self_val.0, "Event", "EventProxy", args, kwargs)
        }

        #[py_name = "Condition"]
        fn condition(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(
                _interp,
                self_val.0,
                "Condition",
                "ConditionProxy",
                args,
                kwargs,
            )
        }

        #[py_name = "Barrier"]
        fn barrier(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            manager_create_proxy(_interp, self_val.0, "Barrier", "BarrierProxy", args, kwargs)
        }
    }

    #[native_state(ManagerState)]
    class SharedMemoryManager : "BaseManager" {
        #[py_name = "SharedMemory"]
        fn shared_memory(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] size: crate::interpreter::builtin_args::PyIndex,
        ) -> Result<Value> {
            let sz = size.expect_i64(FN_NAME, "size")?;
            let sm = _interp.load_module("multiprocessing.shared_memory")?;
            let cls = _interp.get_attr(&sm, "SharedMemory")?;
            let shm_obj = call_pos(_interp, cls, &[Value::none(), Value::bool_(true), Value::int(sz)])?;
            with_state_mut(self_val.0, |st| {
                st.shms.push(shm_obj.clone());
                Ok(())
            })?;
            Ok(shm_obj)
        }

        #[py_name = "ShareableList"]
        fn shareable_list(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] sequence: PyValueRef<'_>,
        ) -> Result<Value> {
            let sm = _interp.load_module("multiprocessing.shared_memory")?;
            let cls = _interp.get_attr(&sm, "ShareableList")?;
            let sl = call_pos(_interp, cls, std::slice::from_ref(sequence.0))?;
            let shm_obj = _interp.get_attr(&sl, "shm")?;
            with_state_mut(self_val.0, |st| {
                st.shms.push(shm_obj);
                Ok(())
            })?;
            Ok(sl)
        }
    }
}
