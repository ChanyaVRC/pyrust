// `xmlrpc.server` module — 100% native-Rust Python 3.12 XML-RPC server
// implementation using PyRust's expanded macro toolkit (`pyrust_module!`,
// `class`, `#[native_state]`, `link_class_bases` in `post_load_init`).
//
// Provides `resolve_dotted_attribute`, `list_public_methods`,
// `SimpleXMLRPCDispatcher`, `SimpleXMLRPCRequestHandler`,
// `SimpleXMLRPCServer`, `XMLRPCDocGenerator`, `DocXMLRPCRequestHandler`,
// and `DocXMLRPCServer`.
//
// Reference: <https://docs.python.org/3/library/xmlrpc.server.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_pos(interp, m, args)
}

fn get_attr_truthy(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    let v = interp.get_attr(obj, name)?;
    interp.truthy_value(&v)
}

fn xs_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("xmlrpc.server")?;
    interp.get_attr(&m, name)
}

fn xc_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("xmlrpc.client")?;
    interp.get_attr(&m, name)
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn resolve_dotted_attr_impl(
    interp: &mut Interpreter,
    obj: &Value,
    attr: &str,
    allow_dotted_names: bool,
) -> Result<Value> {
    let attrs: Vec<&str> = if allow_dotted_names {
        attr.split('.').collect()
    } else {
        vec![attr]
    };
    let mut cur = obj.clone();
    for seg in attrs {
        if seg.starts_with('_') {
            return Err(PyError::named(
                "AttributeError",
                format!("attempt to access private attribute {seg:?}"),
            ));
        }
        cur = interp.get_attr(&cur, seg)?;
    }
    Ok(cur)
}

fn list_public_methods_impl(interp: &mut Interpreter, obj: &Value) -> Result<Vec<String>> {
    let dir_val = call_pos(
        interp,
        Value::builtin_function("dir"),
        std::slice::from_ref(obj),
    )?;
    let mut methods = Vec::new();
    for item in interp.collect_iterable(&dir_val)? {
        let Some(name) = item.as_str() else {
            continue;
        };
        if name.starts_with('_') {
            continue;
        }
        if let Ok(attr) = interp.get_attr(obj, name) {
            let is_call = call_pos(interp, Value::builtin_function("callable"), &[attr])?;
            if interp.truthy_value(&is_call)? {
                methods.push(name.to_string());
            }
        }
    }
    methods.sort();
    Ok(methods)
}

struct RegisterDecoratorState {
    dispatcher: Value,
    name: Value,
}

/// Wire cross-module base classes:
/// - `SimpleXMLRPCRequestHandler` -> `http.server.BaseHTTPRequestHandler`
/// - `SimpleXMLRPCServer` -> `(socketserver.TCPServer, SimpleXMLRPCDispatcher)`
/// - `DocXMLRPCRequestHandler` -> `SimpleXMLRPCRequestHandler`
/// - `DocXMLRPCServer` -> `(SimpleXMLRPCServer, XMLRPCDocGenerator)`
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let http_srv_mod = interp.load_module("http.server")?;
    let ss_mod = interp.load_module("socketserver")?;

    let base_req_h = interp.get_attr(&http_srv_mod, "BaseHTTPRequestHandler")?;
    let tcp_srv = interp.get_attr(&ss_mod, "TCPServer")?;

    let (req_h_val, srv_val, disp_val, doc_gen_val, doc_req_val, doc_srv_val) = {
        let m = module.borrow();
        (
            m.attrs.get("SimpleXMLRPCRequestHandler").cloned(),
            m.attrs.get("SimpleXMLRPCServer").cloned(),
            m.attrs.get("SimpleXMLRPCDispatcher").cloned(),
            m.attrs.get("XMLRPCDocGenerator").cloned(),
            m.attrs.get("DocXMLRPCRequestHandler").cloned(),
            m.attrs.get("DocXMLRPCServer").cloned(),
        )
    };

    if let (Some(rh), ValueKind::PyClass(base_rc)) = (req_h_val.as_ref(), base_req_h.kind())
        && let ValueKind::PyClass(rh_rc) = rh.kind()
    {
        link_class_bases(rh_rc, vec![Rc::clone(base_rc)]);
    }

    if let (Some(srv), ValueKind::PyClass(tcp_rc), Some(disp)) =
        (srv_val.as_ref(), tcp_srv.kind(), disp_val.as_ref())
        && let (ValueKind::PyClass(srv_rc), ValueKind::PyClass(disp_rc)) = (srv.kind(), disp.kind())
    {
        link_class_bases(srv_rc, vec![Rc::clone(tcp_rc), Rc::clone(disp_rc)]);
    }

    if let (Some(drh), Some(rh)) = (doc_req_val.as_ref(), req_h_val.as_ref())
        && let (ValueKind::PyClass(drh_rc), ValueKind::PyClass(rh_rc)) = (drh.kind(), rh.kind())
    {
        link_class_bases(drh_rc, vec![Rc::clone(rh_rc)]);
    }

    if let (Some(dsrv), Some(srv), Some(dg)) =
        (doc_srv_val.as_ref(), srv_val.as_ref(), doc_gen_val.as_ref())
        && let (ValueKind::PyClass(dsrv_rc), ValueKind::PyClass(srv_rc), ValueKind::PyClass(dg_rc)) =
            (dsrv.kind(), srv.kind(), dg.kind())
    {
        link_class_bases(dsrv_rc, vec![Rc::clone(srv_rc), Rc::clone(dg_rc)]);
    }

    Ok(())
}

