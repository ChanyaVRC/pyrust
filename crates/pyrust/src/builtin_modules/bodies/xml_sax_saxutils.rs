// `xml.sax.saxutils` module — 100% native-Rust Python 3.12 SAX utilities:
// `escape`, `unescape`, `quoteattr`, `XMLGenerator`, `XMLFilterBase`, and
// `prepare_input_source` (issue #3260).
//
// Reference: <https://docs.python.org/3/library/xml.sax.utils.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyModule, Value, ValueKind};

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

fn replace_entities(
    interp: &mut Interpreter,
    mut data: String,
    entities: &Value,
) -> Result<String> {
    let items = call_method(interp, entities, "items", &[])?;
    for pair in interp.collect_iterable(&items)? {
        let kv = interp.collect_iterable(&pair)?;
        if kv.len() == 2 {
            let k = kv[0]
                .as_str()
                .ok_or_else(|| PyError::named("TypeError", "entity key must be str"))?;
            let v = kv[1]
                .as_str()
                .ok_or_else(|| PyError::named("TypeError", "entity value must be str"))?;
            data = data.replace(k, v);
        }
    }
    Ok(data)
}

fn escape_impl(interp: &mut Interpreter, data: &str, entities: Option<&Value>) -> Result<String> {
    let mut s = data
        .replace('&', "&amp;")
        .replace('>', "&gt;")
        .replace('<', "&lt;");
    if let Some(ent) = entities {
        s = replace_entities(interp, s, ent)?;
    }
    Ok(s)
}

fn unescape_impl(interp: &mut Interpreter, data: &str, entities: Option<&Value>) -> Result<String> {
    let mut s = data.replace("&lt;", "<").replace("&gt;", ">");
    if let Some(ent) = entities {
        s = replace_entities(interp, s, ent)?;
    }
    Ok(s.replace("&amp;", "&"))
}

fn quoteattr_impl(
    interp: &mut Interpreter,
    data: &str,
    entities: Option<&Value>,
) -> Result<String> {
    let mut s = data
        .replace('&', "&amp;")
        .replace('>', "&gt;")
        .replace('<', "&lt;")
        .replace('\n', "&#10;")
        .replace('\r', "&#13;")
        .replace('\t', "&#9;");
    if let Some(ent) = entities {
        s = replace_entities(interp, s, ent)?;
    }
    if s.contains('"') {
        if s.contains('\'') {
            Ok(format!("\"{}\"", s.replace('"', "&quot;")))
        } else {
            Ok(format!("'{s}'"))
        }
    } else {
        Ok(format!("\"{s}\""))
    }
}

fn write_gen_text(interp: &mut Interpreter, self_val: &Value, text: &str) -> Result<()> {
    let out = interp.get_attr(self_val, "_out")?;
    let encoding = interp
        .get_attr(self_val, "_encoding")
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "iso-8859-1".to_string());
    let is_bytes = interp
        .get_attr(self_val, "_out_is_bytes")
        .ok()
        .and_then(|v| interp.truthy_value(&v).ok())
        .unwrap_or(false);
    if is_bytes {
        let encoded = call_method(
            interp,
            &Value::string(text),
            "encode",
            &[Value::string(&encoding), Value::string("xmlcharrefreplace")],
        )?;
        call_method(interp, &out, "write", &[encoded])?;
    } else {
        call_method(interp, &out, "write", &[Value::string(text)])?;
    }
    Ok(())
}

