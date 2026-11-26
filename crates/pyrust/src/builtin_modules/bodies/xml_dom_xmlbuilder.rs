// `xml.dom.xmlbuilder` module — 100% native-Rust Python 3.12 DOM Level 3
// Load/Save builder interfaces: `Options`, `DOMBuilder`, `DOMEntityResolver`,
// `DOMInputSource`, `DOMBuilderFilter`, `DocumentLS`, `DOMImplementationLS`,
// and `makeBuilder` (issue #3260).
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/xml/dom/xmlbuilder.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value};

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

fn raise_dom_exc(interp: &mut Interpreter, exc_name: &'static str, msg: &str) -> PyError {
    if let Ok(dom_mod) = interp.load_module("xml.dom")
        && let Ok(cls) = interp.get_attr(&dom_mod, exc_name)
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(msg)])
    {
        return PyError::raised(inst);
    }
    PyError::named(exc_name, msg)
}

fn norm_feature(name: &str) -> String {
    name.to_ascii_lowercase().replace('-', "_")
}

/// Supported `(feature_name, state_int)` table matching CPython's `DOMBuilder._settings`.
fn feature_supported(name: &str, state: i64) -> Option<bool> {
    let s = state != 0;
    match name {
        "namespace_declarations" => Some(true),
        "validation" => Some(!s),
        "external_general_entities" => Some(true),
        "external_parameter_entities" => Some(true),
        "validate_if_schema" => Some(!s),
        "create_entity_ref_nodes" => Some(true),
        "entities" => Some(true),
        "whitespace_in_element_content" => Some(true),
        "cdata_sections" => Some(true),
        "comments" => Some(true),
        "charset_overrides_xml_encoding" => Some(true),
        "infoset" => Some(!s),
        "supported_mediatypes_only" => Some(!s),
        "namespaces" => Some(true),
        _ => None,
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    if let Ok(copy_m) = interp.load_module("copy") {
        module.borrow_mut().insert_attr("copy".to_string(), copy_m);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.dom"),
        "__all__" => Value::list(vec![
            Value::string("DOMBuilder"),
            Value::string("DOMEntityResolver"),
            Value::string("DOMInputSource"),
        ]),
    }

    class Options {
        constants {
            "namespaces" => Value::int(1),
            "namespace_declarations" => Value::bool_(true),
            "validation" => Value::bool_(false),
            "external_parameter_entities" => Value::bool_(true),
            "external_general_entities" => Value::bool_(true),
            "external_dtd_subset" => Value::bool_(true),
            "validate_if_schema" => Value::bool_(false),
            "validate" => Value::bool_(false),
            "datatype_normalization" => Value::bool_(false),
            "create_entity_ref_nodes" => Value::bool_(true),
            "entities" => Value::bool_(true),
            "whitespace_in_element_content" => Value::bool_(true),
            "cdata_sections" => Value::bool_(true),
            "comments" => Value::bool_(true),
            "charset_overrides_xml_encoding" => Value::bool_(true),
            "infoset" => Value::bool_(false),
            "supported_mediatypes_only" => Value::bool_(false),
            "errorHandler" => Value::none(),
            "filter" => Value::none(),
        }
    }

    class DOMBuilder {
        constants {
            "ACTION_REPLACE" => Value::int(1),
            "ACTION_APPEND_AS_CHILDREN" => Value::int(2),
            "ACTION_INSERT_AFTER" => Value::int(3),
            "ACTION_INSERT_BEFORE" => Value::int(4),
        }

        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let xb_mod = _interp.load_module("xml.dom.xmlbuilder")?;
            let opt_cls = _interp.get_attr(&xb_mod, "Options")?;
            let opts = call_pos(_interp, opt_cls, &[])?;
            _interp.assign_attr(self_val.0.clone(), "_options", opts)?;
            _interp.assign_attr(self_val.0.clone(), "entityResolver", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "errorHandler", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "filter", Value::none())?;
            Ok(Value::none())
        }

        #[py_name = "setFeature"]
        fn set_feature(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            state: PyValue,
        ) -> Result<Value> {
            let norm = norm_feature(&name.0);
            let st_bool = _interp.truthy_value(&state.0)?;
            let st_int = if st_bool { 1 } else { 0 };
            match feature_supported(&norm, st_int) {
                Some(true) => {
                    let opts = _interp.get_attr(self_val.0, "_options")?;
                    _interp.assign_attr(opts, &norm, Value::int(st_int))?;
                    Ok(Value::none())
                }
                Some(false) => Err(raise_dom_exc(
                    _interp,
                    "NotSupportedErr",
                    &format!("unsupported feature state for {:?}", name.0),
                )),
                None => Err(raise_dom_exc(
                    _interp,
                    "NotFoundErr",
                    &format!("unknown feature: {:?}", name.0),
                )),
            }
        }

        #[py_name = "supportsFeature"]
        fn supports_feature(
            #[positional_only] _self: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let norm = norm_feature(&name.0);
            Ok(Value::bool_(feature_supported(&norm, 1).is_some()))
        }

        #[py_name = "canSetFeature"]
        fn can_set_feature(
            #[positional_only] _self: PyValueRef<'_>,
            name: PyStr<'_>,
            state: PyValue,
        ) -> Result<Value> {
            let norm = norm_feature(&name.0);
            let st_int = if _interp.truthy_value(&state.0)? { 1 } else { 0 };
            Ok(Value::bool_(feature_supported(&norm, st_int) == Some(true)))
        }

        #[py_name = "getFeature"]
        fn get_feature(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let norm = norm_feature(&name.0);
            let opts = _interp.get_attr(self_val.0, "_options")?;
            match _interp.get_attr(&opts, &norm) {
                Ok(v) => Ok(v),
                Err(_) => Err(raise_dom_exc(
                    _interp,
                    "NotFoundErr",
                    &format!("feature {:?} not known", name.0),
                )),
            }
        }

        #[py_name = "parseURI"]
        fn parse_uri(
            #[positional_only] self_val: PyValueRef<'_>,
            uri: PyStr<'_>,
        ) -> Result<Value> {
            let resolver = _interp.get_attr(self_val.0, "entityResolver")?;
            let source = if !resolver.is_none() {
                call_method(
                    _interp,
                    &resolver,
                    "resolveEntity",
                    &[Value::none(), Value::string(uri.0.as_ref())],
                )?
            } else {
                let xb_mod = _interp.load_module("xml.dom.xmlbuilder")?;
                let er_cls = _interp.get_attr(&xb_mod, "DOMEntityResolver")?;
                let er = call_pos(_interp, er_cls, &[])?;
                call_method(
                    _interp,
                    &er,
                    "resolveEntity",
                    &[Value::none(), Value::string(uri.0.as_ref())],
                )?
            };
            call_method(_interp, self_val.0, "parse", &[source])
        }

        fn parse(
            #[positional_only] self_val: PyValueRef<'_>,
            input: PyValue,
        ) -> Result<Value> {
            let opts = _interp.get_attr(self_val.0, "_options")?;
            let eb_mod = _interp.load_module("xml.dom.expatbuilder")?;
            let make_b = _interp.get_attr(&eb_mod, "makeBuilder")?;
            let builder = call_pos(_interp, make_b, &[opts])?;
            let byte_s = _interp.get_attr(&input.0, "byteStream")?;
            if !byte_s.is_none() {
                return call_method(_interp, &builder, "parseFile", &[byte_s]);
            }
            let char_s = _interp.get_attr(&input.0, "characterStream")?;
            if !char_s.is_none() {
                return call_method(_interp, &builder, "parseFile", &[char_s]);
            }
            let str_data = _interp.get_attr(&input.0, "stringData")?;
            if !str_data.is_none() {
                return call_method(_interp, &builder, "parseString", &[str_data]);
            }
            let sysid = _interp.get_attr(&input.0, "systemId")?;
            if !sysid.is_none() {
                return call_method(_interp, self_val.0, "parseURI", &[sysid]);
            }
            Err(PyError::named("ValueError", "DOMInputSource has no input data"))
        }

        #[py_name = "parseWithContext"]
        fn parse_with_context(
            #[positional_only] _self: PyValueRef<'_>,
            _input: PyValue,
            _cnode: PyValue,
            action: PyIndex,
        ) -> Result<Value> {
            let act = action.expect_i64(FN_NAME, "action")?;
            if !(1..=4).contains(&act) {
                return Err(PyError::named("ValueError", "not a legal action"));
            }
            Err(PyError::named(
                "NotImplementedError",
                "Haven't written this yet...",
            ))
        }
    }

    class DOMEntityResolver {
        #[py_name = "resolveEntity"]
        fn resolve_entity(
            #[positional_only] _self: PyValueRef<'_>,
            publicId: PyValue,
            systemId: PyValue,
        ) -> Result<Value> {
            let xb_mod = _interp.load_module("xml.dom.xmlbuilder")?;
            let is_cls = _interp.get_attr(&xb_mod, "DOMInputSource")?;
            let source = call_pos(_interp, is_cls, &[])?;
            _interp.assign_attr(source.clone(), "publicId", publicId.0)?;
            _interp.assign_attr(source.clone(), "systemId", systemId.0.clone())?;
            if let Some(sys_s) = systemId.0.as_str() {
                let f = call_pos(
                    _interp,
                    Value::builtin_function("open"),
                    &[Value::string(sys_s), Value::string("rb")],
                )?;
                _interp.assign_attr(source.clone(), "byteStream", f)?;
            }
            Ok(source)
        }
    }

    class DOMInputSource {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "byteStream", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "characterStream", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "stringData", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "encoding", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "publicId", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "systemId", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "baseURI", Value::none())?;
            Ok(Value::none())
        }
    }

    class DOMBuilderFilter {
        constants {
            "FILTER_ACCEPT" => Value::int(1),
            "FILTER_REJECT" => Value::int(2),
            "FILTER_SKIP" => Value::int(3),
            "FILTER_INTERRUPT" => Value::int(4),
            "whatToShow" => Value::int(0xFFFF_FFFF_i64),
        }

        #[py_name = "acceptNode"]
        fn accept_node(#[positional_only] _self: PyValueRef<'_>, _element: PyValue) -> Result<Value> {
            Ok(Value::int(1))
        }

        #[py_name = "startContainer"]
        fn start_container(#[positional_only] _self: PyValueRef<'_>, _element: PyValue) -> Result<Value> {
            Ok(Value::int(1))
        }
    }

    class DocumentLS {
        constants {
            "async_" => Value::bool_(false),
        }

        fn _get_async(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(false))
        }

        fn _set_async(#[positional_only] _self: PyValueRef<'_>, flag: PyValue) -> Result<Value> {
            if _interp.truthy_value(&flag.0)? {
                return Err(raise_dom_exc(
                    _interp,
                    "NotSupportedErr",
                    "asynchronous document loading is not supported",
                ));
            }
            Ok(Value::none())
        }

        fn abort(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "haven't figured out what this means yet",
            ))
        }

        fn load(#[positional_only] _self: PyValueRef<'_>, _uri: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "haven't written this yet",
            ))
        }

        #[py_name = "loadXML"]
        fn load_xml(#[positional_only] _self: PyValueRef<'_>, _source: PyValue) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "haven't written this yet",
            ))
        }

        #[py_name = "saveXML"]
        fn save_xml(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] snode: OptValueRef<'_>,
        ) -> Result<Value> {
            let target = match snode.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => self_val.0.clone(),
            };
            call_method(_interp, &target, "toxml", &[])
        }
    }

    class DOMImplementationLS {
        constants {
            "MODE_SYNCHRONOUS" => Value::int(1),
            "MODE_ASYNCHRONOUS" => Value::int(2),
        }

        #[py_name = "createDOMBuilder"]
        fn create_dom_builder(
            #[positional_only] _self: PyValueRef<'_>,
            mode: PyIndex,
            schemaType: PyValue,
        ) -> Result<Value> {
            if !schemaType.0.is_none() {
                return Err(raise_dom_exc(
                    _interp,
                    "NotSupportedErr",
                    "schemaType not yet supported",
                ));
            }
            let m = mode.expect_i64(FN_NAME, "mode")?;
            if m == 1 {
                let xb_mod = _interp.load_module("xml.dom.xmlbuilder")?;
                let cls = _interp.get_attr(&xb_mod, "DOMBuilder")?;
                return call_pos(_interp, cls, &[]);
            }
            if m == 2 {
                return Err(raise_dom_exc(
                    _interp,
                    "NotSupportedErr",
                    "asynchronous builders are not supported",
                ));
            }
            Err(PyError::named("ValueError", "unknown value for mode"))
        }

        #[py_name = "createDOMWriter"]
        fn create_dom_writer(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "the writer interface hasn't been written yet!",
            ))
        }

        #[py_name = "createDOMInputSource"]
        fn create_dom_input_source(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let xb_mod = _interp.load_module("xml.dom.xmlbuilder")?;
            let cls = _interp.get_attr(&xb_mod, "DOMInputSource")?;
            call_pos(_interp, cls, &[])
        }
    }

    #[py_name = "makeBuilder"]
    fn make_builder(options: PyValue) -> Result<Value> {
        let eb_mod = _interp.load_module("xml.dom.expatbuilder")?;
        let f = _interp.get_attr(&eb_mod, "makeBuilder")?;
        call_pos(_interp, f, &[options.0])
    }
}
