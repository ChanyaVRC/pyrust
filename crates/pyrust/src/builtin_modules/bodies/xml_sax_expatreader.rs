// `xml.sax.expatreader` module — 100% native-Rust Python 3.12 SAX2 driver
// backed by PyRust's pure-Rust `pyexpat` engine (issue #3260).
//
// Provides `ExpatParser`, `ExpatLocator`, and `create_parser`.
//
// Reference: <https://docs.python.org/3/library/xml.sax.reader.html>

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

use super::xml_sax_handler::{
    FEATURE_EXTERNAL_GES, FEATURE_EXTERNAL_PES, FEATURE_NAMESPACE_PREFIXES, FEATURE_NAMESPACES,
    FEATURE_STRING_INTERNING, FEATURE_VALIDATION, PROPERTY_INTERNING_DICT,
    PROPERTY_LEXICAL_HANDLER, PROPERTY_XML_STRING,
};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

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

fn is_expat_error(interp: &mut Interpreter, err: &PyError) -> Option<Value> {
    let exc_val = interp.materialize_pyerror(err.clone()).ok()?;
    let pyexpat_mod = interp.load_module("pyexpat").ok()?;
    let expat_err_cls = interp.get_attr(&pyexpat_mod, "ExpatError").ok()?;
    let matched = call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[exc_val.clone(), expat_err_cls],
    )
    .ok()
    .and_then(|v| interp.truthy_value(&v).ok())
    .unwrap_or(false);
    if matched { Some(exc_val) } else { None }
}