fn finish_pending_start_element(interp: &mut Interpreter, self_val: &Value) -> Result<()> {
    let pending = interp.get_attr(self_val, "_pending_start_element")?;
    if interp.truthy_value(&pending)? {
        write_gen_text(interp, self_val, ">")?;
        interp.assign_attr(
            self_val.clone(),
            "_pending_start_element",
            Value::bool_(false),
        )?;
    }
    Ok(())
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let h_mod = interp.load_module("xml.sax.handler")?;
    let xr_mod = interp.load_module("xml.sax.xmlreader")?;
    let ch_cls = interp.get_attr(&h_mod, "ContentHandler")?;
    let xr_cls = interp.get_attr(&xr_mod, "XMLReader")?;
    let (gen_val, flt_val) = {
        let m = module.borrow();
        (
            m.get_attr_value("XMLGenerator"),
            m.get_attr_value("XMLFilterBase"),
        )
    };
    if let (Some(g), ValueKind::PyClass(bc)) = (gen_val.as_ref(), ch_cls.kind())
        && let ValueKind::PyClass(gc) = g.kind()
    {
        link_class_bases(gc, vec![Rc::clone(bc)]);
    }
    if let (Some(f), ValueKind::PyClass(bc)) = (flt_val.as_ref(), xr_cls.kind())
        && let ValueKind::PyClass(fc) = f.kind()
    {
        link_class_bases(fc, vec![Rc::clone(bc)]);
    }
    let mut m = module.borrow_mut();
    m.insert_attr("handler".to_string(), h_mod);
    m.insert_attr("xmlreader".to_string(), xr_mod);
    if let Ok(io_m) = interp.load_module("io") {
        m.insert_attr("io".to_string(), io_m);
    }
    if let Ok(os_m) = interp.load_module("os") {
        m.insert_attr("os".to_string(), os_m);
    }
    if let Ok(c_m) = interp.load_module("codecs") {
        m.insert_attr("codecs".to_string(), c_m);
    }
    if let Ok(u_m) = interp.load_module("urllib") {
        m.insert_attr("urllib".to_string(), u_m);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.sax"),
    }

    fn escape(
        data: PyStr<'_>,
        #[default(OptionalArg::Omitted)] entities: OptValueRef<'_>,
    ) -> Result<Value> {
        let out = escape_impl(_interp, &data.0, entities.as_value())?;
        Ok(Value::string(out))
    }

    fn unescape(
        data: PyStr<'_>,
        #[default(OptionalArg::Omitted)] entities: OptValueRef<'_>,
    ) -> Result<Value> {
        let out = unescape_impl(_interp, &data.0, entities.as_value())?;
        Ok(Value::string(out))
    }

    fn quoteattr(
        data: PyStr<'_>,
        #[default(OptionalArg::Omitted)] entities: OptValueRef<'_>,
    ) -> Result<Value> {
        let out = quoteattr_impl(_interp, &data.0, entities.as_value())?;
        Ok(Value::string(out))
    }

    fn prepare_input_source(
        source: PyValue,
        #[default(OptionalArg::Omitted)] base: OptValueRef<'_>,
    ) -> Result<Value> {
        let xr_mod = _interp.load_module("xml.sax.xmlreader")?;
        let is_cls = _interp.get_attr(&xr_mod, "InputSource")?;

        let is_input_src = call_pos(
            _interp,
            Value::builtin_function("isinstance"),
            &[source.0.clone(), is_cls.clone()],
        )?;
        let src_obj = if _interp.truthy_value(&is_input_src)? {
            source.0
        } else {
            // Check os.PathLike or str
            let os_mod = _interp.load_module("os")?;
            let fspath = _interp.get_attr(&os_mod, "fspath")?;
            let path_opt = if source.0.as_str().is_some() {
                Some(source.0.clone())
            } else {
                call_pos(_interp, fspath, std::slice::from_ref(&source.0)).ok()
            };
            if let Some(path_val) = path_opt {
                call_pos(_interp, is_cls, &[path_val])?
            } else {
                // File-like object
                let s = call_pos(_interp, is_cls, &[])?;
                if let Ok(name_val) = _interp.get_attr(&source.0, "name") {
                    call_method(_interp, &s, "setSystemId", &[name_val])?;
                }
                let io_mod = _interp.load_module("io")?;
                let text_io_base = _interp.get_attr(&io_mod, "TextIOBase")?;
                let is_text = call_pos(
                    _interp,
                    Value::builtin_function("isinstance"),
                    &[source.0.clone(), text_io_base],
                )
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
                if is_text {
                    call_method(_interp, &s, "setCharacterStream", &[source.0])?;
                } else {
                    call_method(_interp, &s, "setByteStream", &[source.0])?;
                }
                s
            }
        };

        let char_stream = call_method(_interp, &src_obj, "getCharacterStream", &[])?;
        let byte_stream = call_method(_interp, &src_obj, "getByteStream", &[])?;
        if char_stream.is_none() && byte_stream.is_none() {
            let sysid = call_method(_interp, &src_obj, "getSystemId", &[])?;
            if let Some(sysid_str) = sysid.as_str() {
                let base_str = base
                    .as_value()
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let full_path = if !base_str.is_empty() {
                    let os_path = _interp.load_module("os.path")?;
                    let dirname = _interp.get_attr(&os_path, "dirname")?;
                    let join = _interp.get_attr(&os_path, "join")?;
                    let basehead = call_pos(_interp, dirname, &[Value::string(base_str)])?;
                    let joined = call_pos(_interp, join, &[basehead, Value::string(sysid_str)])?;
                    joined.as_str().unwrap_or(sysid_str).to_string()
                } else {
                    sysid_str.to_string()
                };
                if std::path::Path::new(&full_path).is_file() {
                    call_method(
                        _interp,
                        &src_obj,
                        "setSystemId",
                        &[Value::string(&full_path)],
                    )?;
                    let f = call_pos(
                        _interp,
                        Value::builtin_function("open"),
                        &[Value::string(&full_path), Value::string("rb")],
                    )?;
                    call_method(_interp, &src_obj, "setByteStream", &[f])?;
                } else {
                    let f = call_pos(
                        _interp,
                        Value::builtin_function("open"),
                        &[Value::string(sysid_str), Value::string("rb")],
                    )?;
                    call_method(_interp, &src_obj, "setByteStream", &[f])?;
                }
            }
        }
        Ok(src_obj)
    }

    class XMLGenerator {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] out: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(false.into())] short_empty_elements: PyBool,
        ) -> Result<Value> {
            let h_mod = _interp.load_module("xml.sax.handler")?;
            let ch_cls = _interp.get_attr(&h_mod, "ContentHandler")?;
            let ch_init = _interp.get_attr(&ch_cls, "__init__")?;
            call_pos(_interp, ch_init, std::slice::from_ref(self_val.0))?;

            let enc = encoding
                .as_value()
                .and_then(|v| v.as_str())
                .unwrap_or("iso-8859-1");
            let out_val = match out.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let sys_mod = _interp.load_module("sys")?;
                    _interp.get_attr(&sys_mod, "stdout")?
                }
            };
            let io_mod = _interp.load_module("io")?;
            let text_io_base = _interp.get_attr(&io_mod, "TextIOBase")?;
            let is_text = call_pos(
                _interp,
                Value::builtin_function("isinstance"),
                &[out_val.clone(), text_io_base],
            )
            .ok()
            .and_then(|v| _interp.truthy_value(&v).ok())
            .unwrap_or(false);
            let is_bytes = !is_text
                && _interp
                    .get_attr(&out_val, "encoding")
                    .map(|v| v.is_none())
                    .unwrap_or(true);
            // If out is BytesIO or binary stream, write encoded bytes; if TextIOBase / StringIO / stdout, write str.
            let raw_io = _interp.get_attr(&io_mod, "RawIOBase")?;
            let buf_io = _interp.get_attr(&io_mod, "BufferedIOBase")?;
            let is_bin = call_pos(
                _interp,
                Value::builtin_function("isinstance"),
                &[out_val.clone(), Value::tuple(vec![raw_io, buf_io])],
            )
            .ok()
            .and_then(|v| _interp.truthy_value(&v).ok())
            .unwrap_or(false);
            let out_is_bytes = is_bin || (!is_text && is_bytes && {
                let tname = crate::interpreter::value_type_name_str(&out_val);
                tname.contains("Bytes")
            });

            _interp.assign_attr(self_val.0.clone(), "_out", out_val)?;
            _interp.assign_attr(self_val.0.clone(), "_out_is_bytes", Value::bool_(out_is_bytes))?;
            _interp.assign_attr(self_val.0.clone(), "_encoding", Value::string(enc))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_short_empty_elements",
                Value::bool_(short_empty_elements.0),
            )?;
            _interp.assign_attr(self_val.0.clone(), "_ns_contexts", Value::list(vec![Value::dict(PyDict::default())]))?;
            _interp.assign_attr(self_val.0.clone(), "_current_context", Value::dict(PyDict::default()))?;
            _interp.assign_attr(self_val.0.clone(), "_undeclared_ns_maps", Value::list(Vec::new()))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_pending_start_element",
                Value::bool_(false),
            )?;
            Ok(Value::none())
        }

        fn _qname(#[positional_only] self_val: PyValueRef<'_>, name: PyValue) -> Result<Value> {
            let parts = _interp.collect_iterable(&name.0)?;
            if parts.len() != 2 {
                return Err(PyError::named("ValueError", "name must be a (uri, localname) tuple"));
            }
            let uri = &parts[0];
            let local = parts[1]
                .as_str()
                .ok_or_else(|| PyError::named("TypeError", "localname must be str"))?;
            if !uri.is_none() {
                if uri.as_str() == Some("http://www.w3.org/XML/1998/namespace") {
                    return Ok(Value::string(format!("xml:{local}")));
                }
                let ctx = _interp.get_attr(self_val.0, "_current_context")?;
                let prefix = call_method(_interp, &ctx, "get", &[uri.clone(), Value::none()])?;
                if let Some(pfx) = prefix.as_str()
                    && !pfx.is_empty()
                {
                    return Ok(Value::string(format!("{pfx}:{local}")));
                }
            }
            Ok(Value::string(local))
        }

        fn _finish_pending_start_element(
            #[positional_only] self_val: PyValueRef<'_>,
        ) -> Result<Value> {
            finish_pending_start_element(_interp, self_val.0)?;
            Ok(Value::none())
        }

        #[py_name = "startDocument"]
        fn start_document(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let enc = _interp.get_attr(self_val.0, "_encoding")?;
            let enc_s = enc.as_str().unwrap_or("iso-8859-1");
            write_gen_text(
                _interp,
                self_val.0,
                &format!("<?xml version=\"1.0\" encoding=\"{enc_s}\"?>\n"),
            )?;
            Ok(Value::none())
        }

        #[py_name = "endDocument"]
        fn end_document(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "startPrefixMapping"]
        fn start_prefix_mapping(
            #[positional_only] self_val: PyValueRef<'_>,
            prefix: PyValue,
            uri: PyValue,
        ) -> Result<Value> {
            let ns_contexts = _interp.get_attr(self_val.0, "_ns_contexts")?;
            let cur_ctx = _interp.get_attr(self_val.0, "_current_context")?;
            let copy_ctx = call_method(_interp, &cur_ctx, "copy", &[])?;
            call_method(_interp, &ns_contexts, "append", &[copy_ctx])?;
            call_method(_interp, &cur_ctx, "__setitem__", &[uri.0.clone(), prefix.0.clone()])?;
            let undecl = _interp.get_attr(self_val.0, "_undeclared_ns_maps")?;
            call_method(
                _interp,
                &undecl,
                "append",
                &[Value::tuple(vec![prefix.0, uri.0])],
            )?;
            Ok(Value::none())
        }

        #[py_name = "endPrefixMapping"]
        fn end_prefix_mapping(
            #[positional_only] self_val: PyValueRef<'_>,
            _prefix: PyValue,
        ) -> Result<Value> {
            let ns_contexts = _interp.get_attr(self_val.0, "_ns_contexts")?;
            let prev = call_method(_interp, &ns_contexts, "pop", &[])?;
            _interp.assign_attr(self_val.0.clone(), "_current_context", prev)?;
            Ok(Value::none())
        }

        #[py_name = "startElement"]
        fn start_element(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            attrs: PyValue,
        ) -> Result<Value> {
            finish_pending_start_element(_interp, self_val.0)?;
            let mut out = format!("<{}", name.0);
            let items = call_method(_interp, &attrs.0, "items", &[])?;
            for pair in _interp.collect_iterable(&items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    let k = kv[0].as_str().unwrap_or("");
                    let v = kv[1].as_str().unwrap_or("");
                    let qv = quoteattr_impl(_interp, v, None)?;
                    out.push_str(&format!(" {k}={qv}"));
                }
            }
            let short = _interp.get_attr(self_val.0, "_short_empty_elements")?;
            if _interp.truthy_value(&short)? {
                _interp.assign_attr(self_val.0.clone(), "_pending_start_element", Value::bool_(true))?;
            } else {
                out.push('>');
            }
            write_gen_text(_interp, self_val.0, &out)?;
            Ok(Value::none())
        }

        #[py_name = "endElement"]
        fn end_element(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let pending = _interp.get_attr(self_val.0, "_pending_start_element")?;
            if _interp.truthy_value(&pending)? {
                write_gen_text(_interp, self_val.0, "/>")?;
                _interp.assign_attr(self_val.0.clone(), "_pending_start_element", Value::bool_(false))?;
            } else {
                write_gen_text(_interp, self_val.0, &format!("</{}>", name.0))?;
            }
            Ok(Value::none())
        }

        #[py_name = "startElementNS"]
        fn start_element_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            _qname: PyValue,
            attrs: PyValue,
        ) -> Result<Value> {
            finish_pending_start_element(_interp, self_val.0)?;
            let qn = call_method(_interp, self_val.0, "_qname", &[name.0])?;
            let qn_s = qn.as_str().unwrap_or("");
            let mut out = format!("<{qn_s}");
            let undecl = _interp.get_attr(self_val.0, "_undeclared_ns_maps")?;
            let pairs = _interp.collect_iterable(&undecl)?;
            for pair in pairs {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    let uri_s = kv[1].as_str().unwrap_or("");
                    if let Some(pfx) = kv[0].as_str()
                        && !pfx.is_empty()
                    {
                        out.push_str(&format!(" xmlns:{pfx}=\"{uri_s}\""));
                    } else {
                        out.push_str(&format!(" xmlns=\"{uri_s}\""));
                    }
                }
            }
            _interp.assign_attr(self_val.0.clone(), "_undeclared_ns_maps", Value::list(Vec::new()))?;
            let items = call_method(_interp, &attrs.0, "items", &[])?;
            for pair in _interp.collect_iterable(&items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    let aq = call_method(_interp, self_val.0, "_qname", &[kv[0].clone()])?;
                    let aqs = aq.as_str().unwrap_or("");
                    let vs = kv[1].as_str().unwrap_or("");
                    let qv = quoteattr_impl(_interp, vs, None)?;
                    out.push_str(&format!(" {aqs}={qv}"));
                }
            }
            let short = _interp.get_attr(self_val.0, "_short_empty_elements")?;
            if _interp.truthy_value(&short)? {
                _interp.assign_attr(self_val.0.clone(), "_pending_start_element", Value::bool_(true))?;
            } else {
                out.push('>');
            }
            write_gen_text(_interp, self_val.0, &out)?;
            Ok(Value::none())
        }

        #[py_name = "endElementNS"]
        fn end_element_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            _qname: PyValue,
        ) -> Result<Value> {
            let pending = _interp.get_attr(self_val.0, "_pending_start_element")?;
            if _interp.truthy_value(&pending)? {
                write_gen_text(_interp, self_val.0, "/>")?;
                _interp.assign_attr(self_val.0.clone(), "_pending_start_element", Value::bool_(false))?;
            } else {
                let qn = call_method(_interp, self_val.0, "_qname", &[name.0])?;
                let qn_s = qn.as_str().unwrap_or("");
                write_gen_text(_interp, self_val.0, &format!("</{qn_s}>"))?;
            }
            Ok(Value::none())
        }

        fn characters(
            #[positional_only] self_val: PyValueRef<'_>,
            content: PyValue,
        ) -> Result<Value> {
            let s = match content.0.kind() {
                ValueKind::Str(s) => s.to_string(),
                ValueKind::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                _ => _interp.render_value_as_str(&content.0)?,
            };
            if !s.is_empty() {
                finish_pending_start_element(_interp, self_val.0)?;
                let esc = escape_impl(_interp, &s, None)?;
                write_gen_text(_interp, self_val.0, &esc)?;
            }
            Ok(Value::none())
        }

        #[py_name = "ignorableWhitespace"]
        fn ignorable_whitespace(
            #[positional_only] self_val: PyValueRef<'_>,
            content: PyValue,
        ) -> Result<Value> {
            let s = match content.0.kind() {
                ValueKind::Str(s) => s.to_string(),
                ValueKind::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                _ => _interp.render_value_as_str(&content.0)?,
            };
            if !s.is_empty() {
                finish_pending_start_element(_interp, self_val.0)?;
                write_gen_text(_interp, self_val.0, &s)?;
            }
            Ok(Value::none())
        }

        #[py_name = "processingInstruction"]
        fn processing_instruction(
            #[positional_only] self_val: PyValueRef<'_>,
            target: PyStr<'_>,
            data: PyStr<'_>,
        ) -> Result<Value> {
            finish_pending_start_element(_interp, self_val.0)?;
            write_gen_text(_interp, self_val.0, &format!("<?{} {}?>", target.0, data.0))?;
            Ok(Value::none())
        }

        #[py_name = "skippedEntity"]
        fn skipped_entity(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            finish_pending_start_element(_interp, self_val.0)?;
            write_gen_text(_interp, self_val.0, &format!("&{};", name.0))?;
            Ok(Value::none())
        }
    }

    class XMLFilterBase {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] parent: OptValueRef<'_>,
        ) -> Result<Value> {
            let xr_mod = _interp.load_module("xml.sax.xmlreader")?;
            let xr_cls = _interp.get_attr(&xr_mod, "XMLReader")?;
            let xr_init = _interp.get_attr(&xr_cls, "__init__")?;
            call_pos(_interp, xr_init, std::slice::from_ref(self_val.0))?;
            let p = parent.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "_parent", p)?;
            Ok(Value::none())
        }

        // ErrorHandler forwarding
        fn error(#[positional_only] self_val: PyValueRef<'_>, exception: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_err_handler")?;
            call_method(_interp, &h, "error", &[exception.0])
        }
        #[py_name = "fatalError"]
        fn fatal_error(#[positional_only] self_val: PyValueRef<'_>, exception: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_err_handler")?;
            call_method(_interp, &h, "fatalError", &[exception.0])
        }
        fn warning(#[positional_only] self_val: PyValueRef<'_>, exception: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_err_handler")?;
            call_method(_interp, &h, "warning", &[exception.0])
        }

        // ContentHandler forwarding
        #[py_name = "setDocumentLocator"]
        fn set_document_locator(#[positional_only] self_val: PyValueRef<'_>, locator: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "setDocumentLocator", &[locator.0])
        }
        #[py_name = "startDocument"]
        fn start_document(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "startDocument", &[])
        }
        #[py_name = "endDocument"]
        fn end_document(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "endDocument", &[])
        }
        #[py_name = "startPrefixMapping"]
        fn start_prefix_mapping(#[positional_only] self_val: PyValueRef<'_>, prefix: PyValue, uri: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "startPrefixMapping", &[prefix.0, uri.0])
        }
        #[py_name = "endPrefixMapping"]
        fn end_prefix_mapping(#[positional_only] self_val: PyValueRef<'_>, prefix: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "endPrefixMapping", &[prefix.0])
        }
        #[py_name = "startElement"]
        fn start_element(#[positional_only] self_val: PyValueRef<'_>, name: PyValue, attrs: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "startElement", &[name.0, attrs.0])
        }
        #[py_name = "endElement"]
        fn end_element(#[positional_only] self_val: PyValueRef<'_>, name: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "endElement", &[name.0])
        }
        #[py_name = "startElementNS"]
        fn start_element_ns(#[positional_only] self_val: PyValueRef<'_>, name: PyValue, qname: PyValue, attrs: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "startElementNS", &[name.0, qname.0, attrs.0])
        }
        #[py_name = "endElementNS"]
        fn end_element_ns(#[positional_only] self_val: PyValueRef<'_>, name: PyValue, qname: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "endElementNS", &[name.0, qname.0])
        }
        fn characters(#[positional_only] self_val: PyValueRef<'_>, content: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "characters", &[content.0])
        }
        #[py_name = "ignorableWhitespace"]
        fn ignorable_whitespace(#[positional_only] self_val: PyValueRef<'_>, chars: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "ignorableWhitespace", &[chars.0])
        }
        #[py_name = "processingInstruction"]
        fn processing_instruction(#[positional_only] self_val: PyValueRef<'_>, target: PyValue, data: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "processingInstruction", &[target.0, data.0])
        }
        #[py_name = "skippedEntity"]
        fn skipped_entity(#[positional_only] self_val: PyValueRef<'_>, name: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &h, "skippedEntity", &[name.0])
        }

        // DTDHandler & EntityResolver forwarding
        #[py_name = "notationDecl"]
        fn notation_decl(#[positional_only] self_val: PyValueRef<'_>, name: PyValue, public_id: PyValue, system_id: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_dtd_handler")?;
            call_method(_interp, &h, "notationDecl", &[name.0, public_id.0, system_id.0])
        }
        #[py_name = "unparsedEntityDecl"]
        fn unparsed_entity_decl(#[positional_only] self_val: PyValueRef<'_>, name: PyValue, public_id: PyValue, system_id: PyValue, ndata: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_dtd_handler")?;
            call_method(_interp, &h, "unparsedEntityDecl", &[name.0, public_id.0, system_id.0, ndata.0])
        }
        #[py_name = "resolveEntity"]
        fn resolve_entity(#[positional_only] self_val: PyValueRef<'_>, public_id: PyValue, system_id: PyValue) -> Result<Value> {
            let h = _interp.get_attr(self_val.0, "_ent_handler")?;
            call_method(_interp, &h, "resolveEntity", &[public_id.0, system_id.0])
        }

        // XMLReader parent delegation
        fn parse(#[positional_only] self_val: PyValueRef<'_>, source: PyValue) -> Result<Value> {
            let parent = _interp.get_attr(self_val.0, "_parent")?;
            call_method(_interp, &parent, "setContentHandler", std::slice::from_ref(self_val.0))?;
            call_method(_interp, &parent, "setErrorHandler", std::slice::from_ref(self_val.0))?;
            call_method(_interp, &parent, "setEntityResolver", std::slice::from_ref(self_val.0))?;
            call_method(_interp, &parent, "setDTDHandler", std::slice::from_ref(self_val.0))?;
            call_method(_interp, &parent, "parse", &[source.0])
        }
        #[py_name = "setLocale"]
        fn set_locale(#[positional_only] self_val: PyValueRef<'_>, locale: PyValue) -> Result<Value> {
            let parent = _interp.get_attr(self_val.0, "_parent")?;
            call_method(_interp, &parent, "setLocale", &[locale.0])
        }
        #[py_name = "getFeature"]
        fn get_feature(#[positional_only] self_val: PyValueRef<'_>, name: PyValue) -> Result<Value> {
            let parent = _interp.get_attr(self_val.0, "_parent")?;
            call_method(_interp, &parent, "getFeature", &[name.0])
        }
        #[py_name = "setFeature"]
        fn set_feature(#[positional_only] self_val: PyValueRef<'_>, name: PyValue, state: PyValue) -> Result<Value> {
            let parent = _interp.get_attr(self_val.0, "_parent")?;
            call_method(_interp, &parent, "setFeature", &[name.0, state.0])
        }
        #[py_name = "getProperty"]
        fn get_property(#[positional_only] self_val: PyValueRef<'_>, name: PyValue) -> Result<Value> {
            let parent = _interp.get_attr(self_val.0, "_parent")?;
            call_method(_interp, &parent, "getProperty", &[name.0])
        }
        #[py_name = "setProperty"]
        fn set_property(#[positional_only] self_val: PyValueRef<'_>, name: PyValue, value: PyValue) -> Result<Value> {
            let parent = _interp.get_attr(self_val.0, "_parent")?;
            call_method(_interp, &parent, "setProperty", &[name.0, value.0])
        }
        #[py_name = "getParent"]
        fn get_parent(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_parent")
        }
        #[py_name = "setParent"]
        fn set_parent(#[positional_only] self_val: PyValueRef<'_>, parent: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_parent", parent.0)?;
            Ok(Value::none())
        }
    }
}
