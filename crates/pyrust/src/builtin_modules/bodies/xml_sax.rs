// `xml.sax` package module — 100% native-Rust Python 3.12 SAX2 package:
// `make_parser`, `parse`, `parseString`, `SAXException` hierarchy,
// `ContentHandler`, `ErrorHandler`, `InputSource`, `default_parser_list`,
// and submodules `handler`, `saxutils`, `xmlreader`, `expatreader` (issue #3260).
//
// Reference: <https://docs.python.org/3/library/xml.sax.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
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

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let h_mod = interp.load_module("xml.sax.handler")?;
    let xr_mod = interp.load_module("xml.sax.xmlreader")?;
    let mut m = module.borrow_mut();
    if let Ok(ch) = interp.get_attr(&h_mod, "ContentHandler") {
        m.insert_attr("ContentHandler".to_string(), ch);
    }
    if let Ok(eh) = interp.get_attr(&h_mod, "ErrorHandler") {
        m.insert_attr("ErrorHandler".to_string(), eh);
    }
    if let Ok(is_cls) = interp.get_attr(&xr_mod, "InputSource") {
        m.insert_attr("InputSource".to_string(), is_cls);
    }
    Ok(())
}

fn make_parser_impl(interp: &mut Interpreter, parser_list: Option<&Value>) -> Result<Value> {
    let mut names: Vec<String> = Vec::new();
    if let Some(pl) = parser_list {
        for item in interp.collect_iterable(pl)? {
            if let Some(s) = item.as_str() {
                names.push(s.to_string());
            }
        }
    }
    let sax_mod = interp.load_module("xml.sax")?;
    let def_list = interp.get_attr(&sax_mod, "default_parser_list")?;
    for item in interp.collect_iterable(&def_list)? {
        if let Some(s) = item.as_str() {
            names.push(s.to_string());
        }
    }
    for name in names {
        if let Ok(mod_val) = interp.load_module(&name)
            && let Ok(create_fn) = interp.get_attr(&mod_val, "create_parser")
            && let Ok(parser) = call_pos(interp, create_fn, &[])
        {
            return Ok(parser);
        }
    }
    let cls = interp.get_attr(&sax_mod, "SAXReaderNotAvailable")?;
    let exc = call_pos(
        interp,
        cls,
        &[Value::string("No parsers found"), Value::none()],
    )?;
    Err(PyError::raised(exc))
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.sax"),
        "default_parser_list" => Value::list(vec![Value::string("xml.sax.expatreader")]),
        "handler" => super::xml_sax_handler::module(),
        "saxutils" => super::xml_sax_saxutils::module(),
        "xmlreader" => super::xml_sax_xmlreader::module(),
        "expatreader" => super::xml_sax_expatreader::module(),
    }

    class SAXException("Exception") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
            #[default(OptionalArg::Omitted)] exception: OptValueRef<'_>,
        ) -> Result<Value> {
            let exc = exception.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "_msg", msg.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "_exception", exc)?;
            _interp.assign_attr(self_val.0.clone(), "args", Value::tuple(vec![msg.0]))?;
            Ok(Value::none())
        }

        #[py_name = "getMessage"]
        fn get_message(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_msg")
        }

        #[py_name = "getException"]
        fn get_exception(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_exception")
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let msg = _interp.get_attr(self_val.0, "_msg")?;
            let s = _interp.render_value_as_str(&msg)?;
            Ok(Value::string(s))
        }

        fn __getitem__(#[positional_only] _self: PyValueRef<'_>, _ix: PyValue) -> Result<Value> {
            Err(PyError::named("AttributeError", "__getitem__"))
        }
    }

    class SAXParseException("SAXException") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
            exception: PyValue,
            locator: PyValue,
        ) -> Result<Value> {
            let sax_mod = _interp.load_module("xml.sax")?;
            let base_cls = _interp.get_attr(&sax_mod, "SAXException")?;
            let base_init = _interp.get_attr(&base_cls, "__init__")?;
            call_pos(_interp, base_init, &[self_val.0.clone(), msg.0, exception.0])?;
            _interp.assign_attr(self_val.0.clone(), "_locator", locator.0.clone())?;
            let sysid = call_method(_interp, &locator.0, "getSystemId", &[])
                .unwrap_or_else(|_| Value::none());
            let colnum = call_method(_interp, &locator.0, "getColumnNumber", &[])
                .unwrap_or_else(|_| Value::none());
            let linenum = call_method(_interp, &locator.0, "getLineNumber", &[])
                .unwrap_or_else(|_| Value::none());
            _interp.assign_attr(self_val.0.clone(), "_systemId", sysid)?;
            _interp.assign_attr(self_val.0.clone(), "_colnum", colnum)?;
            _interp.assign_attr(self_val.0.clone(), "_linenum", linenum)?;
            Ok(Value::none())
        }

        #[py_name = "getColumnNumber"]
        fn get_column_number(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_colnum")
        }

        #[py_name = "getLineNumber"]
        fn get_line_number(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_linenum")
        }

        #[py_name = "getPublicId"]
        fn get_public_id(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let loc = _interp.get_attr(self_val.0, "_locator")?;
            call_method(_interp, &loc, "getPublicId", &[])
        }

        #[py_name = "getSystemId"]
        fn get_system_id(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_systemId")
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let sysid = call_method(_interp, self_val.0, "getSystemId", &[])?;
            let sysid_s = if sysid.is_none() {
                "<unknown>".to_string()
            } else {
                _interp.render_value_as_str(&sysid)?
            };
            let line = call_method(_interp, self_val.0, "getLineNumber", &[])?;
            let line_s = if line.is_none() {
                "?".to_string()
            } else {
                _interp.render_value_as_str(&line)?
            };
            let col = call_method(_interp, self_val.0, "getColumnNumber", &[])?;
            let col_s = if col.is_none() {
                "?".to_string()
            } else {
                _interp.render_value_as_str(&col)?
            };
            let msg = _interp.get_attr(self_val.0, "_msg")?;
            let msg_s = _interp.render_value_as_str(&msg)?;
            Ok(Value::string(format!("{sysid_s}:{line_s}:{col_s}: {msg_s}")))
        }
    }

    class SAXNotRecognizedException("SAXException") {}
    class SAXNotSupportedException("SAXException") {}
    class SAXReaderNotAvailable("SAXNotSupportedException") {}

    fn make_parser(
        #[default(OptionalArg::Omitted)] parser_list: OptValueRef<'_>,
    ) -> Result<Value> {
        make_parser_impl(_interp, parser_list.as_value())
    }

    fn parse(
        source: PyValue,
        handler: PyValue,
        #[default(OptionalArg::Omitted)] errorHandler: OptValueRef<'_>,
    ) -> Result<Value> {
        let parser = make_parser_impl(_interp, None)?;
        call_method(_interp, &parser, "setContentHandler", &[handler.0])?;
        let eh = match errorHandler.as_value() {
            Some(v) => v.clone(),
            None => {
                let h_mod = _interp.load_module("xml.sax.handler")?;
                let eh_cls = _interp.get_attr(&h_mod, "ErrorHandler")?;
                call_pos(_interp, eh_cls, &[])?
            }
        };
        call_method(_interp, &parser, "setErrorHandler", &[eh])?;
        call_method(_interp, &parser, "parse", &[source.0])?;
        Ok(Value::none())
    }

    #[py_name = "parseString"]
    fn parse_string(
        string: PyValue,
        handler: PyValue,
        #[default(OptionalArg::Omitted)] errorHandler: OptValueRef<'_>,
    ) -> Result<Value> {
        let io_mod = _interp.load_module("io")?;
        let is_str = matches!(string.0.kind(), ValueKind::Str(_));
        let stream = if is_str {
            let sio_cls = _interp.get_attr(&io_mod, "StringIO")?;
            call_pos(_interp, sio_cls, &[string.0])?
        } else {
            let bio_cls = _interp.get_attr(&io_mod, "BytesIO")?;
            call_pos(_interp, bio_cls, &[string.0])?
        };
        let xr_mod = _interp.load_module("xml.sax.xmlreader")?;
        let is_cls = _interp.get_attr(&xr_mod, "InputSource")?;
        let inpsrc = call_pos(_interp, is_cls, &[])?;
        if matches!(
            _interp
                .get_attr(&stream, "__class__")
                .ok()
                .and_then(|c| _interp.get_attr(&c, "__name__").ok())
                .and_then(|n| n.as_str().map(str::to_string))
                .as_deref(),
            Some("StringIO")
        ) {
            call_method(_interp, &inpsrc, "setCharacterStream", &[stream])?;
        } else {
            call_method(_interp, &inpsrc, "setByteStream", &[stream])?;
        }
        let parser = make_parser_impl(_interp, None)?;
        call_method(_interp, &parser, "setContentHandler", &[handler.0])?;
        let eh = match errorHandler.as_value() {
            Some(v) => v.clone(),
            None => {
                let h_mod = _interp.load_module("xml.sax.handler")?;
                let eh_cls = _interp.get_attr(&h_mod, "ErrorHandler")?;
                call_pos(_interp, eh_cls, &[])?
            }
        };
        call_method(_interp, &parser, "setErrorHandler", &[eh])?;
        call_method(_interp, &parser, "parse", &[inpsrc])?;
        Ok(Value::none())
    }
}