fn split_expat_ns_name(raw: &str) -> (Value, String, Value) {
    let parts: Vec<&str> = raw.split(' ').collect();
    match parts.len() {
        1 => (
            Value::tuple(vec![Value::none(), Value::string(parts[0])]),
            parts[0].to_string(),
            Value::string(parts[0]),
        ),
        2 => (
            Value::tuple(vec![Value::string(parts[0]), Value::string(parts[1])]),
            parts[1].to_string(),
            Value::string(parts[1]),
        ),
        _ => {
            let qname = format!("{}:{}", parts[2], parts[1]);
            (
                Value::tuple(vec![Value::string(parts[0]), Value::string(parts[1])]),
                qname.clone(),
                Value::string(qname),
            )
        }
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let sax_mod = interp.load_module("xml.sax")?;
    let h_mod = interp.load_module("xml.sax.handler")?;
    let xr_mod = interp.load_module("xml.sax.xmlreader")?;
    let su_mod = interp.load_module("xml.sax.saxutils")?;
    let expat_mod = interp.load_module("xml.parsers.expat")?;

    let inc_cls = interp.get_attr(&xr_mod, "IncrementalParser")?;
    let loc_cls = interp.get_attr(&xr_mod, "Locator")?;

    let (ep_val, el_val) = {
        let m = module.borrow();
        (
            m.get_attr_value("ExpatParser"),
            m.get_attr_value("ExpatLocator"),
        )
    };
    if let (Some(ep), ValueKind::PyClass(ic), ValueKind::PyClass(lc)) =
        (ep_val.as_ref(), inc_cls.kind(), loc_cls.kind())
        && let ValueKind::PyClass(epc) = ep.kind()
    {
        link_class_bases(epc, vec![Rc::clone(ic), Rc::clone(lc)]);
    }
    if let (Some(el), ValueKind::PyClass(lc)) = (el_val.as_ref(), loc_cls.kind())
        && let ValueKind::PyClass(elc) = el.kind()
    {
        link_class_bases(elc, vec![Rc::clone(lc)]);
    }

    let mut m = module.borrow_mut();
    m.insert_attr("expat".to_string(), expat_mod);
    m.insert_attr("handler".to_string(), h_mod.clone());
    m.insert_attr("xmlreader".to_string(), xr_mod.clone());
    m.insert_attr("saxutils".to_string(), su_mod);
    for name in [
        "SAXException",
        "SAXParseException",
        "SAXNotRecognizedException",
        "SAXNotSupportedException",
        "SAXReaderNotAvailable",
    ] {
        if let Ok(v) = interp.get_attr(&sax_mod, name) {
            m.insert_attr(name.to_string(), v);
        }
    }
    for name in ["AttributesImpl", "AttributesNSImpl"] {
        if let Ok(v) = interp.get_attr(&xr_mod, name) {
            m.insert_attr(name.to_string(), v);
        }
    }
    for name in [
        "feature_namespaces",
        "feature_namespace_prefixes",
        "feature_string_interning",
        "feature_validation",
        "feature_external_ges",
        "feature_external_pes",
        "property_interning_dict",
        "property_xml_string",
    ] {
        if let Ok(v) = interp.get_attr(&h_mod, name) {
            m.insert_attr(name.to_string(), v);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.sax"),
        "version" => Value::string("0.20"),
    }

    fn create_parser(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        let m = _interp.load_module("xml.sax.expatreader")?;
        let cls = _interp.get_attr(&m, "ExpatParser")?;
        let mut expanded: Vec<ExpandedCallArg> = args
            .iter()
            .cloned()
            .map(|value| ExpandedCallArg { name: None, value })
            .collect();
        for (k, v) in kwargs {
            expanded.push(ExpandedCallArg { name: Some(k), value: v });
        }
        _interp.call_function_expanded(cls, &expanded)
    }

    class ExpatLocator {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            parser: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_ref", parser.0)?;
            Ok(Value::none())
        }

        #[py_name = "getColumnNumber"]
        fn get_column_number(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let p = _interp.get_attr(self_val.0, "_ref")?;
            if p.is_none() {
                return Ok(Value::none());
            }
            call_method(_interp, &p, "getColumnNumber", &[])
        }

        #[py_name = "getLineNumber"]
        fn get_line_number(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let p = _interp.get_attr(self_val.0, "_ref")?;
            if p.is_none() {
                return Ok(Value::int(1));
            }
            call_method(_interp, &p, "getLineNumber", &[])
        }

        #[py_name = "getPublicId"]
        fn get_public_id(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let p = _interp.get_attr(self_val.0, "_ref")?;
            if p.is_none() {
                return Ok(Value::none());
            }
            call_method(_interp, &p, "getPublicId", &[])
        }

        #[py_name = "getSystemId"]
        fn get_system_id(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let p = _interp.get_attr(self_val.0, "_ref")?;
            if p.is_none() {
                return Ok(Value::none());
            }
            call_method(_interp, &p, "getSystemId", &[])
        }
    }

    class ExpatParser {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] namespaceHandling: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] bufsize: OptValueRef<'_>,
        ) -> Result<Value> {
            let xr_mod = _interp.load_module("xml.sax.xmlreader")?;
            let inc_cls = _interp.get_attr(&xr_mod, "IncrementalParser")?;
            let inc_init = _interp.get_attr(&inc_cls, "__init__")?;
            let bs = bufsize
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::int(65516));
            call_pos(_interp, inc_init, &[self_val.0.clone(), bs])?;
            let ns_h = namespaceHandling
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::int(0));
            _interp.assign_attr(self_val.0.clone(), "_namespaces", ns_h)?;
            _interp.assign_attr(self_val.0.clone(), "_parser", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_source", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_parsing", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "_entity_stack", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "_external_ges", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "_interning", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_lex_handler_prop", Value::none())?;
            Ok(Value::none())
        }

        fn parse(#[positional_only] self_val: PyValueRef<'_>, source: PyValue) -> Result<Value> {
            let su_mod = _interp.load_module("xml.sax.saxutils")?;
            let prep = _interp.get_attr(&su_mod, "prepare_input_source")?;
            let prepared = call_pos(_interp, prep, &[source.0])?;
            _interp.assign_attr(self_val.0.clone(), "_source", prepared.clone())?;
            let xr_mod = _interp.load_module("xml.sax.xmlreader")?;
            let inc_cls = _interp.get_attr(&xr_mod, "IncrementalParser")?;
            let inc_parse = _interp.get_attr(&inc_cls, "parse")?;
            let res = call_pos(_interp, inc_parse, &[self_val.0.clone(), prepared]);
            let _ = call_method(_interp, self_val.0, "_close_source", &[]);
            res
        }

        #[py_name = "prepareParser"]
        fn prepare_parser(
            #[positional_only] self_val: PyValueRef<'_>,
            source: PyValue,
        ) -> Result<Value> {
            let sysid = call_method(_interp, &source.0, "getSystemId", &[])?;
            if !sysid.is_none() {
                let parser = _interp.get_attr(self_val.0, "_parser")?;
                if !parser.is_none() {
                    let _ = call_method(_interp, &parser, "SetBase", &[sysid]);
                }
            }
            Ok(Value::none())
        }

        #[py_name = "setContentHandler"]
        fn set_content_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            handler: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_cont_handler", handler.0)?;
            let parsing = _interp.get_attr(self_val.0, "_parsing")?;
            if _interp.truthy_value(&parsing)? {
                call_method(_interp, self_val.0, "_reset_cont_handler", &[])?;
            }
            Ok(Value::none())
        }

        #[py_name = "getFeature"]
        fn get_feature(#[positional_only] self_val: PyValueRef<'_>, name: PyStr<'_>) -> Result<Value> {
            match name.0.as_ref() {
                FEATURE_NAMESPACES => _interp.get_attr(self_val.0, "_namespaces"),
                FEATURE_STRING_INTERNING => {
                    let intern = _interp.get_attr(self_val.0, "_interning")?;
                    Ok(Value::bool_(!intern.is_none()))
                }
                FEATURE_VALIDATION | FEATURE_EXTERNAL_PES | FEATURE_NAMESPACE_PREFIXES => {
                    Ok(Value::int(0))
                }
                FEATURE_EXTERNAL_GES => _interp.get_attr(self_val.0, "_external_ges"),
                _ => Err(raise_sax_exc(
                    _interp,
                    "SAXNotRecognizedException",
                    format!("Feature '{}' not recognized", name.0),
                )),
            }
        }

        #[py_name = "setFeature"]
        fn set_feature(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            state: PyValue,
        ) -> Result<Value> {
            let parsing = _interp.get_attr(self_val.0, "_parsing")?;
            if _interp.truthy_value(&parsing)? {
                return Err(raise_sax_exc(
                    _interp,
                    "SAXNotSupportedException",
                    "Cannot set features while parsing".to_string(),
                ));
            }
            let truthy = _interp.truthy_value(&state.0)?;
            match name.0.as_ref() {
                FEATURE_NAMESPACES => {
                    _interp.assign_attr(self_val.0.clone(), "_namespaces", state.0)?;
                }
                FEATURE_EXTERNAL_GES => {
                    _interp.assign_attr(self_val.0.clone(), "_external_ges", state.0)?;
                }
                FEATURE_STRING_INTERNING => {
                    if truthy {
                        let cur = _interp.get_attr(self_val.0, "_interning")?;
                        if cur.is_none() {
                            _interp.assign_attr(
                                self_val.0.clone(),
                                "_interning",
                                Value::dict(PyDict::default()),
                            )?;
                        }
                    } else {
                        _interp.assign_attr(self_val.0.clone(), "_interning", Value::none())?;
                    }
                }
                FEATURE_VALIDATION => {
                    if truthy {
                        return Err(raise_sax_exc(
                            _interp,
                            "SAXNotSupportedException",
                            "expat does not support validation".to_string(),
                        ));
                    }
                }
                FEATURE_EXTERNAL_PES => {
                    if truthy {
                        return Err(raise_sax_exc(
                            _interp,
                            "SAXNotSupportedException",
                            "expat does not read external parameter entities".to_string(),
                        ));
                    }
                }
                FEATURE_NAMESPACE_PREFIXES => {
                    if truthy {
                        return Err(raise_sax_exc(
                            _interp,
                            "SAXNotSupportedException",
                            "expat does not report namespace prefixes".to_string(),
                        ));
                    }
                }
                _ => {
                    return Err(raise_sax_exc(
                        _interp,
                        "SAXNotRecognizedException",
                        format!("Feature '{}' not recognized", name.0),
                    ));
                }
            }
            Ok(Value::none())
        }

        #[py_name = "getProperty"]
        fn get_property(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            match name.0.as_ref() {
                PROPERTY_LEXICAL_HANDLER => _interp.get_attr(self_val.0, "_lex_handler_prop"),
                PROPERTY_INTERNING_DICT => _interp.get_attr(self_val.0, "_interning"),
                PROPERTY_XML_STRING => {
                    let parser = _interp.get_attr(self_val.0, "_parser")?;
                    if !parser.is_none()
                        && let Ok(ctx) = call_method(_interp, &parser, "GetInputContext", &[])
                        && !ctx.is_none()
                    {
                        return Ok(ctx);
                    }
                    Err(raise_sax_exc(
                        _interp,
                        "SAXNotRecognizedException",
                        "This version of expat does not support getting the XML string".to_string(),
                    ))
                }
                _ => Err(raise_sax_exc(
                    _interp,
                    "SAXNotRecognizedException",
                    format!("Property '{}' not recognized", name.0),
                )),
            }
        }

        #[py_name = "setProperty"]
        fn set_property(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            value: PyValue,
        ) -> Result<Value> {
            match name.0.as_ref() {
                PROPERTY_LEXICAL_HANDLER => {
                    _interp.assign_attr(self_val.0.clone(), "_lex_handler_prop", value.0)?;
                    let parsing = _interp.get_attr(self_val.0, "_parsing")?;
                    if _interp.truthy_value(&parsing)? {
                        call_method(_interp, self_val.0, "_reset_lex_handler_prop", &[])?;
                    }
                    Ok(Value::none())
                }
                PROPERTY_INTERNING_DICT => {
                    _interp.assign_attr(self_val.0.clone(), "_interning", value.0)?;
                    Ok(Value::none())
                }
                PROPERTY_XML_STRING => Err(raise_sax_exc(
                    _interp,
                    "SAXNotSupportedException",
                    format!("Property '{}' cannot be set", name.0),
                )),
                _ => Err(raise_sax_exc(
                    _interp,
                    "SAXNotRecognizedException",
                    format!("Property '{}' not recognized", name.0),
                )),
            }
        }

        fn feed(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyValue,
            #[default(false.into())] isFinal: PyBool,
        ) -> Result<Value> {
            let parsing = _interp.get_attr(self_val.0, "_parsing")?;
            if !_interp.truthy_value(&parsing)? {
                call_method(_interp, self_val.0, "reset", &[])?;
                _interp.assign_attr(self_val.0.clone(), "_parsing", Value::int(1))?;
                let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
                call_method(_interp, &ch, "startDocument", &[])?;
            }
            let parser = _interp.get_attr(self_val.0, "_parser")?;
            let res = call_method(_interp, &parser, "Parse", &[data.0, Value::bool_(isFinal.0)]);
            if let Err(err) = res {
                if let Some(expat_exc) = is_expat_error(_interp, &err) {
                    let msg = _interp.render_value_as_str(&expat_exc)?;
                    let sax_mod = _interp.load_module("xml.sax")?;
                    let spe_cls = _interp.get_attr(&sax_mod, "SAXParseException")?;
                    let spe = call_pos(
                        _interp,
                        spe_cls,
                        &[Value::string(msg), expat_exc, self_val.0.clone()],
                    )?;
                    let eh = _interp.get_attr(self_val.0, "_err_handler")?;
                    call_method(_interp, &eh, "fatalError", &[spe])?;
                    return Ok(Value::none());
                }
                return Err(err);
            }
            Ok(Value::none())
        }

        fn flush(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let parser = _interp.get_attr(self_val.0, "_parser")?;
            if parser.is_none() {
                return Ok(Value::none());
            }
            let was_enabled = _interp.get_attr(&parser, "buffer_text")?;
            let _ = _interp.assign_attr(parser.clone(), "buffer_text", Value::bool_(false));
            let _ = _interp.assign_attr(parser, "buffer_text", was_enabled);
            Ok(Value::none())
        }

        fn _close_source(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let source = _interp.get_attr(self_val.0, "_source")?;
            if source.is_none() {
                return Ok(Value::none());
            }
            let char_s = call_method(_interp, &source, "getCharacterStream", &[])?;
            if !char_s.is_none()
                && let Ok(close_fn) = _interp.get_attr(&char_s, "close")
            {
                let _ = call_pos(_interp, close_fn, &[]);
            }
            let byte_s = call_method(_interp, &source, "getByteStream", &[])?;
            if !byte_s.is_none()
                && let Ok(close_fn) = _interp.get_attr(&byte_s, "close")
            {
                let _ = call_pos(_interp, close_fn, &[]);
            }
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let entity_stack = _interp.get_attr(self_val.0, "_entity_stack")?;
            if _interp.truthy_value(&entity_stack)? {
                return Ok(Value::none());
            }
            let res = (|| -> Result<()> {
                call_method(
                    _interp,
                    self_val.0,
                    "feed",
                    &[Value::bytes(Vec::new()), Value::bool_(true)],
                )?;
                let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
                call_method(_interp, &ch, "endDocument", &[])?;
                _interp.assign_attr(self_val.0.clone(), "_parsing", Value::int(0))?;
                _interp.assign_attr(self_val.0.clone(), "_parser", Value::none())?;
                Ok(())
            })();
            _interp.assign_attr(self_val.0.clone(), "_parsing", Value::int(0))?;
            if let Ok(p) = _interp.get_attr(self_val.0, "_parser")
                && !p.is_none()
            {
                _interp.assign_attr(self_val.0.clone(), "_parser", Value::none())?;
            }
            res?;
            Ok(Value::none())
        }

        fn _reset_cont_handler(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let parser = _interp.get_attr(self_val.0, "_parser")?;
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            let pi_cb = _interp.get_attr(&ch, "processingInstruction")?;
            let char_cb = _interp.get_attr(&ch, "characters")?;
            _interp.assign_attr(parser.clone(), "ProcessingInstructionHandler", pi_cb)?;
            _interp.assign_attr(parser, "CharacterDataHandler", char_cb)?;
            Ok(Value::none())
        }

        fn _reset_lex_handler_prop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let parser = _interp.get_attr(self_val.0, "_parser")?;
            let lex = _interp.get_attr(self_val.0, "_lex_handler_prop")?;
            if lex.is_none() {
                _interp.assign_attr(parser.clone(), "CommentHandler", Value::none())?;
                _interp.assign_attr(parser.clone(), "StartCdataSectionHandler", Value::none())?;
                _interp.assign_attr(parser.clone(), "EndCdataSectionHandler", Value::none())?;
                _interp.assign_attr(parser.clone(), "StartDoctypeDeclHandler", Value::none())?;
                _interp.assign_attr(parser, "EndDoctypeDeclHandler", Value::none())?;
            } else {
                let c_h = _interp.get_attr(&lex, "comment")?;
                _interp.assign_attr(parser.clone(), "CommentHandler", c_h)?;
                let sc_h = _interp.get_attr(&lex, "startCDATA")?;
                _interp.assign_attr(parser.clone(), "StartCdataSectionHandler", sc_h)?;
                let ec_h = _interp.get_attr(&lex, "endCDATA")?;
                _interp.assign_attr(parser.clone(), "EndCdataSectionHandler", ec_h)?;
                let sd_h = _interp.get_attr(self_val.0, "start_doctype_decl")?;
                _interp.assign_attr(parser.clone(), "StartDoctypeDeclHandler", sd_h)?;
                let ed_h = _interp.get_attr(&lex, "endDTD")?;
                _interp.assign_attr(parser, "EndDoctypeDeclHandler", ed_h)?;
            }
            Ok(Value::none())
        }

        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let expat_mod = _interp.load_module("xml.parsers.expat")?;
            let create = _interp.get_attr(&expat_mod, "ParserCreate")?;
            let ns_val = _interp.get_attr(self_val.0, "_namespaces")?;
            let use_ns = _interp.truthy_value(&ns_val)?;
            let intern = _interp.get_attr(self_val.0, "_interning")?;
            let encoding = {
                let src = _interp.get_attr(self_val.0, "_source")?;
                if src.is_none() {
                    Value::none()
                } else {
                    call_method(_interp, &src, "getEncoding", &[])?
                }
            };
            let parser = if use_ns {
                let p = call_kw(
                    _interp,
                    create,
                    &[encoding, Value::string(" ")],
                    &[("intern", intern)],
                )?;
                _interp.assign_attr(p.clone(), "namespace_prefixes", Value::int(1))?;
                let se = _interp.get_attr(self_val.0, "start_element_ns")?;
                _interp.assign_attr(p.clone(), "StartElementHandler", se)?;
                let ee = _interp.get_attr(self_val.0, "end_element_ns")?;
                _interp.assign_attr(p.clone(), "EndElementHandler", ee)?;
                let sn = _interp.get_attr(self_val.0, "start_namespace_decl")?;
                _interp.assign_attr(p.clone(), "StartNamespaceDeclHandler", sn)?;
                let en = _interp.get_attr(self_val.0, "end_namespace_decl")?;
                _interp.assign_attr(p.clone(), "EndNamespaceDeclHandler", en)?;
                p
            } else {
                let p = call_kw(_interp, create, &[encoding], &[("intern", intern)])?;
                let se = _interp.get_attr(self_val.0, "start_element")?;
                _interp.assign_attr(p.clone(), "StartElementHandler", se)?;
                let ee = _interp.get_attr(self_val.0, "end_element")?;
                _interp.assign_attr(p.clone(), "EndElementHandler", ee)?;
                p
            };

            _interp.assign_attr(self_val.0.clone(), "_parser", parser.clone())?;
            call_method(_interp, self_val.0, "_reset_cont_handler", &[])?;
            let ue = _interp.get_attr(self_val.0, "unparsed_entity_decl")?;
            _interp.assign_attr(parser.clone(), "UnparsedEntityDeclHandler", ue)?;
            let nd = _interp.get_attr(self_val.0, "notation_decl")?;
            _interp.assign_attr(parser.clone(), "NotationDeclHandler", nd)?;
            let sk = _interp.get_attr(self_val.0, "skipped_entity_handler")?;
            _interp.assign_attr(parser.clone(), "SkippedEntityHandler", sk)?;
            _interp.assign_attr(parser.clone(), "buffer_text", Value::int(1))?;

            let lex = _interp.get_attr(self_val.0, "_lex_handler_prop")?;
            if !lex.is_none() {
                call_method(_interp, self_val.0, "_reset_lex_handler_prop", &[])?;
            }

            let er_mod = _interp.load_module("xml.sax.expatreader")?;
            let loc_cls = _interp.get_attr(&er_mod, "ExpatLocator")?;
            let loc = call_pos(_interp, loc_cls, std::slice::from_ref(self_val.0))?;
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "setDocumentLocator", &[loc])?;
            Ok(Value::none())
        }

        // Locator methods
        #[py_name = "getColumnNumber"]
        fn get_column_number(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let parser = _interp.get_attr(self_val.0, "_parser")?;
            if parser.is_none() {
                return Ok(Value::none());
            }
            _interp.get_attr(&parser, "ErrorColumnNumber")
        }

        #[py_name = "getLineNumber"]
        fn get_line_number(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let parser = _interp.get_attr(self_val.0, "_parser")?;
            if parser.is_none() {
                return Ok(Value::int(1));
            }
            _interp.get_attr(&parser, "ErrorLineNumber")
        }

        #[py_name = "getPublicId"]
        fn get_public_id(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let src = _interp.get_attr(self_val.0, "_source")?;
            if src.is_none() {
                return Ok(Value::none());
            }
            call_method(_interp, &src, "getPublicId", &[])
        }

        #[py_name = "getSystemId"]
        fn get_system_id(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let src = _interp.get_attr(self_val.0, "_source")?;
            if src.is_none() {
                return Ok(Value::none());
            }
            call_method(_interp, &src, "getSystemId", &[])
        }

        // Expat event handlers
        fn start_element(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            attrs: PyValue,
        ) -> Result<Value> {
            let xr_mod = _interp.load_module("xml.sax.xmlreader")?;
            let attr_cls = _interp.get_attr(&xr_mod, "AttributesImpl")?;
            let wrapped = call_pos(_interp, attr_cls, &[attrs.0])?;
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "startElement", &[name.0, wrapped])
        }

        fn end_element(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "endElement", &[name.0])
        }

        fn start_element_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            attrs: PyValue,
        ) -> Result<Value> {
            let (el_pair, _q_str, _el_qname) = split_expat_ns_name(&name.0);
            let newattrs = Value::dict(PyDict::default());
            let qnames = Value::dict(PyDict::default());
            let items = call_method(_interp, &attrs.0, "items", &[])?;
            for pair in _interp.collect_iterable(&items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    let raw_k = kv[0].as_str().unwrap_or("");
                    let (key_tuple, _qs, qv) = split_expat_ns_name(raw_k);
                    call_method(_interp, &newattrs, "__setitem__", &[key_tuple.clone(), kv[1].clone()])?;
                    call_method(_interp, &qnames, "__setitem__", &[key_tuple, qv])?;
                }
            }
            let xr_mod = _interp.load_module("xml.sax.xmlreader")?;
            let attr_ns_cls = _interp.get_attr(&xr_mod, "AttributesNSImpl")?;
            let wrapped = call_pos(_interp, attr_ns_cls, &[newattrs, qnames])?;
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "startElementNS", &[el_pair, Value::none(), wrapped])
        }

        fn end_element_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let (el_pair, _q_str, _el_qname) = split_expat_ns_name(&name.0);
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "endElementNS", &[el_pair, Value::none()])
        }

        fn processing_instruction(
            #[positional_only] self_val: PyValueRef<'_>,
            target: PyValue,
            data: PyValue,
        ) -> Result<Value> {
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "processingInstruction", &[target.0, data.0])
        }

        fn character_data(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyValue,
        ) -> Result<Value> {
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "characters", &[data.0])
        }

        fn start_namespace_decl(
            #[positional_only] self_val: PyValueRef<'_>,
            prefix: PyValue,
            uri: PyValue,
        ) -> Result<Value> {
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "startPrefixMapping", &[prefix.0, uri.0])
        }

        fn end_namespace_decl(
            #[positional_only] self_val: PyValueRef<'_>,
            prefix: PyValue,
        ) -> Result<Value> {
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "endPrefixMapping", &[prefix.0])
        }

        fn start_doctype_decl(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            sysid: PyValue,
            pubid: PyValue,
            _has_internal_subset: PyValue,
        ) -> Result<Value> {
            let lex = _interp.get_attr(self_val.0, "_lex_handler_prop")?;
            if !lex.is_none() {
                call_method(_interp, &lex, "startDTD", &[name.0, pubid.0, sysid.0])?;
            }
            Ok(Value::none())
        }

        fn unparsed_entity_decl(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            _base: PyValue,
            sysid: PyValue,
            pubid: PyValue,
            notation_name: PyValue,
        ) -> Result<Value> {
            let dtd = _interp.get_attr(self_val.0, "_dtd_handler")?;
            call_method(
                _interp,
                &dtd,
                "unparsedEntityDecl",
                &[name.0, pubid.0, sysid.0, notation_name.0],
            )
        }

        fn notation_decl(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            _base: PyValue,
            sysid: PyValue,
            pubid: PyValue,
        ) -> Result<Value> {
            let dtd = _interp.get_attr(self_val.0, "_dtd_handler")?;
            call_method(_interp, &dtd, "notationDecl", &[name.0, pubid.0, sysid.0])
        }

        fn external_entity_ref(
            #[positional_only] _self: PyValueRef<'_>,
            _context: PyValue,
            _base: PyValue,
            _sysid: PyValue,
            _pubid: PyValue,
        ) -> Result<Value> {
            Ok(Value::int(1))
        }

        fn skipped_entity_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            is_pe: PyValue,
        ) -> Result<Value> {
            let ent = if _interp.truthy_value(&is_pe.0)? {
                format!("%{}", name.0)
            } else {
                name.0.to_string()
            };
            let ch = _interp.get_attr(self_val.0, "_cont_handler")?;
            call_method(_interp, &ch, "skippedEntity", &[Value::string(ent)])
        }
    }
}
