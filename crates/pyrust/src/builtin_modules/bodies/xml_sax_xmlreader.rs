// `xml.sax.xmlreader` module — 100% native-Rust Python 3.12 SAX reader,
// incremental parser, locator, input source, and attribute container classes
// (issue #3260).
//
// Reference: <https://docs.python.org/3/library/xml.sax.reader.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

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

fn raise_sax_exc(interp: &mut Interpreter, exc_name: &'static str, msg: String) -> PyError {
    if let Ok(sax_mod) = interp.load_module("xml.sax")
        && let Ok(cls) = interp.get_attr(&sax_mod, exc_name)
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(&msg)])
    {
        return PyError::raised(inst);
    }
    PyError::named(exc_name, msg)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let sax_mod = interp.load_module("xml.sax")?;
    let handler_mod = interp.load_module("xml.sax.handler")?;
    let mut m = module.borrow_mut();
    if let Ok(e) = interp.get_attr(&sax_mod, "SAXNotRecognizedException") {
        m.insert_attr("SAXNotRecognizedException".to_string(), e);
    }
    if let Ok(e) = interp.get_attr(&sax_mod, "SAXNotSupportedException") {
        m.insert_attr("SAXNotSupportedException".to_string(), e);
    }
    m.insert_attr("handler".to_string(), handler_mod);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.sax"),
    }

    class XMLReader {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let h_mod = _interp.load_module("xml.sax.handler")?;
            let ch_cls = _interp.get_attr(&h_mod, "ContentHandler")?;
            let ch = call_pos(_interp, ch_cls, &[])?;
            let dh_cls = _interp.get_attr(&h_mod, "DTDHandler")?;
            let dh = call_pos(_interp, dh_cls, &[])?;
            let eh_cls = _interp.get_attr(&h_mod, "ErrorHandler")?;
            let eh = call_pos(_interp, eh_cls, &[])?;
            let er_cls = _interp.get_attr(&h_mod, "EntityResolver")?;
            let er = call_pos(_interp, er_cls, &[])?;
            _interp.assign_attr(self_val.0.clone(), "_cont_handler", ch)?;
            _interp.assign_attr(self_val.0.clone(), "_dtd_handler", dh)?;
            _interp.assign_attr(self_val.0.clone(), "_err_handler", eh)?;
            _interp.assign_attr(self_val.0.clone(), "_ent_handler", er)?;
            Ok(Value::none())
        }

        fn parse(#[positional_only] _self: PyValueRef<'_>, _source: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "This method must be implemented!",
            ))
        }

        #[py_name = "getContentHandler"]
        fn get_content_handler(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_cont_handler")
        }

        #[py_name = "setContentHandler"]
        fn set_content_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            handler: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_cont_handler", handler.0)?;
            Ok(Value::none())
        }

        #[py_name = "getDTDHandler"]
        fn get_dtd_handler(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_dtd_handler")
        }

        #[py_name = "setDTDHandler"]
        fn set_dtd_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            handler: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_dtd_handler", handler.0)?;
            Ok(Value::none())
        }

        #[py_name = "getEntityResolver"]
        fn get_entity_resolver(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_ent_handler")
        }

        #[py_name = "setEntityResolver"]
        fn set_entity_resolver(
            #[positional_only] self_val: PyValueRef<'_>,
            resolver: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_ent_handler", resolver.0)?;
            Ok(Value::none())
        }

        #[py_name = "getErrorHandler"]
        fn get_error_handler(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_err_handler")
        }

        #[py_name = "setErrorHandler"]
        fn set_error_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            handler: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_err_handler", handler.0)?;
            Ok(Value::none())
        }

        #[py_name = "setLocale"]
        fn set_locale(#[positional_only] _self: PyValueRef<'_>, _locale: PyValue) -> Result<Value> {
            Err(raise_sax_exc(
                _interp,
                "SAXNotSupportedException",
                "Locale support not implemented".to_string(),
            ))
        }

        #[py_name = "getFeature"]
        fn get_feature(#[positional_only] _self: PyValueRef<'_>, name: PyStr<'_>) -> Result<Value> {
            Err(raise_sax_exc(
                _interp,
                "SAXNotRecognizedException",
                format!("Feature '{}' not recognized", name.0),
            ))
        }

        #[py_name = "setFeature"]
        fn set_feature(
            #[positional_only] _self: PyValueRef<'_>,
            name: PyStr<'_>,
            _state: PyValue,
        ) -> Result<Value> {
            Err(raise_sax_exc(
                _interp,
                "SAXNotRecognizedException",
                format!("Feature '{}' not recognized", name.0),
            ))
        }

        #[py_name = "getProperty"]
        fn get_property(
            #[positional_only] _self: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            Err(raise_sax_exc(
                _interp,
                "SAXNotRecognizedException",
                format!("Property '{}' not recognized", name.0),
            ))
        }

        #[py_name = "setProperty"]
        fn set_property(
            #[positional_only] _self: PyValueRef<'_>,
            name: PyStr<'_>,
            _value: PyValue,
        ) -> Result<Value> {
            Err(raise_sax_exc(
                _interp,
                "SAXNotRecognizedException",
                format!("Property '{}' not recognized", name.0),
            ))
        }
    }

    class IncrementalParser("XMLReader") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] bufsize: OptValueRef<'_>,
        ) -> Result<Value> {
            let xr_mod = _interp.load_module("xml.sax.xmlreader")?;
            let xr_cls = _interp.get_attr(&xr_mod, "XMLReader")?;
            let xr_init = _interp.get_attr(&xr_cls, "__init__")?;
            call_pos(_interp, xr_init, std::slice::from_ref(self_val.0))?;
            let bs = match bufsize.as_value() {
                Some(v) => v.as_int().unwrap_or(65536),
                None => 65536,
            };
            _interp.assign_attr(self_val.0.clone(), "_bufsize", Value::int(bs))?;
            Ok(Value::none())
        }

        fn parse(#[positional_only] self_val: PyValueRef<'_>, source: PyValue) -> Result<Value> {
            let su_mod = _interp.load_module("xml.sax.saxutils")?;
            let prep = _interp.get_attr(&su_mod, "prepare_input_source")?;
            let prepared = call_pos(_interp, prep, &[source.0])?;
            call_method(
                _interp,
                self_val.0,
                "prepareParser",
                std::slice::from_ref(&prepared),
            )?;
            let mut file = call_method(_interp, &prepared, "getCharacterStream", &[])?;
            if file.is_none() {
                file = call_method(_interp, &prepared, "getByteStream", &[])?;
            }
            let bufsize = _interp.get_attr(self_val.0, "_bufsize")?;
            let read_fn = _interp.get_attr(&file, "read")?;
            loop {
                let buf = call_pos(_interp, read_fn.clone(), std::slice::from_ref(&bufsize))?;
                let empty = match buf.kind() {
                    ValueKind::Bytes(b) => b.is_empty(),
                    ValueKind::Str(s) => s.is_empty(),
                    _ => !_interp.truthy_value(&buf)?,
                };
                if empty {
                    break;
                }
                call_method(_interp, self_val.0, "feed", &[buf])?;
            }
            call_method(_interp, self_val.0, "close", &[])?;
            Ok(Value::none())
        }

        fn feed(#[positional_only] _self: PyValueRef<'_>, _data: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "This method must be implemented!",
            ))
        }

        #[py_name = "prepareParser"]
        fn prepare_parser(
            #[positional_only] _self: PyValueRef<'_>,
            _source: PyValue,
        ) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "prepareParser must be overridden!",
            ))
        }

        fn close(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "This method must be implemented!",
            ))
        }

        fn reset(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "This method must be implemented!",
            ))
        }
    }

    class Locator {
        #[py_name = "getColumnNumber"]
        fn get_column_number(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::int(-1))
        }

        #[py_name = "getLineNumber"]
        fn get_line_number(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::int(-1))
        }

        #[py_name = "getPublicId"]
        fn get_public_id(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "getSystemId"]
        fn get_system_id(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }

    class InputSource {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] system_id: OptValueRef<'_>,
        ) -> Result<Value> {
            let sys_id = system_id.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "_InputSource__system_id", sys_id)?;
            _interp.assign_attr(self_val.0.clone(), "_InputSource__public_id", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_InputSource__encoding", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_InputSource__bytefile", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_InputSource__charfile", Value::none())?;
            Ok(Value::none())
        }

        #[py_name = "setPublicId"]
        fn set_public_id(
            #[positional_only] self_val: PyValueRef<'_>,
            public_id: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_InputSource__public_id", public_id.0)?;
            Ok(Value::none())
        }

        #[py_name = "getPublicId"]
        fn get_public_id(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_InputSource__public_id")
        }

        #[py_name = "setSystemId"]
        fn set_system_id(
            #[positional_only] self_val: PyValueRef<'_>,
            system_id: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_InputSource__system_id", system_id.0)?;
            Ok(Value::none())
        }

        #[py_name = "getSystemId"]
        fn get_system_id(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_InputSource__system_id")
        }

        #[py_name = "setEncoding"]
        fn set_encoding(
            #[positional_only] self_val: PyValueRef<'_>,
            encoding: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_InputSource__encoding", encoding.0)?;
            Ok(Value::none())
        }

        #[py_name = "getEncoding"]
        fn get_encoding(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_InputSource__encoding")
        }

        #[py_name = "setByteStream"]
        fn set_byte_stream(
            #[positional_only] self_val: PyValueRef<'_>,
            bytefile: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_InputSource__bytefile", bytefile.0)?;
            Ok(Value::none())
        }

        #[py_name = "getByteStream"]
        fn get_byte_stream(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_InputSource__bytefile")
        }

        #[py_name = "setCharacterStream"]
        fn set_character_stream(
            #[positional_only] self_val: PyValueRef<'_>,
            charfile: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_InputSource__charfile", charfile.0)?;
            Ok(Value::none())
        }

        #[py_name = "getCharacterStream"]
        fn get_character_stream(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_InputSource__charfile")
        }
    }

    class AttributesImpl {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, attrs: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_attrs", attrs.0)?;
            Ok(Value::none())
        }

        #[py_name = "getLength"]
        fn get_length(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            call_pos(_interp, Value::builtin_function("len"), &[attrs])
        }

        #[py_name = "getType"]
        fn get_type(#[positional_only] _self: PyValueRef<'_>, _name: PyValue) -> Result<Value> {
            Ok(Value::string("CDATA"))
        }

        #[py_name = "getValue"]
        fn get_value(#[positional_only] self_val: PyValueRef<'_>, name: PyValue) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            call_method(_interp, &attrs, "__getitem__", &[name.0])
        }

        #[py_name = "getValueByQName"]
        fn get_value_by_qname(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            call_method(_interp, &attrs, "__getitem__", &[name.0])
        }

        #[py_name = "getNameByQName"]
        fn get_name_by_qname(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let has = call_method(_interp, &attrs, "__contains__", std::slice::from_ref(&name.0))?;
            if !_interp.truthy_value(&has)? {
                return Err(PyError::named("KeyError", _interp.render_value_as_str(&name.0)?));
            }
            Ok(name.0)
        }

        #[py_name = "getQNameByName"]
        fn get_qname_by_name(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let has = call_method(_interp, &attrs, "__contains__", std::slice::from_ref(&name.0))?;
            if !_interp.truthy_value(&has)? {
                return Err(PyError::named("KeyError", _interp.render_value_as_str(&name.0)?));
            }
            Ok(name.0)
        }

        #[py_name = "getNames"]
        fn get_names(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let keys = call_method(_interp, &attrs, "keys", &[])?;
            call_pos(_interp, Value::builtin_function("list"), &[keys])
        }

        #[py_name = "getQNames"]
        fn get_qnames(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let keys = call_method(_interp, &attrs, "keys", &[])?;
            call_pos(_interp, Value::builtin_function("list"), &[keys])
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            call_pos(_interp, Value::builtin_function("len"), &[attrs])
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            call_method(_interp, &attrs, "__getitem__", &[name.0])
        }

        fn keys(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let k = call_method(_interp, &attrs, "keys", &[])?;
            call_pos(_interp, Value::builtin_function("list"), &[k])
        }

        fn __contains__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            call_method(_interp, &attrs, "__contains__", &[name.0])
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[default(OptionalArg::Omitted)] alternative: OptValueRef<'_>,
        ) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let alt = alternative.as_value().cloned().unwrap_or_else(Value::none);
            call_method(_interp, &attrs, "get", &[name.0, alt])
        }

        fn copy(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let copied = call_method(_interp, &attrs, "copy", &[])?;
            let cls = _interp.get_attr(self_val.0, "__class__")?;
            call_pos(_interp, cls, &[copied])
        }

        fn items(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let it = call_method(_interp, &attrs, "items", &[])?;
            call_pos(_interp, Value::builtin_function("list"), &[it])
        }

        fn values(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let vs = call_method(_interp, &attrs, "values", &[])?;
            call_pos(_interp, Value::builtin_function("list"), &[vs])
        }
    }

    class AttributesNSImpl("AttributesImpl") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            attrs: PyValue,
            qnames: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_attrs", attrs.0)?;
            _interp.assign_attr(self_val.0.clone(), "_qnames", qnames.0)?;
            Ok(Value::none())
        }

        #[py_name = "getValueByQName"]
        fn get_value_by_qname(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let qnames = _interp.get_attr(self_val.0, "_qnames")?;
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let items = call_method(_interp, &qnames, "items", &[])?;
            for pair in _interp.collect_iterable(&items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    let eq = call_method(_interp, &kv[1], "__eq__", std::slice::from_ref(&name.0))?;
                    if _interp.truthy_value(&eq)? {
                        return call_method(_interp, &attrs, "__getitem__", &[kv[0].clone()]);
                    }
                }
            }
            Err(PyError::named("KeyError", _interp.render_value_as_str(&name.0)?))
        }

        #[py_name = "getNameByQName"]
        fn get_name_by_qname(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let qnames = _interp.get_attr(self_val.0, "_qnames")?;
            let items = call_method(_interp, &qnames, "items", &[])?;
            for pair in _interp.collect_iterable(&items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    let eq = call_method(_interp, &kv[1], "__eq__", std::slice::from_ref(&name.0))?;
                    if _interp.truthy_value(&eq)? {
                        return Ok(kv[0].clone());
                    }
                }
            }
            Err(PyError::named("KeyError", _interp.render_value_as_str(&name.0)?))
        }

        #[py_name = "getQNameByName"]
        fn get_qname_by_name(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let qnames = _interp.get_attr(self_val.0, "_qnames")?;
            call_method(_interp, &qnames, "__getitem__", &[name.0])
        }

        #[py_name = "getQNames"]
        fn get_qnames(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let qnames = _interp.get_attr(self_val.0, "_qnames")?;
            let vals = call_method(_interp, &qnames, "values", &[])?;
            call_pos(_interp, Value::builtin_function("list"), &[vals])
        }

        fn copy(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            let qnames = _interp.get_attr(self_val.0, "_qnames")?;
            let a_copy = call_method(_interp, &attrs, "copy", &[])?;
            let q_copy = call_method(_interp, &qnames, "copy", &[])?;
            let cls = _interp.get_attr(self_val.0, "__class__")?;
            call_pos(_interp, cls, &[a_copy, q_copy])
        }
    }
}