pyrust_module! {
    fn resolve_dotted_attribute(
        obj: PyValue,
        attr: PyStr<'_>,
        #[default(true.into())] allow_dotted_names: PyBool,
    ) -> Result<Value> {
        resolve_dotted_attr_impl(_interp, &obj.0, &attr.0, allow_dotted_names.0)
    }

    fn list_public_methods(obj: PyValue) -> Result<Value> {
        let methods = list_public_methods_impl(_interp, &obj.0)?;
        Ok(Value::list(methods.into_iter().map(Value::string).collect()))
    }

    #[native_state(RegisterDecoratorState)]
    class _RegisterFunctionDecorator {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            dispatcher: PyValue,
            name: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                RegisterDecoratorState {
                    dispatcher: dispatcher.0,
                    name: name.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(#[positional_only] self_val: PyValueRef<'_>, func: PyValue) -> Result<Value> {
            let (disp, name) = with_state(self_val.0, |st| {
                Ok((st.dispatcher.clone(), st.name.clone()))
            })?;
            call_method(_interp, &disp, "register_function", &[func.0, name])
        }
    }

    class SimpleXMLRPCDispatcher {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(false.into())] allow_none: PyBool,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(false.into())] use_builtin_types: PyBool,
        ) -> Result<Value> {
            let enc = match encoding.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => Value::string("utf-8"),
            };
            _interp.assign_attr(self_val.0.clone(), "funcs", Value::dict(PyDict::default()))?;
            _interp.assign_attr(self_val.0.clone(), "instance", Value::none())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "allow_none",
                Value::bool_(allow_none.0),
            )?;
            _interp.assign_attr(self_val.0.clone(), "encoding", enc)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "use_builtin_types",
                Value::bool_(use_builtin_types.0),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "allow_dotted_names",
                Value::bool_(false),
            )?;
            Ok(Value::none())
        }

        fn register_instance(
            #[positional_only] self_val: PyValueRef<'_>,
            instance: PyValue,
            #[default(false.into())] allow_dotted_names: PyBool,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "instance", instance.0)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "allow_dotted_names",
                Value::bool_(allow_dotted_names.0),
            )?;
            Ok(Value::none())
        }

        fn register_function(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] function: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
        ) -> Result<Value> {
            let func_opt = function.as_value().filter(|v| !v.is_none());
            let name_val = name.as_value().cloned().unwrap_or_else(Value::none);
            let Some(func) = func_opt else {
                let dec_cls = xs_attr(_interp, "_RegisterFunctionDecorator")?;
                return call_pos(_interp, dec_cls, &[self_val.0.clone(), name_val]);
            };
            let fn_name = if let Some(ns) = name_val.as_str() {
                ns.to_string()
            } else {
                _interp
                    .get_attr(func, "__name__")?
                    .as_str()
                    .ok_or_else(|| PyError::named("TypeError", "function has no __name__"))?
                    .to_string()
            };
            let funcs = _interp.get_attr(self_val.0, "funcs")?;
            funcs.dict_insert(PyKey::str_from(&fn_name), func.clone())?;
            Ok(func.clone())
        }

        fn register_introspection_functions(
            #[positional_only] self_val: PyValueRef<'_>,
        ) -> Result<Value> {
            let funcs = _interp.get_attr(self_val.0, "funcs")?;
            for (rpc_name, method_name) in [
                ("system.listMethods", "system_listMethods"),
                ("system.methodSignature", "system_methodSignature"),
                ("system.methodHelp", "system_methodHelp"),
            ] {
                let bound = _interp.get_attr(self_val.0, method_name)?;
                funcs.dict_insert(PyKey::str_from(rpc_name), bound)?;
            }
            Ok(Value::none())
        }

        fn register_multicall_functions(
            #[positional_only] self_val: PyValueRef<'_>,
        ) -> Result<Value> {
            let funcs = _interp.get_attr(self_val.0, "funcs")?;
            let bound = _interp.get_attr(self_val.0, "system_multicall")?;
            funcs.dict_insert(PyKey::str_from("system.multicall"), bound)?;
            Ok(Value::none())
        }

        fn _marshaled_dispatch(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyValue,
            #[default(OptionalArg::Omitted)] dispatch_method: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _path: OptValueRef<'_>,
        ) -> Result<Value> {
            let use_bt = _interp.get_attr(self_val.0, "use_builtin_types")?;
            let loads_fn = xc_attr(_interp, "loads")?;
            let dumps_fn = xc_attr(_interp, "dumps")?;
            let allow_none = _interp.get_attr(self_val.0, "allow_none")?;
            let encoding = _interp.get_attr(self_val.0, "encoding")?;
            let fault_cls = xc_attr(_interp, "Fault")?;

            let dispatch_res: Result<Value> = (|| {
                let parsed = call_pos(_interp, loads_fn, &[data.0, Value::bool_(false), use_bt])?;
                let pair = _interp.collect_iterable(&parsed)?;
                let params = pair[0].clone();
                let method = pair[1].clone();
                let response = match dispatch_method.as_value() {
                    Some(dm) if !dm.is_none() => call_pos(_interp, dm.clone(), &[method, params])?,
                    _ => call_method(_interp, self_val.0, "_dispatch", &[method, params])?,
                };
                call_pos(
                    _interp,
                    dumps_fn.clone(),
                    &[
                        Value::tuple(vec![response]),
                        Value::none(),
                        Value::bool_(true),
                        encoding.clone(),
                        allow_none.clone(),
                    ],
                )
            })();

            let xml_str = match dispatch_res {
                Ok(s) => s,
                Err(err) => {
                    let exc = _interp.materialize_pyerror(err)?;
                    let is_fault = call_pos(
                        _interp,
                        Value::builtin_function("isinstance"),
                        &[exc.clone(), fault_cls.clone()],
                    )?;
                    let fault_obj = if _interp.truthy_value(&is_fault)? {
                        exc
                    } else {
                        let cls = crate::interpreter::value_class(&exc);
                        let ename = _interp
                            .get_attr(&cls, "__name__")
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_else(|| value_type_name_str(&exc).to_string());
                        let emsg = call_pos(_interp, Value::builtin_function("str"), &[exc])?;
                        let full = format!("<class '{ename}'>:{}", emsg.as_str().unwrap_or_default());
                        call_pos(_interp, fault_cls, &[Value::int(1), Value::string(full)])?
                    };
                    call_pos(
                        _interp,
                        dumps_fn,
                        &[
                            fault_obj,
                            Value::none(),
                            Value::bool_(false),
                            encoding.clone(),
                            allow_none,
                        ],
                    )?
                }
            };
            let xml_s = xml_str.as_str().unwrap_or_default();
            Ok(Value::bytes(xml_s.as_bytes().to_vec()))
        }

        #[py_name = "system_listMethods"]
        fn system_list_methods(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut methods: Vec<String> = Vec::new();
            let funcs = _interp.get_attr(self_val.0, "funcs")?;
            if let Some(d) = funcs.as_dict() {
                for (k, _) in d.iter() {
                    if let PyKey::Str(sv) = k
                        && let Some(ks) = sv.as_str()
                    {
                        methods.push(ks.to_string());
                    }
                }
            }
            let inst = _interp.get_attr(self_val.0, "instance")?;
            if !inst.is_none() {
                if let Ok(lm) = _interp.get_attr(&inst, "_listMethods") {
                    let extra = call_pos(_interp, lm, &[])?;
                    for item in _interp.collect_iterable(&extra)? {
                        if let Some(s) = item.as_str()
                            && !methods.iter().any(|m| m == s)
                        {
                            methods.push(s.to_string());
                        }
                    }
                } else if _interp.get_attr(&inst, "_dispatch").is_err() {
                    for s in list_public_methods_impl(_interp, &inst)? {
                        if !methods.iter().any(|m| m == &s) {
                            methods.push(s);
                        }
                    }
                }
            }
            methods.sort();
            Ok(Value::list(methods.into_iter().map(Value::string).collect()))
        }

        #[py_name = "system_methodSignature"]
        fn system_method_signature(
            #[positional_only] _self: PyValueRef<'_>,
            _method_name: PyStr<'_>,
        ) -> Result<Value> {
            Ok(Value::string("signatures not supported"))
        }

        #[py_name = "system_methodHelp"]
        fn system_method_help(
            #[positional_only] self_val: PyValueRef<'_>,
            method_name: PyStr<'_>,
        ) -> Result<Value> {
            let funcs = _interp.get_attr(self_val.0, "funcs")?;
            let method = funcs
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&method_name.0)).cloned());
            let target = match method {
                Some(f) => Some(f),
                None => {
                    let inst = _interp.get_attr(self_val.0, "instance")?;
                    if !inst.is_none() {
                        if let Ok(mh) = _interp.get_attr(&inst, "_methodHelp") {
                            return call_pos(
                                _interp,
                                mh,
                                &[Value::string(method_name.0.as_ref())],
                            );
                        }
                        let dotted = get_attr_truthy(_interp, self_val.0, "allow_dotted_names")?;
                        resolve_dotted_attr_impl(_interp, &inst, &method_name.0, dotted).ok()
                    } else {
                        None
                    }
                }
            };
            if let Some(obj) = target
                && let Ok(doc) = _interp.get_attr(&obj, "__doc__")
                && let Some(s) = doc.as_str()
            {
                return Ok(Value::string(s));
            }
            Ok(Value::string(""))
        }

        fn system_multicall(
            #[positional_only] self_val: PyValueRef<'_>,
            call_list: PyValue,
        ) -> Result<Value> {
            let mut results: Vec<Value> = Vec::new();
            let fault_cls = xc_attr(_interp, "Fault")?;
            for entry in _interp.collect_iterable(&call_list.0)? {
                let d = entry
                    .as_dict()
                    .ok_or_else(|| PyError::named("TypeError", "multicall entry must be dict"))?;
                let method_name = d
                    .get(&PyKey::str_from("methodName"))
                    .cloned()
                    .ok_or_else(|| PyError::named("KeyError", "'methodName'"))?;
                let params = d
                    .get(&PyKey::str_from("params"))
                    .cloned()
                    .ok_or_else(|| PyError::named("KeyError", "'params'"))?;
                match call_method(_interp, self_val.0, "_dispatch", &[method_name, params]) {
                    Ok(v) => results.push(Value::list(vec![v])),
                    Err(err) => {
                        let exc = _interp.materialize_pyerror(err)?;
                        let is_fault = call_pos(
                            _interp,
                            Value::builtin_function("isinstance"),
                            &[exc.clone(), fault_cls.clone()],
                        )?;
                        let mut fd = PyDict::default();
                        if _interp.truthy_value(&is_fault)? {
                            fd.insert(
                                PyKey::str_from("faultCode"),
                                _interp.get_attr(&exc, "faultCode")?,
                            );
                            fd.insert(
                                PyKey::str_from("faultString"),
                                _interp.get_attr(&exc, "faultString")?,
                            );
                        } else {
                            let cls = crate::interpreter::value_class(&exc);
                            let ename = _interp
                                .get_attr(&cls, "__name__")
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_string))
                                .unwrap_or_else(|| value_type_name_str(&exc).to_string());
                            let emsg = call_pos(_interp, Value::builtin_function("str"), &[exc])?;
                            fd.insert(PyKey::str_from("faultCode"), Value::int(1));
                            fd.insert(
                                PyKey::str_from("faultString"),
                                Value::string(format!(
                                    "<class '{ename}'>:{}",
                                    emsg.as_str().unwrap_or_default()
                                )),
                            );
                        }
                        results.push(Value::dict(fd));
                    }
                }
            }
            Ok(Value::list(results))
        }

        fn _dispatch(
            #[positional_only] self_val: PyValueRef<'_>,
            method: PyStr<'_>,
            params: PyValue,
        ) -> Result<Value> {
            let funcs = _interp.get_attr(self_val.0, "funcs")?;
            let found = funcs
                .as_dict()
                .and_then(|d| d.get(&PyKey::str_from(&method.0)).cloned());
            let func = match found {
                Some(f) => Some(f),
                None => {
                    let inst = _interp.get_attr(self_val.0, "instance")?;
                    if !inst.is_none() {
                        if let Ok(inst_disp) = _interp.get_attr(&inst, "_dispatch") {
                            return call_pos(
                                _interp,
                                inst_disp,
                                &[Value::string(method.0.as_ref()), params.0],
                            );
                        }
                        let dotted = get_attr_truthy(_interp, self_val.0, "allow_dotted_names")?;
                        resolve_dotted_attr_impl(_interp, &inst, &method.0, dotted).ok()
                    } else {
                        None
                    }
                }
            };
            let Some(callable) = func else {
                return Err(PyError::named(
                    "Exception",
                    format!("method {:?} is not supported", method.0),
                ));
            };
            let args = _interp.collect_iterable(&params.0)?;
            call_pos(_interp, callable, &args)
        }
    }

    class SimpleXMLRPCRequestHandler {
        constants {
            "rpc_paths" => Value::tuple(vec![Value::string("/"), Value::string("/RPC2")]),
            "encode_threshold" => Value::int(1400),
        }

        fn is_rpc_path_valid(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let rpc_paths = _interp.get_attr(self_val.0, "rpc_paths")?;
            let paths = _interp.collect_iterable(&rpc_paths)?;
            if paths.is_empty() {
                return Ok(Value::bool_(true));
            }
            let req_path = _interp
                .get_attr(self_val.0, "path")?
                .as_str()
                .unwrap_or("/")
                .to_string();
            let ok = paths.iter().any(|p| p.as_str() == Some(req_path.as_str()));
            Ok(Value::bool_(ok))
        }

        fn do_POST(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let valid = call_method(_interp, self_val.0, "is_rpc_path_valid", &[])?;
            if !_interp.truthy_value(&valid)? {
                return call_method(_interp, self_val.0, "report_404", &[]);
            }
            let headers = _interp.get_attr(self_val.0, "headers")?;
            let cl_val = call_method(
                _interp,
                &headers,
                "get",
                &[Value::string("content-length"), Value::string("0")],
            )?;
            let cl: i64 = cl_val
                .as_str()
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(0);
            let rfile = _interp.get_attr(self_val.0, "rfile")?;
            let data = call_method(_interp, &rfile, "read", &[Value::int(cl)])?;
            let server = _interp.get_attr(self_val.0, "server")?;
            let path = _interp.get_attr(self_val.0, "path")?;
            let response = call_method(
                _interp,
                &server,
                "_marshaled_dispatch",
                &[data, Value::none(), path],
            )?;
            let resp_bytes = match response.kind() {
                ValueKind::Bytes(b) => b.as_ref().clone(),
                ValueKind::Str(s) => s.as_bytes().to_vec(),
                _ => Vec::new(),
            };
            call_method(_interp, self_val.0, "send_response", &[Value::int(200)])?;
            call_method(
                _interp,
                self_val.0,
                "send_header",
                &[Value::string("Content-type"), Value::string("text/xml")],
            )?;
            call_method(
                _interp,
                self_val.0,
                "send_header",
                &[
                    Value::string("Content-length"),
                    Value::string(resp_bytes.len().to_string()),
                ],
            )?;
            call_method(_interp, self_val.0, "end_headers", &[])?;
            let wfile = _interp.get_attr(self_val.0, "wfile")?;
            call_method(_interp, &wfile, "write", &[Value::bytes(resp_bytes)])?;
            Ok(Value::none())
        }

        fn report_404(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let body = b"No such page".to_vec();
            call_method(_interp, self_val.0, "send_response", &[Value::int(404)])?;
            call_method(
                _interp,
                self_val.0,
                "send_header",
                &[Value::string("Content-type"), Value::string("text/plain")],
            )?;
            call_method(
                _interp,
                self_val.0,
                "send_header",
                &[
                    Value::string("Content-length"),
                    Value::string(body.len().to_string()),
                ],
            )?;
            call_method(_interp, self_val.0, "end_headers", &[])?;
            let wfile = _interp.get_attr(self_val.0, "wfile")?;
            call_method(_interp, &wfile, "write", &[Value::bytes(body)])?;
            Ok(Value::none())
        }

        fn log_request(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] code: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] size: OptValueRef<'_>,
        ) -> Result<Value> {
            let server = _interp.get_attr(self_val.0, "server")?;
            let log_reqs = _interp
                .get_attr(&server, "logRequests")
                .map(|v| _interp.truthy_value(&v).unwrap_or(false))
                .unwrap_or(false);
            if log_reqs {
                let http_srv_mod = _interp.load_module("http.server")?;
                let base_cls = _interp.get_attr(&http_srv_mod, "BaseHTTPRequestHandler")?;
                let base_log = _interp.get_attr(&base_cls, "log_request")?;
                let c = code
                    .as_value()
                    .cloned()
                    .unwrap_or_else(|| Value::string("-"));
                let s = size
                    .as_value()
                    .cloned()
                    .unwrap_or_else(|| Value::string("-"));
                call_pos(_interp, base_log, &[self_val.0.clone(), c, s])?;
            }
            Ok(Value::none())
        }
    }

    class SimpleXMLRPCServer {
        constants {
            "allow_reuse_address" => Value::bool_(true),
            "_send_traceback_header" => Value::bool_(false),
        }

        #[allow(clippy::too_many_arguments, non_snake_case)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            addr: PyValue,
            #[default(OptionalArg::Omitted)] requestHandler: OptValueRef<'_>,
            #[default(true.into())] logRequests: PyBool,
            #[default(false.into())] allow_none: PyBool,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(true.into())] bind_and_activate: PyBool,
            #[default(false.into())] use_builtin_types: PyBool,
        ) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "logRequests",
                Value::bool_(logRequests.0),
            )?;
            let disp_cls = xs_attr(_interp, "SimpleXMLRPCDispatcher")?;
            let disp_init = _interp.get_attr(&disp_cls, "__init__")?;
            let enc = encoding.as_value().cloned().unwrap_or_else(Value::none);
            call_pos(
                _interp,
                disp_init,
                &[
                    self_val.0.clone(),
                    Value::bool_(allow_none.0),
                    enc,
                    Value::bool_(use_builtin_types.0),
                ],
            )?;
            let handler_cls = match requestHandler.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => xs_attr(_interp, "SimpleXMLRPCRequestHandler")?,
            };
            let ss_mod = _interp.load_module("socketserver")?;
            let tcp_cls = _interp.get_attr(&ss_mod, "TCPServer")?;
            let tcp_init = _interp.get_attr(&tcp_cls, "__init__")?;
            call_pos(
                _interp,
                tcp_init,
                &[
                    self_val.0.clone(),
                    addr.0,
                    handler_cls,
                    Value::bool_(bind_and_activate.0),
                ],
            )?;
            Ok(Value::none())
        }
    }

    class XMLRPCDocGenerator {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "server_name",
                Value::string("XML-RPC Server Documentation"),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "server_documentation",
                Value::string("This server exports the following methods through the XML-RPC protocol."),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "server_title",
                Value::string("XML-RPC Server Documentation"),
            )?;
            Ok(Value::none())
        }

        fn set_server_title(
            #[positional_only] self_val: PyValueRef<'_>,
            server_title: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "server_title",
                Value::string(server_title.0.as_ref()),
            )?;
            Ok(Value::none())
        }

        fn set_server_name(
            #[positional_only] self_val: PyValueRef<'_>,
            server_name: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "server_name",
                Value::string(server_name.0.as_ref()),
            )?;
            Ok(Value::none())
        }

        fn set_server_documentation(
            #[positional_only] self_val: PyValueRef<'_>,
            server_documentation: PyStr<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "server_documentation",
                Value::string(server_documentation.0.as_ref()),
            )?;
            Ok(Value::none())
        }

        fn generate_html_documentation(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let title = _interp
                .get_attr(self_val.0, "server_title")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "XML-RPC Server Documentation".to_string());
            let name = _interp
                .get_attr(self_val.0, "server_name")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| title.clone());
            let doc = _interp
                .get_attr(self_val.0, "server_documentation")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let methods_val = call_method(_interp, self_val.0, "system_listMethods", &[])?;
            let mut body = format!(
                "<!DOCTYPE html>\n<html><head><title>{}</title></head><body><h1>{}</h1><p>{}</p><dl>",
                html_escape(&title),
                html_escape(&name),
                html_escape(&doc),
            );
            for m in _interp.collect_iterable(&methods_val)? {
                let mname = m.as_str().unwrap_or_default();
                let help = call_method(
                    _interp,
                    self_val.0,
                    "system_methodHelp",
                    &[Value::string(mname)],
                )?;
                let hstr = help.as_str().unwrap_or_default();
                body.push_str(&format!(
                    "<dt><strong>{}</strong></dt><dd>{}</dd>",
                    html_escape(mname),
                    html_escape(hstr)
                ));
            }
            body.push_str("</dl></body></html>\n");
            Ok(Value::string(body))
        }
    }

    class DocXMLRPCRequestHandler {
        fn do_GET(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let valid = call_method(_interp, self_val.0, "is_rpc_path_valid", &[])?;
            if !_interp.truthy_value(&valid)? {
                return call_method(_interp, self_val.0, "report_404", &[]);
            }
            let server = _interp.get_attr(self_val.0, "server")?;
            let doc_html = call_method(_interp, &server, "generate_html_documentation", &[])?;
            let body = doc_html.as_str().unwrap_or_default().as_bytes().to_vec();
            call_method(_interp, self_val.0, "send_response", &[Value::int(200)])?;
            call_method(
                _interp,
                self_val.0,
                "send_header",
                &[Value::string("Content-type"), Value::string("text/html")],
            )?;
            call_method(
                _interp,
                self_val.0,
                "send_header",
                &[
                    Value::string("Content-length"),
                    Value::string(body.len().to_string()),
                ],
            )?;
            call_method(_interp, self_val.0, "end_headers", &[])?;
            let wfile = _interp.get_attr(self_val.0, "wfile")?;
            call_method(_interp, &wfile, "write", &[Value::bytes(body)])?;
            Ok(Value::none())
        }
    }

    class DocXMLRPCServer {
        #[allow(clippy::too_many_arguments, non_snake_case)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            addr: PyValue,
            #[default(OptionalArg::Omitted)] requestHandler: OptValueRef<'_>,
            #[default(true.into())] logRequests: PyBool,
            #[default(false.into())] allow_none: PyBool,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(true.into())] bind_and_activate: PyBool,
            #[default(false.into())] use_builtin_types: PyBool,
        ) -> Result<Value> {
            let handler_cls = match requestHandler.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => xs_attr(_interp, "DocXMLRPCRequestHandler")?,
            };
            let srv_cls = xs_attr(_interp, "SimpleXMLRPCServer")?;
            let srv_init = _interp.get_attr(&srv_cls, "__init__")?;
            let enc = encoding.as_value().cloned().unwrap_or_else(Value::none);
            call_pos(
                _interp,
                srv_init,
                &[
                    self_val.0.clone(),
                    addr.0,
                    handler_cls,
                    Value::bool_(logRequests.0),
                    Value::bool_(allow_none.0),
                    enc,
                    Value::bool_(bind_and_activate.0),
                    Value::bool_(use_builtin_types.0),
                ],
            )?;
            let dg_cls = xs_attr(_interp, "XMLRPCDocGenerator")?;
            let dg_init = _interp.get_attr(&dg_cls, "__init__")?;
            call_pos(_interp, dg_init, std::slice::from_ref(self_val.0))?;
            Ok(Value::none())
        }
    }
}
