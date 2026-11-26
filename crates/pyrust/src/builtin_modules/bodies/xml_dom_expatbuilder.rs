// `xml.dom.expatbuilder` module — 100% native-Rust Python 3.12 Expat-backed
// DOM builder for `xml.dom.minidom` (issue #3260).
//
// Provides `parse`, `parseString`, `parseFragment`, `parseFragmentString`,
// `ExpatBuilder`, `ExpatBuilderNS`, `FragmentBuilder`, `FragmentBuilderNS`,
// `Namespaces`, `ParseEscape`, `Rejecter`, `Skipper`, `FilterVisibilityController`,
// `FilterCrutch`, `ElementInfo`, `InternalSubsetExtractor`, and `makeBuilder`.
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/xml/dom/expatbuilder.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyModule, Value, ValueKind};

const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

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

fn eb_cls(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("xml.dom.expatbuilder")?;
    interp.get_attr(&m, name)
}

fn md_cls(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("xml.dom.minidom")?;
    interp.get_attr(&m, name)
}

/// Split an Expat namespace-expanded name (`"uri local prefix"` or `"uri local"` or `"local"`).
fn split_expat_ns(raw: &str) -> (Option<String>, String, Option<String>, String) {
    let parts: Vec<&str> = raw.split(' ').collect();
    match parts.len() {
        1 => (None, parts[0].to_string(), None, parts[0].to_string()),
        2 => (
            Some(parts[0].to_string()),
            parts[1].to_string(),
            None,
            parts[1].to_string(),
        ),
        _ => {
            let qname = format!("{}:{}", parts[2], parts[1]);
            (
                Some(parts[0].to_string()),
                parts[1].to_string(),
                Some(parts[2].to_string()),
                qname,
            )
        }
    }
}

