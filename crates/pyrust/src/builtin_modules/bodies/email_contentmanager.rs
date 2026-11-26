// `email.contentmanager` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// `ContentManager`, `raw_data_manager`, and the built-in text/bytes/message
// get & set content handlers.
//
// Reference: <https://docs.python.org/3/library/email.contentmanager.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyBytesLike, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value};

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    call_kw(interp, callee, args, &[])
}

fn call_varargs_kwargs(
    interp: &mut Interpreter,
    callee: Value,
    prefix: &[Value],
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Value> {
    let mut expanded = Vec::with_capacity(prefix.len() + args.len() + kwargs.len());
    for v in prefix {
        expanded.push(ExpandedCallArg {
            name: None,
            value: v.clone(),
        });
    }
    for v in args {
        expanded.push(ExpandedCallArg {
            name: None,
            value: v.clone(),
        });
    }
    for (k, v) in kwargs {
        expanded.push(ExpandedCallArg {
            name: Some(k.clone()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn dict_lookup(interp: &mut Interpreter, dict: &Value, key: &Value) -> Result<Option<Value>> {
    let contains = interp.get_attr(dict, "__contains__")?;
    let has = call_pos(interp, contains, std::slice::from_ref(key))?;
    if interp.truthy_value(&has)? {
        let getitem = interp.get_attr(dict, "__getitem__")?;
        Ok(Some(call_pos(interp, getitem, std::slice::from_ref(key))?))
    } else {
        Ok(None)
    }
}

fn dict_set(interp: &mut Interpreter, dict: &Value, key: Value, val: Value) -> Result<()> {
    let setitem = interp.get_attr(dict, "__setitem__")?;
    call_pos(interp, setitem, &[key, val])?;
    Ok(())
}

fn prepare_set(
    interp: &mut Interpreter,
    msg: &Value,
    maintype: &str,
    subtype: &str,
    headers: Option<&Value>,
) -> Result<()> {
    let setitem = interp.get_attr(msg, "__setitem__")?;
    call_pos(
        interp,
        setitem.clone(),
        &[
            Value::string("Content-Type"),
            Value::string(format!("{maintype}/{subtype}")),
        ],
    )?;
    let Some(hdrs_val) = headers.filter(|v| !v.is_none()) else {
        return Ok(());
    };
    if !interp.truthy_value(hdrs_val)? {
        return Ok(());
    }
    let hdrs = interp.collect_iterable(hdrs_val)?;
    let mp = interp.get_attr(msg, "policy")?;
    for hdr in hdrs {
        if let Some(s) = hdr.as_str() {
            let hsp = interp.get_attr(&mp, "header_source_parse")?;
            let pair = call_pos(interp, hsp, &[Value::list(vec![Value::string(s)])])?;
            let items = interp.collect_iterable(&pair)?;
            if items.len() >= 2 {
                if let Ok(hf) = interp.get_attr(&mp, "header_factory")
                    && !hf.is_none()
                {
                    let parsed = call_pos(interp, hf, &[items[0].clone(), items[1].clone()])?;
                    if let Ok(defects) = interp.get_attr(&parsed, "defects")
                        && interp.truthy_value(&defects)?
                    {
                        let dlist = interp.collect_iterable(&defects)?;
                        if let Some(first_def) = dlist.first() {
                            return Err(PyError::named(
                                "ValueError",
                                format!(
                                    "Invalid header: {}",
                                    super::email_errors::py_str(interp, first_def)?
                                ),
                            ));
                        }
                    }
                    let hname = interp.get_attr(&parsed, "name")?;
                    call_pos(interp, setitem.clone(), &[hname, parsed])?;
                } else {
                    call_pos(
                        interp,
                        setitem.clone(),
                        &[items[0].clone(), items[1].clone()],
                    )?;
                }
            }
        } else {
            if let Ok(defects) = interp.get_attr(&hdr, "defects")
                && interp.truthy_value(&defects)?
            {
                return Err(PyError::named(
                    "ValueError",
                    format!(
                        "Invalid header: {}",
                        super::email_errors::py_str(interp, &hdr)?
                    ),
                ));
            }
            let hname = interp.get_attr(&hdr, "name")?;
            call_pos(interp, setitem.clone(), &[hname, hdr])?;
        }
    }
    Ok(())
}

fn finalize_set(
    interp: &mut Interpreter,
    msg: &Value,
    disposition: Option<&Value>,
    filename: Option<&Value>,
    cid: Option<&Value>,
    params: Option<&Value>,
) -> Result<()> {
    let fname = filename.filter(|v| !v.is_none());
    let disp = match disposition.filter(|v| !v.is_none()) {
        Some(v) => Some(v.clone()),
        None if fname.is_some() => Some(Value::string("attachment")),
        None => None,
    };
    if let Some(d) = disp {
        let setitem = interp.get_attr(msg, "__setitem__")?;
        call_pos(interp, setitem, &[Value::string("Content-Disposition"), d])?;
    }
    if let Some(f) = fname {
        let set_param = interp.get_attr(msg, "set_param")?;
        call_kw(
            interp,
            set_param,
            &[Value::string("filename"), f.clone()],
            &[
                ("header", Value::string("Content-Disposition")),
                ("replace", Value::bool_(true)),
            ],
        )?;
    }
    if let Some(c) = cid.filter(|v| !v.is_none()) {
        let setitem = interp.get_attr(msg, "__setitem__")?;
        call_pos(interp, setitem, &[Value::string("Content-ID"), c.clone()])?;
    }
    if let Some(p) = params.filter(|v| !v.is_none()) {
        let items_fn = interp.get_attr(p, "items")?;
        let items_val = call_pos(interp, items_fn, &[])?;
        let pairs = interp.collect_iterable(&items_val)?;
        let set_param = interp.get_attr(msg, "set_param")?;
        for pair in pairs {
            let kv = interp.collect_iterable(&pair)?;
            if kv.len() >= 2 {
                call_pos(interp, set_param.clone(), &[kv[0].clone(), kv[1].clone()])?;
            }
        }
    }
    Ok(())
}

fn encode_base64_body(data: &[u8], max_line_length: usize) -> String {
    let max_ll = if max_line_length == 0 {
        78
    } else {
        max_line_length
    };
    let chunk_size = (max_ll / 4 * 3).max(3);
    let mut out = String::new();
    for chunk in data.chunks(chunk_size) {
        out.push_str(&super::email_base64mime::b64_encode_bytes(chunk));
        out.push('\n');
    }
    out
}

fn split_bytes_lines(data: &[u8]) -> Vec<&[u8]> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut i = 0;
    let mut start = 0;
    let n = data.len();
    while i < n {
        if data[i] == b'\r' {
            out.push(&data[start..i]);
            if i + 1 < n && data[i + 1] == b'\n' {
                i += 2;
            } else {
                i += 1;
            }
            start = i;
        } else if data[i] == b'\n' {
            out.push(&data[start..i]);
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < n {
        out.push(&data[start..n]);
    }
    out
}

fn join_bytes_lines(lines: &[&[u8]], sep: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for line in lines {
        out.extend_from_slice(line);
        out.extend_from_slice(sep);
    }
    if lines.is_empty() {
        out.extend_from_slice(sep);
    }
    out
}

fn bytes_to_surrogate_str(interp: &mut Interpreter, data: &[u8]) -> Result<Value> {
    let b = Value::bytes(data.to_vec());
    let dec = interp.get_attr(&b, "decode")?;
    call_pos(
        interp,
        dec,
        &[Value::string("ascii"), Value::string("surrogateescape")],
    )
}

fn encode_text_body(
    interp: &mut Interpreter,
    string: &str,
    charset: &str,
    cte_opt: Option<&str>,
    policy: &Value,
) -> Result<(String, Value)> {
    let s_val = Value::string(string);
    let enc_fn = interp.get_attr(&s_val, "encode")?;
    let norm_cs = if charset.eq_ignore_ascii_case("us-ascii") {
        "ascii"
    } else {
        charset
    };
    let encoded_val = call_pos(interp, enc_fn, &[Value::string(norm_cs)])?;
    let bl = PyBytesLike::try_from_value_with_interp(
        interp,
        &encoded_val,
        "set_text_content",
        "string",
    )?;
    let encoded_bytes = bl.as_bytes();
    let lines = split_bytes_lines(encoded_bytes);

    let linesep_val = interp.get_attr(policy, "linesep")?;
    let linesep = linesep_val.as_str().unwrap_or("\n").as_bytes();
    let max_ll_val = interp.get_attr(policy, "max_line_length")?;
    let max_ll: usize = max_ll_val
        .as_int()
        .filter(|&v| v > 0)
        .map(|v| v as usize)
        .unwrap_or(usize::MAX);
    let qp_ll: i64 = max_ll_val.as_int().filter(|&v| v >= 4).unwrap_or(76);
    let cte_type = interp
        .get_attr(policy, "cte_type")
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "8bit".to_string());

    let normal = join_bytes_lines(&lines, b"\n");

    let chosen_cte = if let Some(c) = cte_opt {
        c.to_string()
    } else {
        let max_line = lines.iter().map(|l| l.len()).max().unwrap_or(0);
        if max_line <= max_ll {
            if normal.iter().all(|&b| b < 128) {
                let s = String::from_utf8(normal).unwrap_or_default();
                return Ok(("7bit".to_string(), Value::string(s)));
            }
            if cte_type == "8bit" {
                let s_val = bytes_to_surrogate_str(interp, &normal)?;
                return Ok(("8bit".to_string(), s_val));
            }
        }
        let sniff_lines = &lines[..lines.len().min(10)];
        let sniff = join_bytes_lines(sniff_lines, linesep);
        let sniff_latin1: String = sniff.iter().map(|&b| b as char).collect();
        let sniff_qp = super::email_quoprimime::qp_body_encode_str(&sniff_latin1, qp_ll, "\n")?;
        let sniff_b64 = encode_base64_body(&sniff, if max_ll == usize::MAX { 78 } else { max_ll });
        if sniff_qp.len() > sniff_b64.len() {
            "base64".to_string()
        } else {
            if lines.len() <= 10 {
                return Ok(("quoted-printable".to_string(), Value::string(sniff_qp)));
            }
            "quoted-printable".to_string()
        }
    };

    let data_val = match chosen_cte.as_str() {
        "7bit" => {
            if !normal.iter().all(|&b| b < 128) {
                return Err(PyError::named(
                    "UnicodeDecodeError",
                    "'ascii' codec can't decode byte".to_string(),
                ));
            }
            Value::string(String::from_utf8(normal).unwrap_or_default())
        }
        "8bit" => bytes_to_surrogate_str(interp, &normal)?,
        "quoted-printable" => {
            let latin1: String = normal.iter().map(|&b| b as char).collect();
            Value::string(super::email_quoprimime::qp_body_encode_str(
                &latin1, qp_ll, "\n",
            )?)
        }
        "base64" => {
            let embedded = join_bytes_lines(&lines, linesep);
            Value::string(encode_base64_body(
                &embedded,
                if max_ll == usize::MAX { 78 } else { max_ll },
            ))
        }
        other => {
            return Err(PyError::named(
                "ValueError",
                format!("Unknown content transfer encoding {other}"),
            ));
        }
    };
    Ok((chosen_cte, data_val))
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let mod_val = Value::py_module(Rc::clone(module));
    let cm_cls = interp.get_attr(&mod_val, "ContentManager")?;
    let rdm = call_pos(interp, cm_cls, &[])?;
    let add_get = interp.get_attr(&rdm, "add_get_handler")?;
    let add_set = interp.get_attr(&rdm, "add_set_handler")?;

    let get_text = interp.get_attr(&mod_val, "get_text_content")?;
    call_pos(interp, add_get.clone(), &[Value::string("text"), get_text])?;

    let get_non_text = interp.get_attr(&mod_val, "get_non_text_content")?;
    for mt in ["audio", "image", "video", "application"] {
        call_pos(
            interp,
            add_get.clone(),
            &[Value::string(mt), get_non_text.clone()],
        )?;
    }

    let get_msg = interp.get_attr(&mod_val, "get_message_content")?;
    for st in ["message/rfc822", "message/external-body"] {
        call_pos(
            interp,
            add_get.clone(),
            &[Value::string(st), get_msg.clone()],
        )?;
    }

    let get_unk = interp.get_attr(&mod_val, "get_and_fixup_unknown_message_content")?;
    call_pos(interp, add_get, &[Value::string("message"), get_unk])?;

    let set_text = interp.get_attr(&mod_val, "set_text_content")?;
    let str_cls = Value::py_class(
        crate::interpreter::primitive_class_by_name("str")
            .ok_or_else(|| PyError::runtime("str class not found".to_string()))?,
    );
    call_pos(interp, add_set.clone(), &[str_cls, set_text])?;

    let set_msg = interp.get_attr(&mod_val, "set_message_content")?;
    // Register by class name "Message" so circular import during `email.policy`
    // initialization never fails, matching CPython's MRO name lookup.
    call_pos(
        interp,
        add_set.clone(),
        &[Value::string("Message"), set_msg],
    )?;

    let set_bytes = interp.get_attr(&mod_val, "set_bytes_content")?;
    for prim in ["bytes", "bytearray", "memoryview"] {
        if let Some(cls) = crate::interpreter::primitive_class_by_name(prim) {
            call_pos(
                interp,
                add_set.clone(),
                &[Value::py_class(cls), set_bytes.clone()],
            )?;
        }
    }

    module
        .borrow_mut()
        .attrs
        .insert("raw_data_manager".to_string(), rdm);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
    }

    class ContentManager {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "get_handlers",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "set_handlers",
                Value::dict(PyDict::default()),
            )?;
            Ok(Value::none())
        }

        fn add_get_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            handler: PyValue,
        ) -> Result<Value> {
            let handlers = _interp.get_attr(self_val.0, "get_handlers")?;
            dict_set(_interp, &handlers, key.0, handler.0)?;
            Ok(Value::none())
        }

        fn get_content(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
            #[varargs] args: &[Value],
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let handlers = _interp.get_attr(self_val.0, "get_handlers")?;
            let get_ct = _interp.get_attr(&msg.0, "get_content_type")?;
            let content_type = call_pos(_interp, get_ct, &[])?;
            if let Some(h) = dict_lookup(_interp, &handlers, &content_type)? {
                return call_varargs_kwargs(_interp, h, std::slice::from_ref(&msg.0), args, &kw);
            }
            let get_mt = _interp.get_attr(&msg.0, "get_content_maintype")?;
            let maintype = call_pos(_interp, get_mt, &[])?;
            if let Some(h) = dict_lookup(_interp, &handlers, &maintype)? {
                return call_varargs_kwargs(_interp, h, std::slice::from_ref(&msg.0), args, &kw);
            }
            let empty_k = Value::string("");
            if let Some(h) = dict_lookup(_interp, &handlers, &empty_k)? {
                return call_varargs_kwargs(_interp, h, std::slice::from_ref(&msg.0), args, &kw);
            }
            Err(PyError::named(
                "KeyError",
                content_type.as_str().unwrap_or("").to_string(),
            ))
        }

        fn add_set_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            typekey: PyValue,
            handler: PyValue,
        ) -> Result<Value> {
            let handlers = _interp.get_attr(self_val.0, "set_handlers")?;
            dict_set(_interp, &handlers, typekey.0, handler.0)?;
            Ok(Value::none())
        }

        fn set_content(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
            obj: PyValue,
            #[varargs] args: &[Value],
            #[varkwargs] kw: IndexMap<String, Value>,
        ) -> Result<Value> {
            let get_mt = _interp.get_attr(&msg.0, "get_content_maintype")?;
            let mt = call_pos(_interp, get_mt, &[])?;
            if mt.as_str() == Some("multipart") {
                return Err(PyError::named(
                    "TypeError",
                    "set_content not valid on multipart".to_string(),
                ));
            }
            let find_fn = _interp.get_attr(self_val.0, "_find_set_handler")?;
            let handler = call_pos(_interp, find_fn, &[msg.0.clone(), obj.0.clone()])?;
            let clear_fn = _interp.get_attr(&msg.0, "clear_content")?;
            call_pos(_interp, clear_fn, &[])?;
            call_varargs_kwargs(_interp, handler, &[msg.0, obj.0], args, &kw)?;
            Ok(Value::none())
        }

        fn _find_set_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            _msg: PyValue,
            obj: PyValue,
        ) -> Result<Value> {
            let handlers = _interp.get_attr(self_val.0, "set_handlers")?;
            let typ = call_pos(_interp, Value::builtin_function("type"), &[obj.0])?;
            let mro_val = _interp.get_attr(&typ, "__mro__")?;
            let mro = _interp.collect_iterable(&mro_val)?;
            let mut full_path_for_error: Option<String> = None;
            for t in mro {
                if let Some(h) = dict_lookup(_interp, &handlers, &t)? {
                    return Ok(h);
                }
                let name = _interp
                    .get_attr(&t, "__name__")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let qname = _interp
                    .get_attr(&t, "__qualname__")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_else(|| name.clone());
                let modname = _interp
                    .get_attr(&t, "__module__")
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let full_path = if modname.is_empty() {
                    qname.clone()
                } else {
                    format!("{modname}.{qname}")
                };
                if full_path_for_error.is_none() {
                    full_path_for_error = Some(full_path.clone());
                }
                for candidate in [&full_path, &qname, &name] {
                    if let Some(h) = dict_lookup(_interp, &handlers, &Value::string(candidate))? {
                        return Ok(h);
                    }
                }
            }
            if let Some(h) = dict_lookup(_interp, &handlers, &Value::none())? {
                return Ok(h);
            }
            Err(PyError::named(
                "KeyError",
                full_path_for_error.unwrap_or_default(),
            ))
        }
    }

    fn get_text_content(
        msg: PyValueRef<'_>,
        #[default("replace".into())] errors: PyStr<'_>,
    ) -> Result<Value> {
        let gp = _interp.get_attr(msg.0, "get_payload")?;
        let content = call_kw(_interp, gp, &[], &[("decode", Value::bool_(true))])?;
        let get_param = _interp.get_attr(msg.0, "get_param")?;
        let charset = call_pos(
            _interp,
            get_param,
            &[Value::string("charset"), Value::string("ASCII")],
        )?;
        let decode_fn = _interp.get_attr(&content, "decode")?;
        call_kw(
            _interp,
            decode_fn,
            &[charset],
            &[("errors", Value::string(errors.0.as_ref()))],
        )
    }

    fn get_non_text_content(msg: PyValueRef<'_>) -> Result<Value> {
        let gp = _interp.get_attr(msg.0, "get_payload")?;
        call_kw(_interp, gp, &[], &[("decode", Value::bool_(true))])
    }

    fn get_message_content(msg: PyValueRef<'_>) -> Result<Value> {
        let gp = _interp.get_attr(msg.0, "get_payload")?;
        call_pos(_interp, gp, &[Value::int(0)])
    }

    fn get_and_fixup_unknown_message_content(msg: PyValueRef<'_>) -> Result<Value> {
        let gp = _interp.get_attr(msg.0, "get_payload")?;
        let inner = call_pos(_interp, gp, &[Value::int(0)])?;
        let bytes_cls = Value::py_class(
            crate::interpreter::primitive_class_by_name("bytes")
                .ok_or_else(|| PyError::runtime("bytes class not found".to_string()))?,
        );
        call_pos(_interp, bytes_cls, &[inner])
    }

    fn set_text_content(
        msg: PyValueRef<'_>,
        string: PyStr<'_>,
        #[default("plain".into())] subtype: PyStr<'_>,
        #[default("utf-8".into())] charset: PyStr<'_>,
        #[default(OptionalArg::Omitted)] cte: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] disposition: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] cid: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] params: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] headers: OptValueRef<'_>,
    ) -> Result<Value> {
        prepare_set(_interp, msg.0, "text", subtype.0.as_ref(), headers.as_value())?;
        let policy = _interp.get_attr(msg.0, "policy")?;
        let cte_str = cte
            .as_value()
            .filter(|v| !v.is_none())
            .and_then(|v| v.as_str());
        let (final_cte, payload) =
            encode_text_body(_interp, string.0.as_ref(), charset.0.as_ref(), cte_str, &policy)?;
        let sp = _interp.get_attr(msg.0, "set_payload")?;
        call_pos(_interp, sp, &[payload])?;

        let cs_mod = _interp.load_module("email.charset")?;
        let aliases = _interp.get_attr(&cs_mod, "ALIASES")?;
        let canonical_cs = aliases
            .as_dict()
            .and_then(|d| d.get(&PyKey::str_from(charset.0.as_ref())).cloned())
            .unwrap_or_else(|| Value::string(charset.0.as_ref()));
        let set_param = _interp.get_attr(msg.0, "set_param")?;
        call_kw(
            _interp,
            set_param,
            &[Value::string("charset"), canonical_cs],
            &[("replace", Value::bool_(true))],
        )?;

        let setitem = _interp.get_attr(msg.0, "__setitem__")?;
        call_pos(
            _interp,
            setitem,
            &[
                Value::string("Content-Transfer-Encoding"),
                Value::string(final_cte),
            ],
        )?;
        finalize_set(
            _interp,
            msg.0,
            disposition.as_value(),
            filename.as_value(),
            cid.as_value(),
            params.as_value(),
        )?;
        Ok(Value::none())
    }

    fn set_message_content(
        msg: PyValueRef<'_>,
        message: PyValue,
        #[default("rfc822".into())] subtype: PyStr<'_>,
        #[default(OptionalArg::Omitted)] cte: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] disposition: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] cid: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] params: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] headers: OptValueRef<'_>,
    ) -> Result<Value> {
        let sub = subtype.0.as_ref();
        if sub == "partial" {
            return Err(PyError::named(
                "ValueError",
                "message/partial is not supported for Message objects".to_string(),
            ));
        }
        let cte_opt = cte
            .as_value()
            .filter(|v| !v.is_none())
            .and_then(|v| v.as_str());
        let final_cte = if sub == "rfc822" {
            match cte_opt {
                None => "8bit",
                Some(c @ ("7bit" | "8bit" | "binary")) => c,
                Some(other) => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("message/rfc822 parts do not support cte={other}"),
                    ));
                }
            }
        } else if sub == "external-body" {
            match cte_opt {
                None | Some("7bit") => "7bit",
                Some(other) => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("message/external-body parts do not support cte={other}"),
                    ));
                }
            }
        } else {
            cte_opt.unwrap_or("7bit")
        };

        prepare_set(_interp, msg.0, "message", sub, headers.as_value())?;
        let sp = _interp.get_attr(msg.0, "set_payload")?;
        call_pos(_interp, sp, &[Value::list(vec![message.0])])?;
        let setitem = _interp.get_attr(msg.0, "__setitem__")?;
        call_pos(
            _interp,
            setitem,
            &[
                Value::string("Content-Transfer-Encoding"),
                Value::string(final_cte),
            ],
        )?;
        finalize_set(
            _interp,
            msg.0,
            disposition.as_value(),
            filename.as_value(),
            cid.as_value(),
            params.as_value(),
        )?;
        Ok(Value::none())
    }

    fn set_bytes_content(
        msg: PyValueRef<'_>,
        data: PyBytesLike<'_>,
        maintype: PyStr<'_>,
        subtype: PyStr<'_>,
        #[default("base64".into())] cte: PyStr<'_>,
        #[default(OptionalArg::Omitted)] disposition: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] filename: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] cid: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] params: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] headers: OptValueRef<'_>,
    ) -> Result<Value> {
        let raw = data.as_bytes();
        prepare_set(
            _interp,
            msg.0,
            maintype.0.as_ref(),
            subtype.0.as_ref(),
            headers.as_value(),
        )?;
        let policy = _interp.get_attr(msg.0, "policy")?;
        let max_ll = _interp
            .get_attr(&policy, "max_line_length")
            .ok()
            .and_then(|v| v.as_int())
            .filter(|&v| v > 0)
            .map(|v| v as usize)
            .unwrap_or(78);

        let payload_val = match cte.0.as_ref() {
            "base64" => Value::string(encode_base64_body(raw, max_ll)),
            "quoted-printable" => {
                let binascii = _interp.load_module("binascii")?;
                let b2a_qp = _interp.get_attr(&binascii, "b2a_qp")?;
                let qp_bytes = call_kw(
                    _interp,
                    b2a_qp,
                    &[Value::bytes(raw.to_vec())],
                    &[
                        ("istext", Value::bool_(false)),
                        ("header", Value::bool_(false)),
                        ("quotetabs", Value::bool_(true)),
                    ],
                )?;
                let dec = _interp.get_attr(&qp_bytes, "decode")?;
                call_pos(_interp, dec, &[Value::string("ascii")])?
            }
            "7bit" => {
                let b = Value::bytes(raw.to_vec());
                let dec = _interp.get_attr(&b, "decode")?;
                call_pos(_interp, dec, &[Value::string("ascii")])?
            }
            "8bit" | "binary" => bytes_to_surrogate_str(_interp, raw)?,
            other => {
                return Err(PyError::named(
                    "ValueError",
                    format!("Unknown content transfer encoding {other}"),
                ));
            }
        };

        let sp = _interp.get_attr(msg.0, "set_payload")?;
        call_pos(_interp, sp, &[payload_val])?;
        let setitem = _interp.get_attr(msg.0, "__setitem__")?;
        call_pos(
            _interp,
            setitem,
            &[
                Value::string("Content-Transfer-Encoding"),
                Value::string(cte.0.as_ref()),
            ],
        )?;
        finalize_set(
            _interp,
            msg.0,
            disposition.as_value(),
            filename.as_value(),
            cid.as_value(),
            params.as_value(),
        )?;
        Ok(Value::none())
    }
}