fn opt_str_val(s: Option<String>) -> Value {
    match s {
        Some(v) => Value::string(v),
        None => Value::none(),
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let expat_mod = interp.load_module("xml.parsers.expat")?;
    let md_mod = interp.load_module("xml.dom.minidom")?;
    let xb_mod = interp.load_module("xml.dom.xmlbuilder")?;
    let nf_mod = interp.load_module("xml.dom.NodeFilter")?;
    let dom_mod = interp.load_module("xml.dom")?;
    let the_impl = call_method(interp, &md_mod, "getDOMImplementation", &[])?;
    let nf_cls = interp.get_attr(&nf_mod, "NodeFilter")?;
    let node_cls = interp.get_attr(&dom_mod, "Node")?;

    let mut m = module.borrow_mut();
    m.insert_attr("expat".to_string(), expat_mod);
    m.insert_attr("minidom".to_string(), md_mod);
    m.insert_attr("xmlbuilder".to_string(), xb_mod);
    m.insert_attr("NodeFilter".to_string(), nf_cls);
    m.insert_attr("Node".to_string(), node_cls);
    m.insert_attr("theDOMImplementation".to_string(), the_impl);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.dom"),
        "TEXT_NODE" => Value::int(3),
        "CDATA_SECTION_NODE" => Value::int(4),
        "DOCUMENT_NODE" => Value::int(9),
        "FILTER_ACCEPT" => Value::int(1),
        "FILTER_REJECT" => Value::int(2),
        "FILTER_SKIP" => Value::int(3),
        "FILTER_INTERRUPT" => Value::int(4),
        "EMPTY_NAMESPACE" => Value::none(),
        "EMPTY_PREFIX" => Value::none(),
        "XMLNS_NAMESPACE" => Value::string(XMLNS_NS),
    }

    exception ParseEscape;

    class ElementInfo {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            tagName: PyValue,
            #[default(OptionalArg::Omitted)] model: OptValueRef<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "tagName", tagName.0)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "_model",
                model.as_value().cloned().unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(self_val.0.clone(), "_attr_info", Value::list(Vec::new()))?;
            Ok(Value::none())
        }

        #[py_name = "getAttributeType"]
        fn get_attribute_type(#[positional_only] _self: PyValueRef<'_>, _aname: PyValue) -> Result<Value> {
            let cls = md_cls(_interp, "TypeInfo")?;
            call_pos(_interp, cls, &[Value::none(), Value::none()])
        }

        #[py_name = "getAttributeTypeNS"]
        fn get_attribute_type_ns(
            #[positional_only] _self: PyValueRef<'_>,
            _ns: PyValue,
            _local: PyValue,
        ) -> Result<Value> {
            let cls = md_cls(_interp, "TypeInfo")?;
            call_pos(_interp, cls, &[Value::none(), Value::none()])
        }

        #[py_name = "isElementContent"]
        fn is_element_content(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(false))
        }

        #[py_name = "isEmpty"]
        fn is_empty(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(false))
        }

        #[py_name = "isId"]
        fn is_id(#[positional_only] _self: PyValueRef<'_>, _aname: PyValue) -> Result<Value> {
            Ok(Value::bool_(false))
        }

        #[py_name = "isIdNS"]
        fn is_id_ns(
            #[positional_only] _self: PyValueRef<'_>,
            _ns: PyValue,
            _local: PyValue,
        ) -> Result<Value> {
            Ok(Value::bool_(false))
        }
    }

    class FilterVisibilityController {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, filter: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "filter", filter.0)?;
            Ok(Value::none())
        }

        #[py_name = "startContainer"]
        fn start_container(#[positional_only] self_val: PyValueRef<'_>, node: PyValue) -> Result<Value> {
            let f = _interp.get_attr(self_val.0, "filter")?;
            call_method(_interp, &f, "startContainer", &[node.0])
        }

        #[py_name = "acceptNode"]
        fn accept_node(#[positional_only] self_val: PyValueRef<'_>, node: PyValue) -> Result<Value> {
            let f = _interp.get_attr(self_val.0, "filter")?;
            call_method(_interp, &f, "acceptNode", &[node.0])
        }
    }

    class FilterCrutch {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, builder: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_builder", builder.0)?;
            _interp.assign_attr(self_val.0.clone(), "_level", Value::int(0))?;
            Ok(Value::none())
        }
    }

    class Rejecter("FilterCrutch") {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, builder: PyValue) -> Result<Value> {
            let fc = eb_cls(_interp, "FilterCrutch")?;
            let init = _interp.get_attr(&fc, "__init__")?;
            call_pos(_interp, init, &[self_val.0.clone(), builder.0])?;
            Ok(Value::none())
        }
    }

    class Skipper("FilterCrutch") {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, builder: PyValue) -> Result<Value> {
            let fc = eb_cls(_interp, "FilterCrutch")?;
            let init = _interp.get_attr(&fc, "__init__")?;
            call_pos(_interp, init, &[self_val.0.clone(), builder.0])?;
            Ok(Value::none())
        }
    }

    class ExpatBuilder {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] options: OptValueRef<'_>,
        ) -> Result<Value> {
            let opts = match options.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let xb = _interp.load_module("xml.dom.xmlbuilder")?;
                    let cls = _interp.get_attr(&xb, "Options")?;
                    call_pos(_interp, cls, &[])?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "_options", opts)?;
            _interp.assign_attr(self_val.0.clone(), "_parser", Value::none())?;
            call_method(_interp, self_val.0, "reset", &[])?;
            Ok(Value::none())
        }

        #[py_name = "createParser"]
        fn create_parser(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let expat = _interp.load_module("xml.parsers.expat")?;
            let f = _interp.get_attr(&expat, "ParserCreate")?;
            call_pos(_interp, f, &[])
        }

        #[py_name = "getParser"]
        fn get_parser(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let p = _interp.get_attr(self_val.0, "_parser")?;
            if p.is_none() {
                let np = call_method(_interp, self_val.0, "createParser", &[])?;
                _interp.assign_attr(self_val.0.clone(), "_parser", np.clone())?;
                call_method(_interp, self_val.0, "install", std::slice::from_ref(&np))?;
                return Ok(np);
            }
            Ok(p)
        }

        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let doc_cls = md_cls(_interp, "Document")?;
            let doc = call_pos(_interp, doc_cls, &[])?;
            _interp.assign_attr(self_val.0.clone(), "document", doc.clone())?;
            _interp.assign_attr(self_val.0.clone(), "curNode", doc)?;
            _interp.assign_attr(self_val.0.clone(), "_elem_info", Value::dict(PyDict::default()))?;
            _interp.assign_attr(self_val.0.clone(), "_cdata", Value::bool_(false))?;
            Ok(Value::none())
        }

        fn install(#[positional_only] self_val: PyValueRef<'_>, parser: PyValue) -> Result<Value> {
            for (h_name, m_name) in [
                ("StartDoctypeDeclHandler", "start_doctype_decl_handler"),
                ("EndDoctypeDeclHandler", "end_doctype_decl_handler"),
                ("StartElementHandler", "first_element_handler"),
                ("EndElementHandler", "end_element_handler"),
                ("ProcessingInstructionHandler", "pi_handler"),
                ("CharacterDataHandler", "character_data_handler"),
                ("XmlDeclHandler", "xml_decl_handler"),
                ("EntityDeclHandler", "entity_decl_handler"),
                ("NotationDeclHandler", "notation_decl_handler"),
            ] {
                let h = _interp.get_attr(self_val.0, m_name)?;
                _interp.assign_attr(parser.0.clone(), h_name, h)?;
            }
            let opts = _interp.get_attr(self_val.0, "_options")?;
            let want_comments = _interp
                .get_attr(&opts, "comments")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(true);
            if want_comments {
                let h = _interp.get_attr(self_val.0, "comment_handler")?;
                _interp.assign_attr(parser.0.clone(), "CommentHandler", h)?;
            }
            let want_cdata = _interp
                .get_attr(&opts, "cdata_sections")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(true);
            if want_cdata {
                let h1 = _interp.get_attr(self_val.0, "start_cdata_section_handler")?;
                _interp.assign_attr(parser.0.clone(), "StartCdataSectionHandler", h1)?;
                let h2 = _interp.get_attr(self_val.0, "end_cdata_section_handler")?;
                _interp.assign_attr(parser.0.clone(), "EndCdataSectionHandler", h2)?;
            }
            _interp.assign_attr(parser.0, "buffer_text", Value::bool_(true))?;
            Ok(Value::none())
        }

        #[py_name = "parseFile"]
        fn parse_file(#[positional_only] self_val: PyValueRef<'_>, file: PyValue) -> Result<Value> {
            let parser = call_method(_interp, self_val.0, "getParser", &[])?;
            let read_fn = _interp.get_attr(&file.0, "read")?;
            let parse_fn = _interp.get_attr(&parser, "Parse")?;
            loop {
                let buf = call_pos(_interp, read_fn.clone(), &[Value::int(16384)])?;
                let empty = match buf.kind() {
                    ValueKind::Bytes(b) => b.is_empty(),
                    ValueKind::Str(s) => s.is_empty(),
                    _ => !_interp.truthy_value(&buf)?,
                };
                if empty {
                    call_pos(_interp, parse_fn, &[Value::bytes(Vec::new()), Value::bool_(true)])?;
                    break;
                }
                call_pos(_interp, parse_fn.clone(), &[buf, Value::bool_(false)])?;
            }
            _interp.get_attr(self_val.0, "document")
        }

        #[py_name = "parseString"]
        fn parse_string(#[positional_only] self_val: PyValueRef<'_>, string: PyValue) -> Result<Value> {
            let parser = call_method(_interp, self_val.0, "getParser", &[])?;
            call_method(_interp, &parser, "Parse", &[string.0, Value::bool_(true)])?;
            _interp.get_attr(self_val.0, "document")
        }

        // Event handlers
        fn xml_decl_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            version: PyValue,
            encoding: PyValue,
            standalone: PyValue,
        ) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            _interp.assign_attr(doc.clone(), "version", version.0)?;
            _interp.assign_attr(doc.clone(), "encoding", encoding.0)?;
            let st_val = match standalone.0.as_int() {
                Some(n) if n >= 0 => Value::bool_(n != 0),
                _ => Value::none(),
            };
            _interp.assign_attr(doc, "standalone", st_val)?;
            Ok(Value::none())
        }

        fn start_doctype_decl_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            doctypeName: PyStr<'_>,
            systemId: PyValue,
            publicId: PyValue,
            _has_internal_subset: PyValue,
        ) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            let impl_obj = _interp.get_attr(&doc, "implementation")?;
            let dt = call_method(
                _interp,
                &impl_obj,
                "createDocumentType",
                &[Value::string(doctypeName.0.as_ref()), publicId.0, systemId.0],
            )?;
            _interp.assign_attr(dt.clone(), "ownerDocument", doc.clone())?;
            call_method(_interp, &doc, "appendChild", &[dt])?;
            Ok(Value::none())
        }

        fn end_doctype_decl_handler(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        fn pi_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            target: PyValue,
            data: PyValue,
        ) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            let pi = call_method(_interp, &doc, "createProcessingInstruction", &[target.0, data.0])?;
            let cur = _interp.get_attr(self_val.0, "curNode")?;
            call_method(_interp, &cur, "appendChild", &[pi])?;
            Ok(Value::none())
        }

        fn character_data_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyStr<'_>,
        ) -> Result<Value> {
            let cur = _interp.get_attr(self_val.0, "curNode")?;
            let doc = _interp.get_attr(self_val.0, "document")?;
            let in_cdata = _interp
                .get_attr(self_val.0, "_cdata")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if in_cdata {
                let node = call_method(
                    _interp,
                    &doc,
                    "createCDATASection",
                    &[Value::string(data.0.as_ref())],
                )?;
                call_method(_interp, &cur, "appendChild", &[node])?;
                return Ok(Value::none());
            }
            // Coalesce with trailing Text node if present
            let children = _interp.get_attr(&cur, "childNodes")?;
            let items = _interp.collect_iterable(&children)?;
            if let Some(last) = items.last()
                && _interp
                    .get_attr(last, "nodeType")
                    .ok()
                    .and_then(|v| v.as_int())
                    == Some(3)
            {
                let old = _interp.get_attr(last, "data")?;
                let combined = format!("{}{}", old.as_str().unwrap_or(""), data.0);
                _interp.assign_attr(last.clone(), "data", Value::string(combined))?;
                return Ok(Value::none());
            }
            let node = call_method(
                _interp,
                &doc,
                "createTextNode",
                &[Value::string(data.0.as_ref())],
            )?;
            call_method(_interp, &cur, "appendChild", &[node])?;
            Ok(Value::none())
        }

        fn character_data_handler_cdata(
            #[positional_only] self_val: PyValueRef<'_>,
            data: PyStr<'_>,
        ) -> Result<Value> {
            let cur = _interp.get_attr(self_val.0, "curNode")?;
            let doc = _interp.get_attr(self_val.0, "document")?;
            let node = call_method(
                _interp,
                &doc,
                "createCDATASection",
                &[Value::string(data.0.as_ref())],
            )?;
            call_method(_interp, &cur, "appendChild", &[node])?;
            Ok(Value::none())
        }

        fn start_cdata_section_handler(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_cdata", Value::bool_(true))?;
            Ok(Value::none())
        }

        fn end_cdata_section_handler(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_cdata", Value::bool_(false))?;
            Ok(Value::none())
        }

        fn comment_handler(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            let node = call_method(_interp, &doc, "createComment", &[data.0])?;
            let cur = _interp.get_attr(self_val.0, "curNode")?;
            call_method(_interp, &cur, "appendChild", &[node])?;
            Ok(Value::none())
        }

        fn entity_decl_handler(
            #[positional_only] _self: PyValueRef<'_>,
            _entityName: PyValue,
            _is_param: PyValue,
            _value: PyValue,
            _base: PyValue,
            _sysid: PyValue,
            _pubid: PyValue,
            _notation: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn notation_decl_handler(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
            _base: PyValue,
            _sysid: PyValue,
            _pubid: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn element_decl_handler(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
            _model: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn attlist_decl_handler(
            #[positional_only] _self: PyValueRef<'_>,
            _elem: PyValue,
            _name: PyValue,
            _type: PyValue,
            _default: PyValue,
            _required: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn external_entity_ref_handler(
            #[positional_only] _self: PyValueRef<'_>,
            _context: PyValue,
            _base: PyValue,
            _sysid: PyValue,
            _pubid: PyValue,
        ) -> Result<Value> {
            Ok(Value::int(1))
        }

        fn first_element_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            attributes: PyValue,
        ) -> Result<Value> {
            let p = _interp.get_attr(self_val.0, "_parser")?;
            if !p.is_none() {
                let se = _interp.get_attr(self_val.0, "start_element_handler")?;
                _interp.assign_attr(p, "StartElementHandler", se)?;
            }
            call_method(
                _interp,
                self_val.0,
                "start_element_handler",
                &[name.0, attributes.0],
            )
        }

        fn start_element_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            attributes: PyValue,
        ) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            let el = call_method(_interp, &doc, "createElement", &[Value::string(name.0.as_ref())])?;
            let items = call_method(_interp, &attributes.0, "items", &[])?;
            for pair in _interp.collect_iterable(&items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    call_method(_interp, &el, "setAttribute", &[kv[0].clone(), kv[1].clone()])?;
                }
            }
            let cur = _interp.get_attr(self_val.0, "curNode")?;
            call_method(_interp, &cur, "appendChild", std::slice::from_ref(&el))?;
            _interp.assign_attr(self_val.0.clone(), "curNode", el)?;
            Ok(Value::none())
        }

        fn end_element_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            _name: PyValue,
        ) -> Result<Value> {
            let cur = _interp.get_attr(self_val.0, "curNode")?;
            let parent = _interp.get_attr(&cur, "parentNode")?;
            _interp.assign_attr(self_val.0.clone(), "curNode", parent)?;
            Ok(Value::none())
        }
    }

    class Namespaces {
        fn _initNamespaces(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_ns_ordered_prefixes", Value::list(Vec::new()))?;
            Ok(Value::none())
        }

        #[py_name = "createParser"]
        fn create_parser(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let expat = _interp.load_module("xml.parsers.expat")?;
            let f = _interp.get_attr(&expat, "ParserCreate")?;
            let p = call_kw(_interp, f, &[], &[("namespace_separator", Value::string(" "))])?;
            _interp.assign_attr(p.clone(), "namespace_prefixes", Value::bool_(true))?;
            Ok(p)
        }

        fn install(#[positional_only] self_val: PyValueRef<'_>, parser: PyValue) -> Result<Value> {
            let eb_base = eb_cls(_interp, "ExpatBuilder")?;
            let base_install = _interp.get_attr(&eb_base, "install")?;
            call_pos(_interp, base_install, &[self_val.0.clone(), parser.0.clone()])?;
            let opts = _interp.get_attr(self_val.0, "_options")?;
            let want_ns_decls = _interp
                .get_attr(&opts, "namespace_declarations")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(true);
            if want_ns_decls {
                let sn = _interp.get_attr(self_val.0, "start_namespace_decl_handler")?;
                _interp.assign_attr(parser.0, "StartNamespaceDeclHandler", sn)?;
            }
            Ok(Value::none())
        }

        fn start_namespace_decl_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            prefix: PyValue,
            uri: PyValue,
        ) -> Result<Value> {
            let list = _interp.get_attr(self_val.0, "_ns_ordered_prefixes")?;
            call_method(_interp, &list, "append", &[Value::tuple(vec![prefix.0, uri.0])])?;
            Ok(Value::none())
        }

        fn start_element_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            attributes: PyValue,
        ) -> Result<Value> {
            let (ns_uri, local, pfx, qname) = split_expat_ns(&name.0);
            let doc = _interp.get_attr(self_val.0, "document")?;
            let el_cls = md_cls(_interp, "Element")?;
            let el = call_pos(
                _interp,
                el_cls,
                &[
                    Value::string(qname),
                    opt_str_val(ns_uri),
                    opt_str_val(pfx),
                    Value::string(local),
                ],
            )?;
            _interp.assign_attr(el.clone(), "ownerDocument", doc.clone())?;

            // Emit namespace declarations as xmlns / xmlns:* attributes
            let ns_list = _interp.get_attr(self_val.0, "_ns_ordered_prefixes")?;
            let decls = _interp.collect_iterable(&ns_list)?;
            if !decls.is_empty() {
                call_method(_interp, &ns_list, "clear", &[])?;
                for pair in decls {
                    let kv = _interp.collect_iterable(&pair)?;
                    if kv.len() == 2 {
                        let uri_s = kv[1].as_str().unwrap_or("");
                        if let Some(p) = kv[0].as_str() {
                            call_method(
                                _interp,
                                &el,
                                "setAttributeNS",
                                &[
                                    Value::string(XMLNS_NS),
                                    Value::string(format!("xmlns:{p}")),
                                    Value::string(uri_s),
                                ],
                            )?;
                        } else {
                            call_method(
                                _interp,
                                &el,
                                "setAttributeNS",
                                &[
                                    Value::string(XMLNS_NS),
                                    Value::string("xmlns"),
                                    Value::string(uri_s),
                                ],
                            )?;
                        }
                    }
                }
            }

            // Regular attributes
            let items = call_method(_interp, &attributes.0, "items", &[])?;
            for pair in _interp.collect_iterable(&items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    let raw_k = kv[0].as_str().unwrap_or("");
                    let (a_ns, _a_local, _a_pfx, a_qname) = split_expat_ns(raw_k);
                    call_method(
                        _interp,
                        &el,
                        "setAttributeNS",
                        &[opt_str_val(a_ns), Value::string(a_qname), kv[1].clone()],
                    )?;
                }
            }

            let cur = _interp.get_attr(self_val.0, "curNode")?;
            call_method(_interp, &cur, "appendChild", std::slice::from_ref(&el))?;
            _interp.assign_attr(self_val.0.clone(), "curNode", el)?;
            Ok(Value::none())
        }
    }

    class ExpatBuilderNS("Namespaces", "ExpatBuilder") {
        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let eb_base = eb_cls(_interp, "ExpatBuilder")?;
            let base_reset = _interp.get_attr(&eb_base, "reset")?;
            call_pos(_interp, base_reset, std::slice::from_ref(self_val.0))?;
            call_method(_interp, self_val.0, "_initNamespaces", &[])?;
            Ok(Value::none())
        }
    }

    class FragmentBuilder("ExpatBuilder") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            context: PyValue,
            #[default(OptionalArg::Omitted)] options: OptValueRef<'_>,
        ) -> Result<Value> {
            let nt = _interp
                .get_attr(&context.0, "nodeType")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(0);
            let doc = if nt == 9 {
                context.0.clone()
            } else {
                _interp.get_attr(&context.0, "ownerDocument")?
            };
            _interp.assign_attr(self_val.0.clone(), "originalDocument", doc)?;
            _interp.assign_attr(self_val.0.clone(), "context", context.0)?;
            let eb_base = eb_cls(_interp, "ExpatBuilder")?;
            let base_init = _interp.get_attr(&eb_base, "__init__")?;
            let opt_v = options.as_value().cloned().unwrap_or_else(Value::none);
            call_pos(_interp, base_init, &[self_val.0.clone(), opt_v])?;
            Ok(Value::none())
        }

        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let eb_base = eb_cls(_interp, "ExpatBuilder")?;
            let base_reset = _interp.get_attr(&eb_base, "reset")?;
            call_pos(_interp, base_reset, std::slice::from_ref(self_val.0))?;
            _interp.assign_attr(self_val.0.clone(), "fragment", Value::none())?;
            Ok(Value::none())
        }

        #[py_name = "parseString"]
        fn parse_string(
            #[positional_only] self_val: PyValueRef<'_>,
            string: PyValue,
        ) -> Result<Value> {
            let raw_s = match string.0.kind() {
                ValueKind::Str(s) => s.to_string(),
                ValueKind::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                _ => _interp.render_value_as_str(&string.0)?,
            };
            let wrapped = format!("<_pyrust_fragment_wrapper_>{raw_s}</_pyrust_fragment_wrapper_>");
            let eb_base = eb_cls(_interp, "ExpatBuilder")?;
            let base_parse = _interp.get_attr(&eb_base, "parseString")?;
            let doc = call_pos(_interp, base_parse, &[self_val.0.clone(), Value::string(wrapped)])?;
            let orig_doc = _interp.get_attr(self_val.0, "originalDocument")?;
            let frag = call_method(_interp, &orig_doc, "createDocumentFragment", &[])?;
            let root = _interp.get_attr(&doc, "documentElement")?;
            if !root.is_none() {
                let children = _interp.get_attr(&root, "childNodes")?;
                let snapshot = _interp.collect_iterable(&children)?;
                for child in snapshot {
                    let imported = call_method(
                        _interp,
                        &orig_doc,
                        "importNode",
                        &[child, Value::bool_(true)],
                    )?;
                    call_method(_interp, &frag, "appendChild", &[imported])?;
                }
            }
            _interp.assign_attr(self_val.0.clone(), "fragment", frag.clone())?;
            Ok(frag)
        }

        #[py_name = "parseFile"]
        fn parse_file(#[positional_only] self_val: PyValueRef<'_>, file: PyValue) -> Result<Value> {
            let data = call_method(_interp, &file.0, "read", &[])?;
            call_method(_interp, self_val.0, "parseString", &[data])
        }
    }

    class FragmentBuilderNS("Namespaces", "FragmentBuilder") {
        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let fb_base = eb_cls(_interp, "FragmentBuilder")?;
            let base_reset = _interp.get_attr(&fb_base, "reset")?;
            call_pos(_interp, base_reset, std::slice::from_ref(self_val.0))?;
            call_method(_interp, self_val.0, "_initNamespaces", &[])?;
            Ok(Value::none())
        }
    }

    class InternalSubsetExtractor("ExpatBuilder") {}

    fn parse(
        file: PyValue,
        #[default(true.into())] namespaces: PyBool,
    ) -> Result<Value> {
        let builder_cls = if namespaces.0 {
            eb_cls(_interp, "ExpatBuilderNS")?
        } else {
            eb_cls(_interp, "ExpatBuilder")?
        };
        let builder = call_pos(_interp, builder_cls, &[])?;
        if let Some(path_s) = file.0.as_str() {
            let fp = call_pos(
                _interp,
                Value::builtin_function("open"),
                &[Value::string(path_s), Value::string("rb")],
            )?;
            let res = call_method(_interp, &builder, "parseFile", std::slice::from_ref(&fp));
            let _ = call_method(_interp, &fp, "close", &[]);
            res
        } else {
            call_method(_interp, &builder, "parseFile", &[file.0])
        }
    }

    #[py_name = "parseString"]
    fn parse_string(
        string: PyValue,
        #[default(true.into())] namespaces: PyBool,
    ) -> Result<Value> {
        let builder_cls = if namespaces.0 {
            eb_cls(_interp, "ExpatBuilderNS")?
        } else {
            eb_cls(_interp, "ExpatBuilder")?
        };
        let builder = call_pos(_interp, builder_cls, &[])?;
        call_method(_interp, &builder, "parseString", &[string.0])
    }

    #[py_name = "parseFragment"]
    fn parse_fragment(
        file: PyValue,
        context: PyValue,
        #[default(true.into())] namespaces: PyBool,
    ) -> Result<Value> {
        let builder_cls = if namespaces.0 {
            eb_cls(_interp, "FragmentBuilderNS")?
        } else {
            eb_cls(_interp, "FragmentBuilder")?
        };
        let builder = call_pos(_interp, builder_cls, &[context.0])?;
        if let Some(path_s) = file.0.as_str() {
            let fp = call_pos(
                _interp,
                Value::builtin_function("open"),
                &[Value::string(path_s), Value::string("rb")],
            )?;
            let res = call_method(_interp, &builder, "parseFile", std::slice::from_ref(&fp));
            let _ = call_method(_interp, &fp, "close", &[]);
            res
        } else {
            call_method(_interp, &builder, "parseFile", &[file.0])
        }
    }

    #[py_name = "parseFragmentString"]
    fn parse_fragment_string(
        string: PyValue,
        context: PyValue,
        #[default(true.into())] namespaces: PyBool,
    ) -> Result<Value> {
        let builder_cls = if namespaces.0 {
            eb_cls(_interp, "FragmentBuilderNS")?
        } else {
            eb_cls(_interp, "FragmentBuilder")?
        };
        let builder = call_pos(_interp, builder_cls, &[context.0])?;
        call_method(_interp, &builder, "parseString", &[string.0])
    }

    #[py_name = "makeBuilder"]
    fn make_builder(options: PyValue) -> Result<Value> {
        let ns = _interp
            .get_attr(&options.0, "namespaces")
            .ok()
            .and_then(|v| _interp.truthy_value(&v).ok())
            .unwrap_or(true);
        let cls = if ns {
            eb_cls(_interp, "ExpatBuilderNS")?
        } else {
            eb_cls(_interp, "ExpatBuilder")?
        };
        call_pos(_interp, cls, &[options.0])
    }
}
