// `xml.dom.minidom` module — 100% native-Rust Python 3.12 W3C DOM Level 2/3
// minimal implementation (issue #3260).
//
// Reference: <https://docs.python.org/3/library/xml.dom.minidom.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, values_are_identical};
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

fn minidom_cls(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("xml.dom.minidom")?;
    interp.get_attr(&m, name)
}

fn new_node_list(interp: &mut Interpreter, items: Vec<Value>) -> Result<Value> {
    let mc = interp.load_module("xml.dom.minicompat")?;
    let cls = interp.get_attr(&mc, "NodeList")?;
    let nl = call_pos(interp, cls, &[])?;
    if !items.is_empty() {
        call_method(interp, &nl, "extend", &[Value::list(items)])?;
    }
    Ok(nl)
}

fn new_empty_node_list(interp: &mut Interpreter) -> Result<Value> {
    let mc = interp.load_module("xml.dom.minicompat")?;
    let cls = interp.get_attr(&mc, "EmptyNodeList")?;
    call_pos(interp, cls, &[])
}

fn split_qname_parts(qname: &str) -> (Option<String>, String) {
    match qname.find(':') {
        Some(idx) => (Some(qname[..idx].to_string()), qname[idx + 1..].to_string()),
        None => (None, qname.to_string()),
    }
}

fn escape_xml_data(s: &str, is_attr: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if is_attr => out.push_str("&quot;"),
            '\n' if is_attr => out.push_str("&#10;"),
            '\r' if is_attr => out.push_str("&#13;"),
            '\t' if is_attr => out.push_str("&#9;"),
            _ => out.push(c),
        }
    }
    out
}

fn write_str(interp: &mut Interpreter, writer: &Value, s: &str) -> Result<()> {
    call_method(interp, writer, "write", &[Value::string(s)])?;
    Ok(())
}

fn get_node_type(interp: &mut Interpreter, node: &Value) -> i64 {
    interp
        .get_attr(node, "nodeType")
        .ok()
        .and_then(|v| v.as_int())
        .unwrap_or(0)
}

fn collect_elements_by_tag_name(
    interp: &mut Interpreter,
    parent: &Value,
    name: &str,
    out: &mut Vec<Value>,
) -> Result<()> {
    let children = interp.get_attr(parent, "childNodes")?;
    for child in interp.collect_iterable(&children)? {
        if get_node_type(interp, &child) == 1 {
            let tag = interp
                .get_attr(&child, "tagName")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            if name == "*" || tag == name {
                out.push(child.clone());
            }
            collect_elements_by_tag_name(interp, &child, name, out)?;
        }
    }
    Ok(())
}

fn collect_elements_by_tag_name_ns(
    interp: &mut Interpreter,
    parent: &Value,
    ns_uri: Option<&str>,
    local_name: &str,
    out: &mut Vec<Value>,
) -> Result<()> {
    let children = interp.get_attr(parent, "childNodes")?;
    for child in interp.collect_iterable(&children)? {
        if get_node_type(interp, &child) == 1 {
            let c_ns = interp
                .get_attr(&child, "namespaceURI")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string));
            let c_local = interp
                .get_attr(&child, "localName")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            let ns_match = ns_uri == Some("*") || c_ns.as_deref() == ns_uri;
            let local_match = local_name == "*" || c_local == local_name;
            if ns_match && local_match {
                out.push(child.clone());
            }
            collect_elements_by_tag_name_ns(interp, &child, ns_uri, local_name, out)?;
        }
    }
    Ok(())
}

fn set_siblings_for_insert(
    interp: &mut Interpreter,
    parent: &Value,
    children: &Value,
    idx: usize,
    node: &Value,
) -> Result<()> {
    let items = interp.collect_iterable(children)?;
    let prev = if idx > 0 {
        items.get(idx - 1).cloned()
    } else {
        None
    };
    let next = items.get(idx + 1).cloned();
    if let Some(ref p) = prev {
        interp.assign_attr(p.clone(), "nextSibling", node.clone())?;
        interp.assign_attr(node.clone(), "previousSibling", p.clone())?;
    } else {
        interp.assign_attr(node.clone(), "previousSibling", Value::none())?;
    }
    if let Some(ref n) = next {
        interp.assign_attr(n.clone(), "previousSibling", node.clone())?;
        interp.assign_attr(node.clone(), "nextSibling", n.clone())?;
    } else {
        interp.assign_attr(node.clone(), "nextSibling", Value::none())?;
    }
    interp.assign_attr(node.clone(), "parentNode", parent.clone())?;
    Ok(())
}

fn append_child_inner(interp: &mut Interpreter, parent: &Value, node: &Value) -> Result<Value> {
    let nt = get_node_type(interp, node);
    if nt == 11 {
        // DocumentFragment: move all children
        let frag_children = interp.get_attr(node, "childNodes")?;
        let snapshot = interp.collect_iterable(&frag_children)?;
        for child in snapshot {
            append_child_inner(interp, parent, &child)?;
        }
        return Ok(node.clone());
    }
    // Detach from old parent if attached
    if let Ok(old_parent) = interp.get_attr(node, "parentNode")
        && !old_parent.is_none()
    {
        let _ = call_method(
            interp,
            &old_parent,
            "removeChild",
            std::slice::from_ref(node),
        );
    }
    let children = interp.get_attr(parent, "childNodes")?;
    let cur_len = call_pos(
        interp,
        Value::builtin_function("len"),
        std::slice::from_ref(&children),
    )?
    .as_int()
    .unwrap_or(0) as usize;
    call_method(interp, &children, "append", std::slice::from_ref(node))?;
    set_siblings_for_insert(interp, parent, &children, cur_len, node)?;
    Ok(node.clone())
}

fn clone_node_impl(
    interp: &mut Interpreter,
    node: &Value,
    deep: bool,
    new_owner_doc: &Value,
) -> Result<Value> {
    let nt = get_node_type(interp, node);
    let clone = match nt {
        1 => {
            // Element
            let tag = interp.get_attr(node, "tagName")?;
            let ns = interp.get_attr(node, "namespaceURI")?;
            let pfx = interp.get_attr(node, "prefix")?;
            let local = interp.get_attr(node, "localName")?;
            let el_cls = minidom_cls(interp, "Element")?;
            let el = call_pos(interp, el_cls, &[tag, ns, pfx, local])?;
            interp.assign_attr(el.clone(), "ownerDocument", new_owner_doc.clone())?;
            // Clone attributes
            let attrs = interp.get_attr(node, "_attrs")?;
            if !attrs.is_none() {
                let vals = call_method(interp, &attrs, "values", &[])?;
                for attr in interp.collect_iterable(&vals)? {
                    let ac = clone_node_impl(interp, &attr, true, new_owner_doc)?;
                    call_method(interp, &el, "setAttributeNode", &[ac])?;
                }
            }
            if deep {
                let children = interp.get_attr(node, "childNodes")?;
                for child in interp.collect_iterable(&children)? {
                    let cc = clone_node_impl(interp, &child, true, new_owner_doc)?;
                    append_child_inner(interp, &el, &cc)?;
                }
            }
            el
        }
        2 => {
            // Attr
            let name = interp.get_attr(node, "name")?;
            let ns = interp.get_attr(node, "namespaceURI")?;
            let local = interp.get_attr(node, "localName")?;
            let pfx = interp.get_attr(node, "prefix")?;
            let val = interp.get_attr(node, "value")?;
            let attr_cls = minidom_cls(interp, "Attr")?;
            let a = call_pos(interp, attr_cls, &[name, ns, local, pfx])?;
            interp.assign_attr(a.clone(), "value", val)?;
            interp.assign_attr(a.clone(), "ownerDocument", new_owner_doc.clone())?;
            a
        }
        3 => {
            // Text
            let data = interp.get_attr(node, "data")?;
            let cls = minidom_cls(interp, "Text")?;
            let t = call_pos(interp, cls, &[])?;
            interp.assign_attr(t.clone(), "data", data)?;
            interp.assign_attr(t.clone(), "ownerDocument", new_owner_doc.clone())?;
            t
        }
        4 => {
            // CDATASection
            let data = interp.get_attr(node, "data")?;
            let cls = minidom_cls(interp, "CDATASection")?;
            let t = call_pos(interp, cls, &[])?;
            interp.assign_attr(t.clone(), "data", data)?;
            interp.assign_attr(t.clone(), "ownerDocument", new_owner_doc.clone())?;
            t
        }
        7 => {
            // ProcessingInstruction
            let target = interp.get_attr(node, "target")?;
            let data = interp.get_attr(node, "data")?;
            let cls = minidom_cls(interp, "ProcessingInstruction")?;
            let p = call_pos(interp, cls, &[target, data])?;
            interp.assign_attr(p.clone(), "ownerDocument", new_owner_doc.clone())?;
            p
        }
        8 => {
            // Comment
            let data = interp.get_attr(node, "data")?;
            let cls = minidom_cls(interp, "Comment")?;
            let c = call_pos(interp, cls, &[data])?;
            interp.assign_attr(c.clone(), "ownerDocument", new_owner_doc.clone())?;
            c
        }
        9 => {
            // Document
            let cls = minidom_cls(interp, "Document")?;
            let doc = call_pos(interp, cls, &[])?;
            if deep {
                let children = interp.get_attr(node, "childNodes")?;
                for child in interp.collect_iterable(&children)? {
                    let cc = clone_node_impl(interp, &child, true, &doc)?;
                    append_child_inner(interp, &doc, &cc)?;
                }
            }
            doc
        }
        10 => {
            // DocumentType
            let qn = interp.get_attr(node, "name")?;
            let cls = minidom_cls(interp, "DocumentType")?;
            let dt = call_pos(interp, cls, &[qn])?;
            let pub_id = interp.get_attr(node, "publicId")?;
            interp.assign_attr(dt.clone(), "publicId", pub_id)?;
            let sys_id = interp.get_attr(node, "systemId")?;
            interp.assign_attr(dt.clone(), "systemId", sys_id)?;
            let int_sub = interp.get_attr(node, "internalSubset")?;
            interp.assign_attr(dt.clone(), "internalSubset", int_sub)?;
            interp.assign_attr(dt.clone(), "ownerDocument", new_owner_doc.clone())?;
            dt
        }
        11 => {
            // DocumentFragment
            let cls = minidom_cls(interp, "DocumentFragment")?;
            let df = call_pos(interp, cls, &[])?;
            interp.assign_attr(df.clone(), "ownerDocument", new_owner_doc.clone())?;
            if deep {
                let children = interp.get_attr(node, "childNodes")?;
                for child in interp.collect_iterable(&children)? {
                    let cc = clone_node_impl(interp, &child, true, new_owner_doc)?;
                    append_child_inner(interp, &df, &cc)?;
                }
            }
            df
        }
        _ => return Ok(Value::none()),
    };
    Ok(clone)
}

fn ensure_element_attrs(interp: &mut Interpreter, el: &Value) -> Result<(Value, Value)> {
    let a = interp.get_attr(el, "_attrs")?;
    let a_ns = interp.get_attr(el, "_attrsNS")?;
    if a.is_none() {
        let d1 = Value::dict(PyDict::default());
        let d2 = Value::dict(PyDict::default());
        interp.assign_attr(el.clone(), "_attrs", d1.clone())?;
        interp.assign_attr(el.clone(), "_attrsNS", d2.clone())?;
        Ok((d1, d2))
    } else {
        Ok((a, a_ns))
    }
}

fn toxml_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    encoding: Option<&str>,
    standalone: Option<bool>,
    indent: &str,
    addindent: &str,
    newl: &str,
) -> Result<Value> {
    let io_mod = interp.load_module("io")?;
    let sio_cls = interp.get_attr(&io_mod, "StringIO")?;
    let writer = call_pos(interp, sio_cls, &[])?;

    let nt = get_node_type(interp, self_val);
    if nt == 9 {
        let enc_val = match encoding {
            Some(e) => Value::string(e),
            None => Value::none(),
        };
        let st_val = match standalone {
            Some(b) => Value::bool_(b),
            None => Value::none(),
        };
        call_method(
            interp,
            self_val,
            "writexml",
            &[
                writer.clone(),
                Value::string(indent),
                Value::string(addindent),
                Value::string(newl),
                enc_val,
                st_val,
            ],
        )?;
    } else {
        call_method(
            interp,
            self_val,
            "writexml",
            &[
                writer.clone(),
                Value::string(indent),
                Value::string(addindent),
                Value::string(newl),
            ],
        )?;
    }

    let text_val = call_method(interp, &writer, "getvalue", &[])?;
    if let Some(enc) = encoding {
        call_method(
            interp,
            &text_val,
            "encode",
            &[Value::string(enc), Value::string("xmlcharrefreplace")],
        )
    } else {
        Ok(text_val)
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let dom_mod = interp.load_module("xml.dom")?;
    let mc_mod = interp.load_module("xml.dom.minicompat")?;
    let xb_mod = interp.load_module("xml.dom.xmlbuilder")?;
    let dr_mod = interp.load_module("xml.dom.domreg")?;

    let dom_node_cls = interp.get_attr(&dom_mod, "Node")?;
    let doc_ls_cls = interp.get_attr(&xb_mod, "DocumentLS")?;
    let impl_ls_cls = interp.get_attr(&xb_mod, "DOMImplementationLS")?;

    let (node_val, doc_val, impl_val) = {
        let m = module.borrow();
        (
            m.get_attr_value("Node"),
            m.get_attr_value("Document"),
            m.get_attr_value("DOMImplementation"),
        )
    };
    if let (Some(nv), ValueKind::PyClass(bc)) = (node_val.as_ref(), dom_node_cls.kind())
        && let ValueKind::PyClass(nc) = nv.kind()
    {
        link_class_bases(nc, vec![Rc::clone(bc)]);
    }
    if let (Some(dv), Some(nv), ValueKind::PyClass(dls)) =
        (doc_val.as_ref(), node_val.as_ref(), doc_ls_cls.kind())
        && let (ValueKind::PyClass(dc), ValueKind::PyClass(nc)) = (dv.kind(), nv.kind())
    {
        link_class_bases(dc, vec![Rc::clone(nc), Rc::clone(dls)]);
    }
    if let (Some(iv), ValueKind::PyClass(ils)) = (impl_val.as_ref(), impl_ls_cls.kind())
        && let ValueKind::PyClass(ic) = iv.kind()
    {
        link_class_bases(ic, vec![Rc::clone(ils)]);
    }

    let mut m = module.borrow_mut();
    m.insert_attr("DocumentLS".to_string(), doc_ls_cls);
    m.insert_attr("DOMImplementationLS".to_string(), impl_ls_cls);
    m.insert_attr("domreg".to_string(), dr_mod);
    for name in ["NodeList", "EmptyNodeList", "StringTypes", "defproperty"] {
        if let Ok(v) = interp.get_attr(&mc_mod, name) {
            m.insert_attr(name.to_string(), v);
        }
    }
    if let Some(nnm) = m.get_attr_value("NamedNodeMap") {
        m.insert_attr("AttributeList".to_string(), nnm);
    }
    if let Ok(io_m) = interp.load_module("io") {
        m.insert_attr("io".to_string(), io_m);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.dom"),
        "EMPTY_NAMESPACE" => Value::none(),
        "EMPTY_PREFIX" => Value::none(),
        "XMLNS_NAMESPACE" => Value::string(XMLNS_NS),
    }

    class TypeInfo {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            namespace: PyValue,
            name: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "namespace", namespace.0)?;
            _interp.assign_attr(self_val.0.clone(), "name", name.0)?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ns = _interp.get_attr(self_val.0, "namespace")?;
            let name = _interp.get_attr(self_val.0, "name")?;
            let name_r = crate::interpreter::render_value_repr(_interp, &name)?;
            if ns.is_none() {
                Ok(Value::string(format!("<TypeInfo {name_r}>")))
            } else {
                let ns_r = crate::interpreter::render_value_repr(_interp, &ns)?;
                Ok(Value::string(format!("<TypeInfo {name_r} (from {ns_r})>")))
            }
        }
    }

    class ElementInfo {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "tagName", name.0)?;
            Ok(Value::none())
        }

        #[py_name = "getAttributeType"]
        fn get_attribute_type(#[positional_only] _self: PyValueRef<'_>, _aname: PyValue) -> Result<Value> {
            let cls = minidom_cls(_interp, "TypeInfo")?;
            call_pos(_interp, cls, &[Value::none(), Value::none()])
        }

        #[py_name = "getAttributeTypeNS"]
        fn get_attribute_type_ns(
            #[positional_only] _self: PyValueRef<'_>,
            _ns: PyValue,
            _local: PyValue,
        ) -> Result<Value> {
            let cls = minidom_cls(_interp, "TypeInfo")?;
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

    class Node {
        constants {
            "ELEMENT_NODE" => Value::int(1),
            "ATTRIBUTE_NODE" => Value::int(2),
            "TEXT_NODE" => Value::int(3),
            "CDATA_SECTION_NODE" => Value::int(4),
            "ENTITY_REFERENCE_NODE" => Value::int(5),
            "ENTITY_NODE" => Value::int(6),
            "PROCESSING_INSTRUCTION_NODE" => Value::int(7),
            "COMMENT_NODE" => Value::int(8),
            "DOCUMENT_NODE" => Value::int(9),
            "DOCUMENT_TYPE_NODE" => Value::int(10),
            "DOCUMENT_FRAGMENT_NODE" => Value::int(11),
            "NOTATION_NODE" => Value::int(12),
            "namespaceURI" => Value::none(),
            "parentNode" => Value::none(),
            "ownerDocument" => Value::none(),
            "nextSibling" => Value::none(),
            "previousSibling" => Value::none(),
            "prefix" => Value::none(),
            "attributes" => Value::none(),
        }

        fn __bool__(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(true))
        }

        fn toxml(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] standalone: OptValueRef<'_>,
        ) -> Result<Value> {
            let enc = encoding.as_value().and_then(|v| v.as_str());
            let st = match standalone.as_value() {
                Some(v) if !v.is_none() => Some(_interp.truthy_value(v)?),
                _ => None,
            };
            toxml_impl(_interp, self_val.0, enc, st, "", "", "")
        }

        fn toprettyxml(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] indent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] newl: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] standalone: OptValueRef<'_>,
        ) -> Result<Value> {
            let ind = indent.as_value().and_then(|v| v.as_str()).unwrap_or("\t");
            let nl = newl.as_value().and_then(|v| v.as_str()).unwrap_or("\n");
            let enc = encoding.as_value().and_then(|v| v.as_str());
            let st = match standalone.as_value() {
                Some(v) if !v.is_none() => Some(_interp.truthy_value(v)?),
                _ => None,
            };
            toxml_impl(_interp, self_val.0, enc, st, "", ind, nl)
        }

        #[py_name = "hasChildNodes"]
        fn has_child_nodes(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ch = _interp.get_attr(self_val.0, "childNodes")?;
            let n = call_pos(_interp, Value::builtin_function("len"), &[ch])?
                .as_int()
                .unwrap_or(0);
            Ok(Value::bool_(n > 0))
        }

        #[getter("firstChild")]
        fn first_child(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ch = _interp.get_attr(self_val.0, "childNodes")?;
            let items = _interp.collect_iterable(&ch)?;
            Ok(items.into_iter().next().unwrap_or_else(Value::none))
        }

        #[getter("lastChild")]
        fn last_child(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ch = _interp.get_attr(self_val.0, "childNodes")?;
            let items = _interp.collect_iterable(&ch)?;
            Ok(items.into_iter().last().unwrap_or_else(Value::none))
        }

        #[getter("localName")]
        fn local_name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            if let Ok(v) = _interp.get_attr(self_val.0, "_localName") {
                return Ok(v);
            }
            Ok(Value::none())
        }

        #[py_name = "insertBefore"]
        fn insert_before(
            #[positional_only] self_val: PyValueRef<'_>,
            newChild: PyValue,
            refChild: PyValue,
        ) -> Result<Value> {
            if refChild.0.is_none() {
                return append_child_inner(_interp, self_val.0, &newChild.0);
            }
            let children = _interp.get_attr(self_val.0, "childNodes")?;
            let items = _interp.collect_iterable(&children)?;
            let pos = items
                .iter()
                .position(|c| values_are_identical(c, &refChild.0))
                .ok_or_else(|| raise_dom_exc(_interp, "NotFoundErr", "refChild not found"))?;
            if get_node_type(_interp, &newChild.0) == 11 {
                let frag_ch = _interp.get_attr(&newChild.0, "childNodes")?;
                let snapshot = _interp.collect_iterable(&frag_ch)?;
                for child in snapshot {
                    call_method(
                        _interp,
                        self_val.0,
                        "insertBefore",
                        &[child, refChild.0.clone()],
                    )?;
                }
                return Ok(newChild.0);
            }
            if let Ok(old_p) = _interp.get_attr(&newChild.0, "parentNode")
                && !old_p.is_none()
            {
                let _ = call_method(
                    _interp,
                    &old_p,
                    "removeChild",
                    std::slice::from_ref(&newChild.0),
                );
            }
            // Re-find refChild position in case newChild was in same parent
            let items2 = _interp.collect_iterable(&children)?;
            let pos2 = items2
                .iter()
                .position(|c| values_are_identical(c, &refChild.0))
                .unwrap_or(pos);
            call_method(
                _interp,
                &children,
                "insert",
                &[Value::int(pos2 as i64), newChild.0.clone()],
            )?;
            set_siblings_for_insert(_interp, self_val.0, &children, pos2, &newChild.0)?;
            Ok(newChild.0)
        }

        #[py_name = "appendChild"]
        fn append_child(
            #[positional_only] self_val: PyValueRef<'_>,
            node: PyValue,
        ) -> Result<Value> {
            append_child_inner(_interp, self_val.0, &node.0)
        }

        #[py_name = "replaceChild"]
        fn replace_child(
            #[positional_only] self_val: PyValueRef<'_>,
            newChild: PyValue,
            oldChild: PyValue,
        ) -> Result<Value> {
            if values_are_identical(&newChild.0, &oldChild.0) {
                return Ok(oldChild.0);
            }
            call_method(
                _interp,
                self_val.0,
                "insertBefore",
                &[newChild.0, oldChild.0.clone()],
            )?;
            call_method(
                _interp,
                self_val.0,
                "removeChild",
                std::slice::from_ref(&oldChild.0),
            )?;
            Ok(oldChild.0)
        }

        #[py_name = "removeChild"]
        fn remove_child(
            #[positional_only] self_val: PyValueRef<'_>,
            oldChild: PyValue,
        ) -> Result<Value> {
            let children = _interp.get_attr(self_val.0, "childNodes")?;
            let items = _interp.collect_iterable(&children)?;
            let pos = items
                .iter()
                .position(|c| values_are_identical(c, &oldChild.0))
                .ok_or_else(|| raise_dom_exc(_interp, "NotFoundErr", "oldChild not found"))?;
            let prev = if pos > 0 { items.get(pos - 1).cloned() } else { None };
            let next = items.get(pos + 1).cloned();
            call_method(_interp, &children, "pop", &[Value::int(pos as i64)])?;
            if let Some(ref p) = prev {
                _interp.assign_attr(
                    p.clone(),
                    "nextSibling",
                    next.clone().unwrap_or_else(Value::none),
                )?;
            }
            if let Some(ref n) = next {
                _interp.assign_attr(
                    n.clone(),
                    "previousSibling",
                    prev.clone().unwrap_or_else(Value::none),
                )?;
            }
            _interp.assign_attr(oldChild.0.clone(), "parentNode", Value::none())?;
            _interp.assign_attr(oldChild.0.clone(), "previousSibling", Value::none())?;
            _interp.assign_attr(oldChild.0.clone(), "nextSibling", Value::none())?;
            Ok(oldChild.0)
        }

        fn normalize(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let children = _interp.get_attr(self_val.0, "childNodes")?;
            let items = _interp.collect_iterable(&children)?;
            let mut merged: Vec<Value> = Vec::new();
            for child in items {
                let nt = get_node_type(_interp, &child);
                if nt == 3 {
                    let data = _interp
                        .get_attr(&child, "data")
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string))
                        .unwrap_or_default();
                    if data.is_empty() {
                        let _ = _interp.assign_attr(child.clone(), "parentNode", Value::none());
                        continue;
                    }
                    if let Some(prev) = merged.last()
                        && get_node_type(_interp, prev) == 3
                    {
                        let pdata = _interp
                            .get_attr(prev, "data")
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_string))
                            .unwrap_or_default();
                        _interp.assign_attr(
                            prev.clone(),
                            "data",
                            Value::string(format!("{pdata}{data}")),
                        )?;
                        let _ = _interp.assign_attr(child.clone(), "parentNode", Value::none());
                        continue;
                    }
                    merged.push(child);
                } else {
                    if let Ok(norm_fn) = _interp.get_attr(&child, "normalize") {
                        let _ = call_pos(_interp, norm_fn, &[]);
                    }
                    merged.push(child);
                }
            }
            call_method(_interp, &children, "clear", &[])?;
            for (i, node) in merged.iter().enumerate() {
                call_method(_interp, &children, "append", std::slice::from_ref(node))?;
                let prev = if i > 0 {
                    merged[i - 1].clone()
                } else {
                    Value::none()
                };
                let next = merged.get(i + 1).cloned().unwrap_or_else(Value::none);
                _interp.assign_attr(node.clone(), "previousSibling", prev)?;
                _interp.assign_attr(node.clone(), "nextSibling", next)?;
            }
            Ok(Value::none())
        }

        #[py_name = "cloneNode"]
        fn clone_node(#[positional_only] self_val: PyValueRef<'_>, deep: PyBool) -> Result<Value> {
            let owner = _interp
                .get_attr(self_val.0, "ownerDocument")
                .unwrap_or_else(|_| Value::none());
            clone_node_impl(_interp, self_val.0, deep.0, &owner)
        }

        #[py_name = "isSupported"]
        fn is_supported(
            #[positional_only] self_val: PyValueRef<'_>,
            feature: PyValue,
            version: PyValue,
        ) -> Result<Value> {
            let owner = _interp.get_attr(self_val.0, "ownerDocument")?;
            let impl_obj = if !owner.is_none() {
                _interp.get_attr(&owner, "implementation")?
            } else {
                let m = _interp.load_module("xml.dom.minidom")?;
                let f = _interp.get_attr(&m, "getDOMImplementation")?;
                call_pos(_interp, f, &[])?
            };
            call_method(_interp, &impl_obj, "hasFeature", &[feature.0, version.0])
        }

        #[py_name = "isSameNode"]
        fn is_same_node(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            Ok(Value::bool_(values_are_identical(self_val.0, &other.0)))
        }

        #[py_name = "getInterface"]
        fn get_interface(#[positional_only] self_val: PyValueRef<'_>, feature: PyValue) -> Result<Value> {
            let ok = call_method(_interp, self_val.0, "isSupported", &[feature.0, Value::none()])?;
            if _interp.truthy_value(&ok)? {
                Ok(self_val.0.clone())
            } else {
                Ok(Value::none())
            }
        }

        #[py_name = "getUserData"]
        fn get_user_data(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            if let Ok(ud) = _interp.get_attr(self_val.0, "_user_data")
                && !ud.is_none()
            {
                let pair = call_method(_interp, &ud, "get", &[key.0, Value::none()])?;
                if !pair.is_none() {
                    let parts = _interp.collect_iterable(&pair)?;
                    if !parts.is_empty() {
                        return Ok(parts[0].clone());
                    }
                }
            }
            Ok(Value::none())
        }

        #[py_name = "setUserData"]
        fn set_user_data(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            data: PyValue,
            handler: PyValue,
        ) -> Result<Value> {
            let old = call_method(_interp, self_val.0, "getUserData", std::slice::from_ref(&key.0))?;
            let ud = match _interp.get_attr(self_val.0, "_user_data") {
                Ok(d) if !d.is_none() => d,
                _ => {
                    let d = Value::dict(PyDict::default());
                    _interp.assign_attr(self_val.0.clone(), "_user_data", d.clone())?;
                    d
                }
            };
            call_method(
                _interp,
                &ud,
                "__setitem__",
                &[key.0, Value::tuple(vec![data.0, handler.0])],
            )?;
            Ok(old)
        }

        fn _call_user_data_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            operation: PyValue,
            src: PyValue,
            dst: PyValue,
        ) -> Result<Value> {
            if let Ok(ud) = _interp.get_attr(self_val.0, "_user_data")
                && !ud.is_none()
            {
                let items = call_method(_interp, &ud, "items", &[])?;
                for pair in _interp.collect_iterable(&items)? {
                    let kv = _interp.collect_iterable(&pair)?;
                    if kv.len() == 2 {
                        let dh = _interp.collect_iterable(&kv[1])?;
                        if dh.len() == 2 && !dh[1].is_none() {
                            call_method(
                                _interp,
                                &dh[1],
                                "handle",
                                &[operation.0.clone(), kv[0].clone(), dh[0].clone(), src.0.clone(), dst.0.clone()],
                            )?;
                        }
                    }
                }
            }
            Ok(Value::none())
        }

        fn unlink(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "parentNode", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "previousSibling", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "nextSibling", Value::none())?;
            if let Ok(ch) = _interp.get_attr(self_val.0, "childNodes") {
                for c in _interp.collect_iterable(&ch)? {
                    let _ = call_method(_interp, &c, "unlink", &[]);
                }
                if matches!(ch.kind(), ValueKind::List(_))
                    || _interp.get_attr(&ch, "clear").is_ok()
                {
                    let _ = call_method(_interp, &ch, "clear", &[]);
                }
            }
            _interp.assign_attr(self_val.0.clone(), "ownerDocument", Value::none())?;
            Ok(Value::none())
        }
    }

    class DocumentFragment("Node") {
        constants {
            "nodeType" => Value::int(11),
            "nodeName" => Value::string("#document-fragment"),
            "nodeValue" => Value::none(),
            "attributes" => Value::none(),
        }

        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let nl = new_node_list(_interp, Vec::new())?;
            _interp.assign_attr(self_val.0.clone(), "childNodes", nl)?;
            Ok(Value::none())
        }
    }

    class Attr("Node") {
        constants {
            "nodeType" => Value::int(2),
            "attributes" => Value::none(),
            "specified" => Value::bool_(false),
            "ownerElement" => Value::none(),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            qName: PyStr<'_>,
            #[default(OptionalArg::Omitted)] namespaceURI: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] localName: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] prefix: OptValueRef<'_>,
        ) -> Result<Value> {
            let qn = qName.0.to_string();
            let (auto_pfx, auto_local) = split_qname_parts(&qn);
            let ns = namespaceURI.as_value().cloned().unwrap_or_else(Value::none);
            let ln = match localName.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => Value::string(auto_local),
            };
            let pfx = match prefix.as_value() {
                Some(v) => v.clone(),
                None => match auto_pfx {
                    Some(p) => Value::string(p),
                    None => Value::none(),
                },
            };
            _interp.assign_attr(self_val.0.clone(), "_name", Value::string(&qn))?;
            _interp.assign_attr(self_val.0.clone(), "namespaceURI", ns)?;
            _interp.assign_attr(self_val.0.clone(), "_localName", ln)?;
            _interp.assign_attr(self_val.0.clone(), "_prefix", pfx)?;
            _interp.assign_attr(self_val.0.clone(), "_value", Value::string(""))?;
            _interp.assign_attr(self_val.0.clone(), "specified", Value::bool_(true))?;
            _interp.assign_attr(self_val.0.clone(), "_is_id", Value::bool_(false))?;
            let nl = new_node_list(_interp, Vec::new())?;
            _interp.assign_attr(self_val.0.clone(), "childNodes", nl)?;
            Ok(Value::none())
        }

        #[getter]
        fn name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_name")
        }
        #[setter("name")]
        fn set_name(#[positional_only] self_val: PyValueRef<'_>, v: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_name", v.0)?;
            Ok(Value::none())
        }

        #[getter("nodeName")]
        fn node_name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_name")
        }

        #[getter]
        fn value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_value")
        }
        #[setter("value")]
        fn set_value(#[positional_only] self_val: PyValueRef<'_>, v: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_value", v.0)?;
            Ok(Value::none())
        }

        #[getter("nodeValue")]
        fn node_value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_value")
        }
        #[setter("nodeValue")]
        fn set_node_value(#[positional_only] self_val: PyValueRef<'_>, v: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_value", v.0)?;
            Ok(Value::none())
        }

        #[getter]
        fn prefix(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_prefix")
        }
        #[setter("prefix")]
        fn set_prefix(#[positional_only] self_val: PyValueRef<'_>, v: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_prefix", v.0)?;
            Ok(Value::none())
        }

        #[getter("isId")]
        fn is_id(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "_is_id")
        }

        #[getter("schemaType")]
        fn schema_type(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let cls = minidom_cls(_interp, "TypeInfo")?;
            call_pos(_interp, cls, &[Value::none(), Value::none()])
        }

        fn unlink(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "ownerElement", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "ownerDocument", Value::none())?;
            Ok(Value::none())
        }
    }

    class NamedNodeMap {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            attrs: PyValue,
            attrsNS: PyValue,
            ownerElement: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_attrs", attrs.0)?;
            _interp.assign_attr(self_val.0.clone(), "_attrsNS", attrsNS.0)?;
            _interp.assign_attr(self_val.0.clone(), "_ownerElement", ownerElement.0)?;
            Ok(Value::none())
        }

        #[getter]
        fn length(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            call_pos(_interp, Value::builtin_function("len"), &[a])
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            call_pos(_interp, Value::builtin_function("len"), &[a])
        }

        fn item(#[positional_only] self_val: PyValueRef<'_>, index: PyIndex) -> Result<Value> {
            let idx = index.expect_i64(FN_NAME, "index")?;
            if idx < 0 {
                return Ok(Value::none());
            }
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            let vals = call_method(_interp, &a, "values", &[])?;
            let list = _interp.collect_iterable(&vals)?;
            Ok(list.get(idx as usize).cloned().unwrap_or_else(Value::none))
        }

        fn items(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            let vals = call_method(_interp, &a, "values", &[])?;
            let mut out = Vec::new();
            for node in _interp.collect_iterable(&vals)? {
                let n = _interp.get_attr(&node, "name")?;
                let v = _interp.get_attr(&node, "value")?;
                out.push(Value::tuple(vec![n, v]));
            }
            Ok(Value::list(out))
        }

        #[py_name = "itemsNS"]
        fn items_ns(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            let vals = call_method(_interp, &a, "values", &[])?;
            let mut out = Vec::new();
            for node in _interp.collect_iterable(&vals)? {
                let ns = _interp.get_attr(&node, "namespaceURI")?;
                let ln = _interp.get_attr(&node, "localName")?;
                let v = _interp.get_attr(&node, "value")?;
                out.push(Value::tuple(vec![Value::tuple(vec![ns, ln]), v]));
            }
            Ok(Value::list(out))
        }

        fn keys(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            call_method(_interp, &a, "keys", &[])
        }

        #[py_name = "keysNS"]
        fn keys_ns(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrsNS")?;
            call_method(_interp, &a, "keys", &[])
        }

        fn values(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            call_method(_interp, &a, "values", &[])
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            #[default(OptionalArg::Omitted)] value: OptValueRef<'_>,
        ) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            let def = value.as_value().cloned().unwrap_or_else(Value::none);
            call_method(_interp, &a, "get", &[name.0, def])
        }

        fn __contains__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            if matches!(key.0.kind(), ValueKind::Tuple(_)) {
                let a = _interp.get_attr(self_val.0, "_attrsNS")?;
                call_method(_interp, &a, "__contains__", &[key.0])
            } else {
                let a = _interp.get_attr(self_val.0, "_attrs")?;
                call_method(_interp, &a, "__contains__", &[key.0])
            }
        }

        fn __getitem__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            if matches!(key.0.kind(), ValueKind::Tuple(_)) {
                let a = _interp.get_attr(self_val.0, "_attrsNS")?;
                call_method(_interp, &a, "__getitem__", &[key.0])
            } else {
                let a = _interp.get_attr(self_val.0, "_attrs")?;
                call_method(_interp, &a, "__getitem__", &[key.0])
            }
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            attname: PyValue,
            value: PyValue,
        ) -> Result<Value> {
            let owner = _interp.get_attr(self_val.0, "_ownerElement")?;
            if let Some(s) = value.0.as_str() {
                call_method(_interp, &owner, "setAttribute", &[attname.0, Value::string(s)])?;
            } else {
                call_method(_interp, &owner, "setAttributeNode", &[value.0])?;
            }
            Ok(Value::none())
        }

        fn __delitem__(#[positional_only] self_val: PyValueRef<'_>, attname: PyValue) -> Result<Value> {
            let node = call_method(_interp, self_val.0, "__getitem__", &[attname.0])?;
            let owner = _interp.get_attr(self_val.0, "_ownerElement")?;
            call_method(_interp, &owner, "removeAttributeNode", &[node])?;
            Ok(Value::none())
        }

        #[py_name = "getNamedItem"]
        fn get_named_item(#[positional_only] self_val: PyValueRef<'_>, name: PyValue) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            call_method(_interp, &a, "get", &[name.0, Value::none()])
        }

        #[py_name = "getNamedItemNS"]
        fn get_named_item_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            ns: PyValue,
            local: PyValue,
        ) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrsNS")?;
            call_method(_interp, &a, "get", &[Value::tuple(vec![ns.0, local.0]), Value::none()])
        }

        #[py_name = "removeNamedItem"]
        fn remove_named_item(#[positional_only] self_val: PyValueRef<'_>, name: PyValue) -> Result<Value> {
            let n = call_method(_interp, self_val.0, "getNamedItem", &[name.0])?;
            if n.is_none() {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            let owner = _interp.get_attr(self_val.0, "_ownerElement")?;
            call_method(_interp, &owner, "removeAttributeNode", &[n])
        }

        #[py_name = "removeNamedItemNS"]
        fn remove_named_item_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            ns: PyValue,
            local: PyValue,
        ) -> Result<Value> {
            let n = call_method(_interp, self_val.0, "getNamedItemNS", &[ns.0, local.0])?;
            if n.is_none() {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            let owner = _interp.get_attr(self_val.0, "_ownerElement")?;
            call_method(_interp, &owner, "removeAttributeNode", &[n])
        }

        #[py_name = "setNamedItem"]
        fn set_named_item(#[positional_only] self_val: PyValueRef<'_>, node: PyValue) -> Result<Value> {
            if get_node_type(_interp, &node.0) != 2 {
                return Err(raise_dom_exc(_interp, "HierarchyRequestErr", "not an Attr node"));
            }
            let owner = _interp.get_attr(self_val.0, "_ownerElement")?;
            call_method(_interp, &owner, "setAttributeNode", &[node.0])
        }

        #[py_name = "setNamedItemNS"]
        fn set_named_item_ns(#[positional_only] self_val: PyValueRef<'_>, node: PyValue) -> Result<Value> {
            call_method(_interp, self_val.0, "setNamedItem", &[node.0])
        }
    }

    class Element("Node") {
        constants {
            "nodeType" => Value::int(1),
            "nodeValue" => Value::none(),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            tagName: PyStr<'_>,
            #[default(OptionalArg::Omitted)] namespaceURI: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] prefix: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] localName: OptValueRef<'_>,
        ) -> Result<Value> {
            let tn = tagName.0.to_string();
            let (auto_pfx, auto_local) = split_qname_parts(&tn);
            let ns = namespaceURI.as_value().cloned().unwrap_or_else(Value::none);
            let pfx = match prefix.as_value() {
                Some(v) => v.clone(),
                None => match auto_pfx {
                    Some(p) => Value::string(p),
                    None => Value::none(),
                },
            };
            let ln = match localName.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => Value::string(auto_local),
            };
            _interp.assign_attr(self_val.0.clone(), "tagName", Value::string(&tn))?;
            _interp.assign_attr(self_val.0.clone(), "nodeName", Value::string(&tn))?;
            _interp.assign_attr(self_val.0.clone(), "namespaceURI", ns)?;
            _interp.assign_attr(self_val.0.clone(), "prefix", pfx)?;
            _interp.assign_attr(self_val.0.clone(), "_localName", ln)?;
            _interp.assign_attr(self_val.0.clone(), "_attrs", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_attrsNS", Value::none())?;
            let nl = new_node_list(_interp, Vec::new())?;
            _interp.assign_attr(self_val.0.clone(), "childNodes", nl)?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let tn = _interp.get_attr(self_val.0, "tagName")?;
            let tns = tn.as_str().unwrap_or("");
            Ok(Value::string(format!("<DOM Element: {tns}>")))
        }

        #[getter]
        fn attributes(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let (a, a_ns) = ensure_element_attrs(_interp, self_val.0)?;
            let cls = minidom_cls(_interp, "NamedNodeMap")?;
            call_pos(_interp, cls, &[a, a_ns, self_val.0.clone()])
        }

        #[py_name = "hasAttributes"]
        fn has_attributes(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            if a.is_none() {
                return Ok(Value::bool_(false));
            }
            let n = call_pos(_interp, Value::builtin_function("len"), &[a])?
                .as_int()
                .unwrap_or(0);
            Ok(Value::bool_(n > 0))
        }

        #[py_name = "getAttribute"]
        fn get_attribute(#[positional_only] self_val: PyValueRef<'_>, attname: PyStr<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            if a.is_none() {
                return Ok(Value::string(""));
            }
            let node = call_method(
                _interp,
                &a,
                "get",
                &[Value::string(attname.0.as_ref()), Value::none()],
            )?;
            if node.is_none() {
                Ok(Value::string(""))
            } else {
                _interp.get_attr(&node, "value")
            }
        }

        #[py_name = "getAttributeNS"]
        fn get_attribute_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            localName: PyStr<'_>,
        ) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrsNS")?;
            if a.is_none() {
                return Ok(Value::string(""));
            }
            let key = Value::tuple(vec![namespaceURI.0, Value::string(localName.0.as_ref())]);
            let node = call_method(_interp, &a, "get", &[key, Value::none()])?;
            if node.is_none() {
                Ok(Value::string(""))
            } else {
                _interp.get_attr(&node, "value")
            }
        }

        #[py_name = "setAttribute"]
        fn set_attribute(
            #[positional_only] self_val: PyValueRef<'_>,
            attname: PyStr<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let attr_cls = minidom_cls(_interp, "Attr")?;
            let attr = call_pos(
                _interp,
                attr_cls,
                &[
                    Value::string(attname.0.as_ref()),
                    Value::none(),
                    Value::string(attname.0.as_ref()),
                    Value::none(),
                ],
            )?;
            _interp.assign_attr(attr.clone(), "value", value.0)?;
            let owner = _interp.get_attr(self_val.0, "ownerDocument")?;
            _interp.assign_attr(attr.clone(), "ownerDocument", owner)?;
            call_method(_interp, self_val.0, "setAttributeNode", &[attr])?;
            Ok(Value::none())
        }

        #[py_name = "setAttributeNS"]
        fn set_attribute_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            qualifiedName: PyStr<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let (pfx_opt, local) = split_qname_parts(&qualifiedName.0);
            let pfx_val = match pfx_opt {
                Some(p) => Value::string(p),
                None => Value::none(),
            };
            let attr_cls = minidom_cls(_interp, "Attr")?;
            let attr = call_pos(
                _interp,
                attr_cls,
                &[
                    Value::string(qualifiedName.0.as_ref()),
                    namespaceURI.0,
                    Value::string(local),
                    pfx_val,
                ],
            )?;
            _interp.assign_attr(attr.clone(), "value", value.0)?;
            let owner = _interp.get_attr(self_val.0, "ownerDocument")?;
            _interp.assign_attr(attr.clone(), "ownerDocument", owner)?;
            call_method(_interp, self_val.0, "setAttributeNode", &[attr])?;
            Ok(Value::none())
        }

        #[py_name = "getAttributeNode"]
        fn get_attribute_node(
            #[positional_only] self_val: PyValueRef<'_>,
            attrname: PyStr<'_>,
        ) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            if a.is_none() {
                return Ok(Value::none());
            }
            call_method(
                _interp,
                &a,
                "get",
                &[Value::string(attrname.0.as_ref()), Value::none()],
            )
        }

        #[py_name = "getAttributeNodeNS"]
        fn get_attribute_node_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            localName: PyStr<'_>,
        ) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrsNS")?;
            if a.is_none() {
                return Ok(Value::none());
            }
            let key = Value::tuple(vec![namespaceURI.0, Value::string(localName.0.as_ref())]);
            call_method(_interp, &a, "get", &[key, Value::none()])
        }

        #[py_name = "setAttributeNode"]
        fn set_attribute_node(
            #[positional_only] self_val: PyValueRef<'_>,
            attr: PyValue,
        ) -> Result<Value> {
            let old_owner = _interp.get_attr(&attr.0, "ownerElement")?;
            if !old_owner.is_none() && !values_are_identical(&old_owner, self_val.0) {
                return Err(raise_dom_exc(
                    _interp,
                    "InuseAttributeErr",
                    "attribute is already owned by another element",
                ));
            }
            let (a, a_ns) = ensure_element_attrs(_interp, self_val.0)?;
            let name = _interp.get_attr(&attr.0, "name")?;
            let ns = _interp.get_attr(&attr.0, "namespaceURI")?;
            let local = _interp.get_attr(&attr.0, "localName")?;
            let ns_key = Value::tuple(vec![ns, local]);
            let old1 = call_method(_interp, &a, "get", &[name.clone(), Value::none()])?;
            let old2 = call_method(_interp, &a_ns, "get", &[ns_key.clone(), Value::none()])?;
            let old = if !old1.is_none() {
                if !values_are_identical(&old1, &attr.0) {
                    let _ = _interp.assign_attr(old1.clone(), "ownerElement", Value::none());
                }
                old1
            } else if !old2.is_none() {
                if !values_are_identical(&old2, &attr.0) {
                    let _ = _interp.assign_attr(old2.clone(), "ownerElement", Value::none());
                }
                old2
            } else {
                Value::none()
            };
            call_method(_interp, &a, "__setitem__", &[name, attr.0.clone()])?;
            call_method(_interp, &a_ns, "__setitem__", &[ns_key, attr.0.clone()])?;
            _interp.assign_attr(attr.0.clone(), "ownerElement", self_val.0.clone())?;
            let owner_doc = _interp.get_attr(self_val.0, "ownerDocument")?;
            _interp.assign_attr(attr.0, "ownerDocument", owner_doc)?;
            Ok(old)
        }

        #[py_name = "setAttributeNodeNS"]
        fn set_attribute_node_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            attr: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "setAttributeNode", &[attr.0])
        }

        #[py_name = "removeAttribute"]
        fn remove_attribute(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            if a.is_none() {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            let attr = call_method(
                _interp,
                &a,
                "get",
                &[Value::string(name.0.as_ref()), Value::none()],
            )?;
            if attr.is_none() {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            call_method(_interp, self_val.0, "removeAttributeNode", &[attr])?;
            Ok(Value::none())
        }

        #[py_name = "removeAttributeNS"]
        fn remove_attribute_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            localName: PyStr<'_>,
        ) -> Result<Value> {
            let a_ns = _interp.get_attr(self_val.0, "_attrsNS")?;
            if a_ns.is_none() {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            let key = Value::tuple(vec![namespaceURI.0, Value::string(localName.0.as_ref())]);
            let attr = call_method(_interp, &a_ns, "get", &[key, Value::none()])?;
            if attr.is_none() {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            call_method(_interp, self_val.0, "removeAttributeNode", &[attr])?;
            Ok(Value::none())
        }

        #[py_name = "removeAttributeNode"]
        fn remove_attribute_node(
            #[positional_only] self_val: PyValueRef<'_>,
            node: PyValue,
        ) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            let a_ns = _interp.get_attr(self_val.0, "_attrsNS")?;
            if a.is_none() {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            let name = _interp.get_attr(&node.0, "name")?;
            let ns = _interp.get_attr(&node.0, "namespaceURI")?;
            let local = _interp.get_attr(&node.0, "localName")?;
            let has = call_method(_interp, &a, "__contains__", std::slice::from_ref(&name))?;
            if !_interp.truthy_value(&has)? {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            call_method(_interp, &a, "pop", &[name, Value::none()])?;
            call_method(
                _interp,
                &a_ns,
                "pop",
                &[Value::tuple(vec![ns, local]), Value::none()],
            )?;
            _interp.assign_attr(node.0.clone(), "ownerElement", Value::none())?;
            Ok(node.0)
        }

        #[py_name = "removeAttributeNodeNS"]
        fn remove_attribute_node_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            node: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "removeAttributeNode", &[node.0])
        }

        #[py_name = "hasAttribute"]
        fn has_attribute(#[positional_only] self_val: PyValueRef<'_>, name: PyStr<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            if a.is_none() {
                return Ok(Value::bool_(false));
            }
            call_method(_interp, &a, "__contains__", &[Value::string(name.0.as_ref())])
        }

        #[py_name = "hasAttributeNS"]
        fn has_attribute_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            localName: PyStr<'_>,
        ) -> Result<Value> {
            let a_ns = _interp.get_attr(self_val.0, "_attrsNS")?;
            if a_ns.is_none() {
                return Ok(Value::bool_(false));
            }
            call_method(
                _interp,
                &a_ns,
                "__contains__",
                &[Value::tuple(vec![namespaceURI.0, Value::string(localName.0.as_ref())])],
            )
        }

        #[py_name = "getElementsByTagName"]
        fn get_elements_by_tag_name(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let mut out = Vec::new();
            collect_elements_by_tag_name(_interp, self_val.0, &name.0, &mut out)?;
            new_node_list(_interp, out)
        }

        #[py_name = "getElementsByTagNameNS"]
        fn get_elements_by_tag_name_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            localName: PyStr<'_>,
        ) -> Result<Value> {
            let ns_opt = namespaceURI.0.as_str();
            let mut out = Vec::new();
            collect_elements_by_tag_name_ns(_interp, self_val.0, ns_opt, &localName.0, &mut out)?;
            new_node_list(_interp, out)
        }

        #[py_name = "setIdAttribute"]
        fn set_id_attribute(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let attr = call_method(
                _interp,
                self_val.0,
                "getAttributeNode",
                &[Value::string(name.0.as_ref())],
            )?;
            if attr.is_none() {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            call_method(_interp, self_val.0, "setIdAttributeNode", &[attr])
        }

        #[py_name = "setIdAttributeNS"]
        fn set_id_attribute_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            localName: PyStr<'_>,
        ) -> Result<Value> {
            let attr = call_method(
                _interp,
                self_val.0,
                "getAttributeNodeNS",
                &[namespaceURI.0, Value::string(localName.0.as_ref())],
            )?;
            if attr.is_none() {
                return Err(raise_dom_exc(_interp, "NotFoundErr", "attribute not found"));
            }
            call_method(_interp, self_val.0, "setIdAttributeNode", &[attr])
        }

        #[py_name = "setIdAttributeNode"]
        fn set_id_attribute_node(
            #[positional_only] self_val: PyValueRef<'_>,
            idAttr: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(idAttr.0.clone(), "_is_id", Value::bool_(true))?;
            if let Ok(doc) = _interp.get_attr(self_val.0, "ownerDocument")
                && !doc.is_none()
                && let Ok(cache) = _interp.get_attr(&doc, "_id_cache")
            {
                let v = _interp.get_attr(&idAttr.0, "value")?;
                let _ = call_method(_interp, &cache, "__setitem__", &[v, self_val.0.clone()]);
            }
            Ok(Value::none())
        }

        fn writexml(
            #[positional_only] self_val: PyValueRef<'_>,
            writer: PyValue,
            #[default(OptionalArg::Omitted)] indent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] addindent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] newl: OptValueRef<'_>,
        ) -> Result<Value> {
            let ind = indent.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let add = addindent.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let nl = newl.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let tn = _interp.get_attr(self_val.0, "tagName")?;
            let tns = tn.as_str().unwrap_or("");
            write_str(_interp, &writer.0, &format!("{ind}<{tns}"))?;

            let attrs = _interp.get_attr(self_val.0, "_attrs")?;
            if !attrs.is_none() {
                let items = call_method(_interp, &attrs, "items", &[])?;
                for pair in _interp.collect_iterable(&items)? {
                    let kv = _interp.collect_iterable(&pair)?;
                    if kv.len() == 2 {
                        let k = kv[0].as_str().unwrap_or("");
                        let val_v = _interp.get_attr(&kv[1], "value")?;
                        let val_s = val_v.as_str().unwrap_or("");
                        let esc = escape_xml_data(val_s, true);
                        write_str(_interp, &writer.0, &format!(" {k}=\"{esc}\""))?;
                    }
                }
            }

            let children = _interp.get_attr(self_val.0, "childNodes")?;
            let child_vec = _interp.collect_iterable(&children)?;
            if !child_vec.is_empty() {
                write_str(_interp, &writer.0, ">")?;
                if child_vec.len() == 1
                    && matches!(get_node_type(_interp, &child_vec[0]), 3 | 4)
                {
                    call_method(
                        _interp,
                        &child_vec[0],
                        "writexml",
                        &[
                            writer.0.clone(),
                            Value::string(""),
                            Value::string(""),
                            Value::string(""),
                        ],
                    )?;
                    write_str(_interp, &writer.0, &format!("</{tns}>{nl}"))?;
                } else {
                    write_str(_interp, &writer.0, nl)?;
                    let next_ind = format!("{ind}{add}");
                    for child in child_vec {
                        call_method(
                            _interp,
                            &child,
                            "writexml",
                            &[
                                writer.0.clone(),
                                Value::string(&next_ind),
                                Value::string(add),
                                Value::string(nl),
                            ],
                        )?;
                    }
                    write_str(_interp, &writer.0, &format!("{ind}</{tns}>{nl}"))?;
                }
            } else {
                write_str(_interp, &writer.0, &format!("/>{nl}"))?;
            }
            Ok(Value::none())
        }

        fn unlink(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let a = _interp.get_attr(self_val.0, "_attrs")?;
            if !a.is_none() {
                let vals = call_method(_interp, &a, "values", &[])?;
                for attr in _interp.collect_iterable(&vals)? {
                    let _ = call_method(_interp, &attr, "unlink", &[]);
                }
            }
            _interp.assign_attr(self_val.0.clone(), "_attrs", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_attrsNS", Value::none())?;
            let node_cls = minidom_cls(_interp, "Node")?;
            let node_unlink = _interp.get_attr(&node_cls, "unlink")?;
            call_pos(_interp, node_unlink, std::slice::from_ref(self_val.0))
        }
    }

    class Childless {
        constants {
            "attributes" => Value::none(),
            "firstChild" => Value::none(),
            "lastChild" => Value::none(),
        }

        #[getter("childNodes")]
        fn child_nodes(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            new_empty_node_list(_interp)
        }

        #[py_name = "appendChild"]
        fn append_child(#[positional_only] _self: PyValueRef<'_>, _node: PyValue) -> Result<Value> {
            Err(raise_dom_exc(
                _interp,
                "HierarchyRequestErr",
                "node cannot have children",
            ))
        }

        #[py_name = "hasChildNodes"]
        fn has_child_nodes(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(false))
        }

        #[py_name = "insertBefore"]
        fn insert_before(
            #[positional_only] _self: PyValueRef<'_>,
            _newChild: PyValue,
            _refChild: PyValue,
        ) -> Result<Value> {
            Err(raise_dom_exc(
                _interp,
                "HierarchyRequestErr",
                "node cannot have children",
            ))
        }

        #[py_name = "removeChild"]
        fn remove_child(#[positional_only] _self: PyValueRef<'_>, _oldChild: PyValue) -> Result<Value> {
            Err(raise_dom_exc(_interp, "NotFoundErr", "node has no children"))
        }

        #[py_name = "replaceChild"]
        fn replace_child(
            #[positional_only] _self: PyValueRef<'_>,
            _newChild: PyValue,
            _oldChild: PyValue,
        ) -> Result<Value> {
            Err(raise_dom_exc(
                _interp,
                "HierarchyRequestErr",
                "node cannot have children",
            ))
        }

        fn normalize(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }

    class ProcessingInstruction("Childless", "Node") {
        constants {
            "nodeType" => Value::int(7),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            target: PyValue,
            data: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "target", target.0)?;
            _interp.assign_attr(self_val.0.clone(), "data", data.0)?;
            Ok(Value::none())
        }

        #[getter("nodeName")]
        fn node_name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "target")
        }

        #[getter("nodeValue")]
        fn node_value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "data")
        }
        #[setter("nodeValue")]
        fn set_node_value(#[positional_only] self_val: PyValueRef<'_>, v: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "data", v.0)?;
            Ok(Value::none())
        }

        fn writexml(
            #[positional_only] self_val: PyValueRef<'_>,
            writer: PyValue,
            #[default(OptionalArg::Omitted)] indent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _addindent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] newl: OptValueRef<'_>,
        ) -> Result<Value> {
            let ind = indent.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let nl = newl.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let target = _interp.get_attr(self_val.0, "target")?;
            let data = _interp.get_attr(self_val.0, "data")?;
            let ts = target.as_str().unwrap_or("");
            let ds = data.as_str().unwrap_or("");
            write_str(_interp, &writer.0, &format!("{ind}<?{ts} {ds}?>{nl}"))?;
            Ok(Value::none())
        }
    }

    class CharacterData("Childless", "Node") {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "data", Value::string(""))?;
            Ok(Value::none())
        }

        #[getter("nodeValue")]
        fn node_value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "data")
        }
        #[setter("nodeValue")]
        fn set_node_value(#[positional_only] self_val: PyValueRef<'_>, v: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "data", v.0)?;
            Ok(Value::none())
        }

        #[getter]
        fn length(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let d = _interp.get_attr(self_val.0, "data")?;
            call_pos(_interp, Value::builtin_function("len"), &[d])
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let d = _interp.get_attr(self_val.0, "data")?;
            call_pos(_interp, Value::builtin_function("len"), &[d])
        }

        #[py_name = "substringData"]
        fn substring_data(
            #[positional_only] self_val: PyValueRef<'_>,
            offset: PyIndex,
            count: PyIndex,
        ) -> Result<Value> {
            let off = offset.expect_i64(FN_NAME, "offset")?;
            let cnt = count.expect_i64(FN_NAME, "count")?;
            let d = _interp.get_attr(self_val.0, "data")?;
            let chars: Vec<char> = d.as_str().unwrap_or("").chars().collect();
            if off < 0 || (off as usize) >= chars.len() || cnt < 0 {
                return Err(raise_dom_exc(_interp, "IndexSizeErr", "offset/count out of range"));
            }
            let start = off as usize;
            let end = (start + cnt as usize).min(chars.len());
            let sub: String = chars[start..end].iter().collect();
            Ok(Value::string(sub))
        }

        #[py_name = "appendData"]
        fn append_data(#[positional_only] self_val: PyValueRef<'_>, arg: PyStr<'_>) -> Result<Value> {
            let d = _interp.get_attr(self_val.0, "data")?;
            let cur = d.as_str().unwrap_or("");
            _interp.assign_attr(self_val.0.clone(), "data", Value::string(format!("{cur}{}", arg.0)))?;
            Ok(Value::none())
        }

        #[py_name = "insertData"]
        fn insert_data(
            #[positional_only] self_val: PyValueRef<'_>,
            offset: PyIndex,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let off = offset.expect_i64(FN_NAME, "offset")?;
            let d = _interp.get_attr(self_val.0, "data")?;
            let chars: Vec<char> = d.as_str().unwrap_or("").chars().collect();
            if off < 0 || (off as usize) > chars.len() {
                return Err(raise_dom_exc(_interp, "IndexSizeErr", "offset out of range"));
            }
            let idx = off as usize;
            let before: String = chars[..idx].iter().collect();
            let after: String = chars[idx..].iter().collect();
            _interp.assign_attr(
                self_val.0.clone(),
                "data",
                Value::string(format!("{before}{}{after}", arg.0)),
            )?;
            Ok(Value::none())
        }

        #[py_name = "deleteData"]
        fn delete_data(
            #[positional_only] self_val: PyValueRef<'_>,
            offset: PyIndex,
            count: PyIndex,
        ) -> Result<Value> {
            let off = offset.expect_i64(FN_NAME, "offset")?;
            let cnt = count.expect_i64(FN_NAME, "count")?;
            let d = _interp.get_attr(self_val.0, "data")?;
            let chars: Vec<char> = d.as_str().unwrap_or("").chars().collect();
            if off < 0 || (off as usize) >= chars.len() || cnt < 0 {
                return Err(raise_dom_exc(_interp, "IndexSizeErr", "offset/count out of range"));
            }
            let start = off as usize;
            let end = (start + cnt as usize).min(chars.len());
            let before: String = chars[..start].iter().collect();
            let after: String = chars[end..].iter().collect();
            _interp.assign_attr(
                self_val.0.clone(),
                "data",
                Value::string(format!("{before}{after}")),
            )?;
            Ok(Value::none())
        }

        #[py_name = "replaceData"]
        fn replace_data(
            #[positional_only] self_val: PyValueRef<'_>,
            offset: PyIndex,
            count: PyIndex,
            arg: PyStr<'_>,
        ) -> Result<Value> {
            let off = offset.expect_i64(FN_NAME, "offset")?;
            let cnt = count.expect_i64(FN_NAME, "count")?;
            let d = _interp.get_attr(self_val.0, "data")?;
            let chars: Vec<char> = d.as_str().unwrap_or("").chars().collect();
            if off < 0 || (off as usize) >= chars.len() || cnt < 0 {
                return Err(raise_dom_exc(_interp, "IndexSizeErr", "offset/count out of range"));
            }
            let start = off as usize;
            let end = (start + cnt as usize).min(chars.len());
            let before: String = chars[..start].iter().collect();
            let after: String = chars[end..].iter().collect();
            _interp.assign_attr(
                self_val.0.clone(),
                "data",
                Value::string(format!("{before}{}{after}", arg.0)),
            )?;
            Ok(Value::none())
        }
    }

    class Text("CharacterData") {
        constants {
            "nodeType" => Value::int(3),
            "nodeName" => Value::string("#text"),
        }

        #[py_name = "splitText"]
        fn split_text(#[positional_only] self_val: PyValueRef<'_>, offset: PyIndex) -> Result<Value> {
            let off = offset.expect_i64(FN_NAME, "offset")?;
            let d = _interp.get_attr(self_val.0, "data")?;
            let chars: Vec<char> = d.as_str().unwrap_or("").chars().collect();
            if off < 0 || (off as usize) > chars.len() {
                return Err(raise_dom_exc(_interp, "IndexSizeErr", "illegal offset value"));
            }
            let idx = off as usize;
            let head: String = chars[..idx].iter().collect();
            let tail: String = chars[idx..].iter().collect();
            let cls = _interp.get_attr(self_val.0, "__class__")?;
            let new_text = call_pos(_interp, cls, &[])?;
            _interp.assign_attr(new_text.clone(), "data", Value::string(tail))?;
            _interp.assign_attr(self_val.0.clone(), "data", Value::string(head))?;
            let owner = _interp.get_attr(self_val.0, "ownerDocument")?;
            _interp.assign_attr(new_text.clone(), "ownerDocument", owner)?;
            let parent = _interp.get_attr(self_val.0, "parentNode")?;
            if !parent.is_none() {
                let next = _interp.get_attr(self_val.0, "nextSibling")?;
                call_method(_interp, &parent, "insertBefore", &[new_text.clone(), next])?;
            }
            Ok(new_text)
        }

        #[getter("wholeText")]
        fn whole_text(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut parts: Vec<String> = Vec::new();
            // Walk backward
            let mut cur = _interp.get_attr(self_val.0, "previousSibling")?;
            let mut rev_prefix = Vec::new();
            while !cur.is_none() && matches!(get_node_type(_interp, &cur), 3 | 4) {
                let d = _interp.get_attr(&cur, "data")?;
                rev_prefix.push(d.as_str().unwrap_or("").to_string());
                cur = _interp.get_attr(&cur, "previousSibling")?;
            }
            for p in rev_prefix.into_iter().rev() {
                parts.push(p);
            }
            let self_d = _interp.get_attr(self_val.0, "data")?;
            parts.push(self_d.as_str().unwrap_or("").to_string());
            // Walk forward
            let mut cur = _interp.get_attr(self_val.0, "nextSibling")?;
            while !cur.is_none() && matches!(get_node_type(_interp, &cur), 3 | 4) {
                let d = _interp.get_attr(&cur, "data")?;
                parts.push(d.as_str().unwrap_or("").to_string());
                cur = _interp.get_attr(&cur, "nextSibling")?;
            }
            Ok(Value::string(parts.concat()))
        }

        #[getter("isElementContentWhitespace")]
        fn is_element_content_whitespace(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let d = _interp.get_attr(self_val.0, "data")?;
            let s = d.as_str().unwrap_or("");
            Ok(Value::bool_(!s.is_empty() && s.trim().is_empty()))
        }

        fn writexml(
            #[positional_only] self_val: PyValueRef<'_>,
            writer: PyValue,
            #[default(OptionalArg::Omitted)] indent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _addindent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] newl: OptValueRef<'_>,
        ) -> Result<Value> {
            let ind = indent.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let nl = newl.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let d = _interp.get_attr(self_val.0, "data")?;
            let raw = d.as_str().unwrap_or("");
            let esc = escape_xml_data(raw, false);
            write_str(_interp, &writer.0, &format!("{ind}{esc}{nl}"))?;
            Ok(Value::none())
        }
    }

    class CDATASection("Text") {
        constants {
            "nodeType" => Value::int(4),
            "nodeName" => Value::string("#cdata-section"),
        }

        fn writexml(
            #[positional_only] self_val: PyValueRef<'_>,
            writer: PyValue,
            #[default(OptionalArg::Omitted)] _indent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _addindent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _newl: OptValueRef<'_>,
        ) -> Result<Value> {
            let d = _interp.get_attr(self_val.0, "data")?;
            let raw = d.as_str().unwrap_or("");
            if raw.contains("]]>") {
                return Err(PyError::named("ValueError", "']]>' not allowed in a CDATA section"));
            }
            write_str(_interp, &writer.0, &format!("<![CDATA[{raw}]]>"))?;
            Ok(Value::none())
        }
    }

    class Comment("CharacterData") {
        constants {
            "nodeType" => Value::int(8),
            "nodeName" => Value::string("#comment"),
        }

        fn __init__(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "data", data.0)?;
            Ok(Value::none())
        }

        fn writexml(
            #[positional_only] self_val: PyValueRef<'_>,
            writer: PyValue,
            #[default(OptionalArg::Omitted)] indent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _addindent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] newl: OptValueRef<'_>,
        ) -> Result<Value> {
            let ind = indent.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let nl = newl.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let d = _interp.get_attr(self_val.0, "data")?;
            let raw = d.as_str().unwrap_or("");
            if raw.contains("--") {
                return Err(PyError::named("ValueError", "'--' is not allowed in a comment node"));
            }
            write_str(_interp, &writer.0, &format!("{ind}<!--{raw}-->{nl}"))?;
            Ok(Value::none())
        }
    }

    class ReadOnlySequentialNamedNodeMap {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] seq: OptValueRef<'_>,
        ) -> Result<Value> {
            let s = seq
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::tuple(Vec::new()));
            _interp.assign_attr(self_val.0.clone(), "_seq", s)?;
            Ok(Value::none())
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let s = _interp.get_attr(self_val.0, "_seq")?;
            call_pos(_interp, Value::builtin_function("len"), &[s])
        }

        #[getter]
        fn length(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let s = _interp.get_attr(self_val.0, "_seq")?;
            call_pos(_interp, Value::builtin_function("len"), &[s])
        }

        fn item(#[positional_only] self_val: PyValueRef<'_>, index: PyIndex) -> Result<Value> {
            let idx = index.expect_i64(FN_NAME, "index")?;
            if idx < 0 {
                return Ok(Value::none());
            }
            let s = _interp.get_attr(self_val.0, "_seq")?;
            let items = _interp.collect_iterable(&s)?;
            Ok(items.get(idx as usize).cloned().unwrap_or_else(Value::none))
        }

        #[py_name = "getNamedItem"]
        fn get_named_item(#[positional_only] self_val: PyValueRef<'_>, name: PyStr<'_>) -> Result<Value> {
            let s = _interp.get_attr(self_val.0, "_seq")?;
            for n in _interp.collect_iterable(&s)? {
                if let Ok(nn) = _interp.get_attr(&n, "nodeName")
                    && nn.as_str() == Some(name.0.as_ref())
                {
                    return Ok(n);
                }
            }
            Ok(Value::none())
        }

        #[py_name = "getNamedItemNS"]
        fn get_named_item_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            ns: PyValue,
            local: PyStr<'_>,
        ) -> Result<Value> {
            let s = _interp.get_attr(self_val.0, "_seq")?;
            for n in _interp.collect_iterable(&s)? {
                let n_ns = _interp.get_attr(&n, "namespaceURI").unwrap_or_else(|_| Value::none());
                let n_ln = _interp.get_attr(&n, "localName").unwrap_or_else(|_| Value::none());
                if n_ns.as_str() == ns.0.as_str() && n_ln.as_str() == Some(local.0.as_ref()) {
                    return Ok(n);
                }
            }
            Ok(Value::none())
        }

        #[py_name = "setNamedItem"]
        fn set_named_item(#[positional_only] _self: PyValueRef<'_>, _node: PyValue) -> Result<Value> {
            Err(raise_dom_exc(
                _interp,
                "NoModificationAllowedErr",
                "NamedNodeMap instance is read-only",
            ))
        }

        #[py_name = "removeNamedItem"]
        fn remove_named_item(#[positional_only] _self: PyValueRef<'_>, _name: PyValue) -> Result<Value> {
            Err(raise_dom_exc(
                _interp,
                "NoModificationAllowedErr",
                "NamedNodeMap instance is read-only",
            ))
        }
    }

    class Identified {
        constants {
            "publicId" => Value::none(),
            "systemId" => Value::none(),
        }
    }

    class DocumentType("Identified", "Childless", "Node") {
        constants {
            "nodeType" => Value::int(10),
            "nodeValue" => Value::none(),
            "internalSubset" => Value::none(),
        }

        fn __init__(#[positional_only] self_val: PyValueRef<'_>, qualifiedName: PyStr<'_>) -> Result<Value> {
            let qn = qualifiedName.0.to_string();
            let nm_cls = minidom_cls(_interp, "ReadOnlySequentialNamedNodeMap")?;
            let ents = call_pos(_interp, nm_cls.clone(), &[])?;
            let nots = call_pos(_interp, nm_cls, &[])?;
            _interp.assign_attr(self_val.0.clone(), "name", Value::string(&qn))?;
            _interp.assign_attr(self_val.0.clone(), "nodeName", Value::string(&qn))?;
            _interp.assign_attr(self_val.0.clone(), "entities", ents)?;
            _interp.assign_attr(self_val.0.clone(), "notations", nots)?;
            _interp.assign_attr(self_val.0.clone(), "publicId", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "systemId", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "internalSubset", Value::none())?;
            Ok(Value::none())
        }

        fn writexml(
            #[positional_only] self_val: PyValueRef<'_>,
            writer: PyValue,
            #[default(OptionalArg::Omitted)] indent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _addindent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] newl: OptValueRef<'_>,
        ) -> Result<Value> {
            let ind = indent.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let nl = newl.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let name = _interp.get_attr(self_val.0, "name")?;
            let ns = name.as_str().unwrap_or("");
            let pubid = _interp.get_attr(self_val.0, "publicId")?;
            let sysid = _interp.get_attr(self_val.0, "systemId")?;
            let subset = _interp.get_attr(self_val.0, "internalSubset")?;
            write_str(_interp, &writer.0, &format!("{ind}<!DOCTYPE {ns}"))?;
            if let Some(p) = pubid.as_str() {
                let s = sysid.as_str().unwrap_or("");
                write_str(_interp, &writer.0, &format!("  PUBLIC '{p}'\n  '{s}'"))?;
            } else if let Some(s) = sysid.as_str() {
                write_str(_interp, &writer.0, &format!("  SYSTEM '{s}'"))?;
            }
            if let Some(sub) = subset.as_str() {
                write_str(_interp, &writer.0, &format!(" [{sub}]"))?;
            }
            write_str(_interp, &writer.0, &format!(">{nl}"))?;
            Ok(Value::none())
        }
    }

    class Entity("Identified", "Node") {
        constants {
            "nodeType" => Value::int(6),
            "nodeValue" => Value::none(),
            "attributes" => Value::none(),
            "actualEncoding" => Value::none(),
            "encoding" => Value::none(),
            "version" => Value::none(),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            publicId: PyValue,
            systemId: PyValue,
            notation: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "nodeName", name.0)?;
            _interp.assign_attr(self_val.0.clone(), "publicId", publicId.0)?;
            _interp.assign_attr(self_val.0.clone(), "systemId", systemId.0)?;
            _interp.assign_attr(self_val.0.clone(), "notationName", notation.0)?;
            let nl = new_node_list(_interp, Vec::new())?;
            _interp.assign_attr(self_val.0.clone(), "childNodes", nl)?;
            Ok(Value::none())
        }

        #[py_name = "appendChild"]
        fn append_child(#[positional_only] _self: PyValueRef<'_>, _node: PyValue) -> Result<Value> {
            Err(raise_dom_exc(
                _interp,
                "HierarchyRequestErr",
                "cannot append children to an entity node",
            ))
        }
    }

    class Notation("Identified", "Childless", "Node") {
        constants {
            "nodeType" => Value::int(12),
            "nodeValue" => Value::none(),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            publicId: PyValue,
            systemId: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "nodeName", name.0)?;
            _interp.assign_attr(self_val.0.clone(), "publicId", publicId.0)?;
            _interp.assign_attr(self_val.0.clone(), "systemId", systemId.0)?;
            Ok(Value::none())
        }
    }

    class DOMImplementation {
        #[py_name = "hasFeature"]
        fn has_feature(
            #[positional_only] _self: PyValueRef<'_>,
            feature: PyStr<'_>,
            version: PyValue,
        ) -> Result<Value> {
            let f = feature.0.to_ascii_lowercase();
            let v = if version.0.is_none() || version.0.as_str() == Some("") {
                None
            } else {
                version.0.as_str()
            };
            let supported = matches!(
                (f.as_str(), v),
                ("core", None | Some("1.0") | Some("2.0"))
                    | ("xml", None | Some("1.0") | Some("2.0"))
                    | ("ls-load", None | Some("3.0"))
            );
            Ok(Value::bool_(supported))
        }

        #[py_name = "createDocument"]
        fn create_document(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            qualifiedName: PyValue,
            doctype: PyValue,
        ) -> Result<Value> {
            if !doctype.0.is_none()
                && let Ok(p) = _interp.get_attr(&doctype.0, "parentNode")
                && !p.is_none()
            {
                return Err(raise_dom_exc(
                    _interp,
                    "WrongDocumentErr",
                    "doctype object owned by another DOM tree",
                ));
            }
            let doc = call_method(_interp, self_val.0, "_create_document", &[])?;
            if !doctype.0.is_none() {
                _interp.assign_attr(doctype.0.clone(), "ownerDocument", doc.clone())?;
                append_child_inner(_interp, &doc, &doctype.0)?;
            }
            if !qualifiedName.0.is_none() {
                let el = call_method(
                    _interp,
                    &doc,
                    "createElementNS",
                    &[namespaceURI.0, qualifiedName.0],
                )?;
                append_child_inner(_interp, &doc, &el)?;
            }
            Ok(doc)
        }

        #[py_name = "createDocumentType"]
        fn create_document_type(
            #[positional_only] _self: PyValueRef<'_>,
            qualifiedName: PyStr<'_>,
            publicId: PyValue,
            systemId: PyValue,
        ) -> Result<Value> {
            let dt_cls = minidom_cls(_interp, "DocumentType")?;
            let dt = call_pos(_interp, dt_cls, &[Value::string(qualifiedName.0.as_ref())])?;
            _interp.assign_attr(dt.clone(), "publicId", publicId.0)?;
            _interp.assign_attr(dt.clone(), "systemId", systemId.0)?;
            Ok(dt)
        }

        #[py_name = "getInterface"]
        fn get_interface(#[positional_only] self_val: PyValueRef<'_>, feature: PyValue) -> Result<Value> {
            let ok = call_method(_interp, self_val.0, "hasFeature", &[feature.0, Value::none()])?;
            if _interp.truthy_value(&ok)? {
                Ok(self_val.0.clone())
            } else {
                Ok(Value::none())
            }
        }

        fn _create_document(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            let cls = minidom_cls(_interp, "Document")?;
            call_pos(_interp, cls, &[])
        }
    }

    class Document("Node") {
        constants {
            "nodeType" => Value::int(9),
            "nodeName" => Value::string("#document"),
            "nodeValue" => Value::none(),
            "attributes" => Value::none(),
            "strictErrorChecking" => Value::bool_(false),
            "version" => Value::none(),
            "standalone" => Value::none(),
            "encoding" => Value::none(),
            "actualEncoding" => Value::none(),
            "documentURI" => Value::none(),
            "errorHandler" => Value::none(),
        }

        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let nl = new_node_list(_interp, Vec::new())?;
            _interp.assign_attr(self_val.0.clone(), "childNodes", nl)?;
            _interp.assign_attr(self_val.0.clone(), "_elem_info", Value::dict(PyDict::default()))?;
            _interp.assign_attr(self_val.0.clone(), "_id_cache", Value::dict(PyDict::default()))?;
            _interp.assign_attr(self_val.0.clone(), "_id_search_stack", Value::none())?;
            let impl_cls = minidom_cls(_interp, "DOMImplementation")?;
            let impl_inst = call_pos(_interp, impl_cls, &[])?;
            _interp.assign_attr(self_val.0.clone(), "implementation", impl_inst)?;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            _et: PyValue,
            _ev: PyValue,
            _tb: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "unlink", &[])?;
            Ok(Value::none())
        }

        #[getter("documentElement")]
        fn document_element(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let children = _interp.get_attr(self_val.0, "childNodes")?;
            for c in _interp.collect_iterable(&children)? {
                if get_node_type(_interp, &c) == 1 {
                    return Ok(c);
                }
            }
            Ok(Value::none())
        }

        #[getter]
        fn doctype(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let children = _interp.get_attr(self_val.0, "childNodes")?;
            for c in _interp.collect_iterable(&children)? {
                if get_node_type(_interp, &c) == 10 {
                    return Ok(c);
                }
            }
            Ok(Value::none())
        }

        #[py_name = "createElement"]
        fn create_element(#[positional_only] self_val: PyValueRef<'_>, tagName: PyStr<'_>) -> Result<Value> {
            let cls = minidom_cls(_interp, "Element")?;
            let el = call_pos(_interp, cls, &[Value::string(tagName.0.as_ref())])?;
            _interp.assign_attr(el.clone(), "ownerDocument", self_val.0.clone())?;
            Ok(el)
        }

        #[py_name = "createElementNS"]
        fn create_element_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            qualifiedName: PyStr<'_>,
        ) -> Result<Value> {
            let (pfx_opt, local) = split_qname_parts(&qualifiedName.0);
            let pfx_val = match pfx_opt {
                Some(p) => Value::string(p),
                None => Value::none(),
            };
            let cls = minidom_cls(_interp, "Element")?;
            let el = call_pos(
                _interp,
                cls,
                &[
                    Value::string(qualifiedName.0.as_ref()),
                    namespaceURI.0,
                    pfx_val,
                    Value::string(local),
                ],
            )?;
            _interp.assign_attr(el.clone(), "ownerDocument", self_val.0.clone())?;
            Ok(el)
        }

        #[py_name = "createTextNode"]
        fn create_text_node(#[positional_only] self_val: PyValueRef<'_>, data: PyStr<'_>) -> Result<Value> {
            let cls = minidom_cls(_interp, "Text")?;
            let t = call_pos(_interp, cls, &[])?;
            _interp.assign_attr(t.clone(), "data", Value::string(data.0.as_ref()))?;
            _interp.assign_attr(t.clone(), "ownerDocument", self_val.0.clone())?;
            Ok(t)
        }

        #[py_name = "createCDATASection"]
        fn create_cdata_section(#[positional_only] self_val: PyValueRef<'_>, data: PyStr<'_>) -> Result<Value> {
            let cls = minidom_cls(_interp, "CDATASection")?;
            let t = call_pos(_interp, cls, &[])?;
            _interp.assign_attr(t.clone(), "data", Value::string(data.0.as_ref()))?;
            _interp.assign_attr(t.clone(), "ownerDocument", self_val.0.clone())?;
            Ok(t)
        }

        #[py_name = "createComment"]
        fn create_comment(#[positional_only] self_val: PyValueRef<'_>, data: PyValue) -> Result<Value> {
            let cls = minidom_cls(_interp, "Comment")?;
            let c = call_pos(_interp, cls, &[data.0])?;
            _interp.assign_attr(c.clone(), "ownerDocument", self_val.0.clone())?;
            Ok(c)
        }

        #[py_name = "createProcessingInstruction"]
        fn create_processing_instruction(
            #[positional_only] self_val: PyValueRef<'_>,
            target: PyValue,
            data: PyValue,
        ) -> Result<Value> {
            let cls = minidom_cls(_interp, "ProcessingInstruction")?;
            let p = call_pos(_interp, cls, &[target.0, data.0])?;
            _interp.assign_attr(p.clone(), "ownerDocument", self_val.0.clone())?;
            Ok(p)
        }

        #[py_name = "createAttribute"]
        fn create_attribute(#[positional_only] self_val: PyValueRef<'_>, qName: PyStr<'_>) -> Result<Value> {
            let cls = minidom_cls(_interp, "Attr")?;
            let a = call_pos(_interp, cls, &[Value::string(qName.0.as_ref())])?;
            _interp.assign_attr(a.clone(), "ownerDocument", self_val.0.clone())?;
            _interp.assign_attr(a.clone(), "value", Value::string(""))?;
            Ok(a)
        }

        #[py_name = "createAttributeNS"]
        fn create_attribute_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            qualifiedName: PyStr<'_>,
        ) -> Result<Value> {
            let (pfx_opt, local) = split_qname_parts(&qualifiedName.0);
            let pfx_val = match pfx_opt {
                Some(p) => Value::string(p),
                None => Value::none(),
            };
            let cls = minidom_cls(_interp, "Attr")?;
            let a = call_pos(
                _interp,
                cls,
                &[
                    Value::string(qualifiedName.0.as_ref()),
                    namespaceURI.0,
                    Value::string(local),
                    pfx_val,
                ],
            )?;
            _interp.assign_attr(a.clone(), "ownerDocument", self_val.0.clone())?;
            _interp.assign_attr(a.clone(), "value", Value::string(""))?;
            Ok(a)
        }

        #[py_name = "createDocumentFragment"]
        fn create_document_fragment(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = minidom_cls(_interp, "DocumentFragment")?;
            let df = call_pos(_interp, cls, &[])?;
            _interp.assign_attr(df.clone(), "ownerDocument", self_val.0.clone())?;
            Ok(df)
        }

        #[py_name = "getElementsByTagName"]
        fn get_elements_by_tag_name(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let mut out = Vec::new();
            collect_elements_by_tag_name(_interp, self_val.0, &name.0, &mut out)?;
            new_node_list(_interp, out)
        }

        #[py_name = "getElementsByTagNameNS"]
        fn get_elements_by_tag_name_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            namespaceURI: PyValue,
            localName: PyStr<'_>,
        ) -> Result<Value> {
            let ns_opt = namespaceURI.0.as_str();
            let mut out = Vec::new();
            collect_elements_by_tag_name_ns(_interp, self_val.0, ns_opt, &localName.0, &mut out)?;
            new_node_list(_interp, out)
        }

        #[py_name = "getElementById"]
        fn get_element_by_id(#[positional_only] self_val: PyValueRef<'_>, id: PyStr<'_>) -> Result<Value> {
            let cache = _interp.get_attr(self_val.0, "_id_cache")?;
            let found = call_method(
                _interp,
                &cache,
                "get",
                &[Value::string(id.0.as_ref()), Value::none()],
            )?;
            if !found.is_none() {
                return Ok(found);
            }
            // Fallback scan for attributes marked isId
            let mut all_els = Vec::new();
            collect_elements_by_tag_name(_interp, self_val.0, "*", &mut all_els)?;
            for el in all_els {
                let attrs = _interp.get_attr(&el, "_attrs")?;
                if !attrs.is_none() {
                    let vals = call_method(_interp, &attrs, "values", &[])?;
                    for attr in _interp.collect_iterable(&vals)? {
                        let is_id = _interp
                            .get_attr(&attr, "isId")
                            .ok()
                            .and_then(|v| _interp.truthy_value(&v).ok())
                            .unwrap_or(false);
                        if is_id {
                            let av = _interp.get_attr(&attr, "value")?;
                            if av.as_str() == Some(id.0.as_ref()) {
                                return Ok(el);
                            }
                        }
                    }
                }
            }
            Ok(Value::none())
        }

        #[py_name = "importNode"]
        fn import_node(
            #[positional_only] self_val: PyValueRef<'_>,
            node: PyValue,
            deep: PyBool,
        ) -> Result<Value> {
            if get_node_type(_interp, &node.0) == 9 {
                return Err(raise_dom_exc(
                    _interp,
                    "NotSupportedErr",
                    "cannot import document nodes",
                ));
            }
            clone_node_impl(_interp, &node.0, deep.0, self_val.0)
        }

        #[py_name = "renameNode"]
        fn rename_node(
            #[positional_only] _self: PyValueRef<'_>,
            n: PyValue,
            namespaceURI: PyValue,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let nt = get_node_type(_interp, &n.0);
            let (pfx_opt, local) = split_qname_parts(&name.0);
            let pfx_val = match pfx_opt {
                Some(p) => Value::string(p),
                None => Value::none(),
            };
            if nt == 1 {
                _interp.assign_attr(n.0.clone(), "tagName", Value::string(name.0.as_ref()))?;
                _interp.assign_attr(n.0.clone(), "nodeName", Value::string(name.0.as_ref()))?;
                _interp.assign_attr(n.0.clone(), "namespaceURI", namespaceURI.0)?;
                _interp.assign_attr(n.0.clone(), "prefix", pfx_val)?;
                _interp.assign_attr(n.0.clone(), "_localName", Value::string(local))?;
                return Ok(n.0);
            }
            if nt == 2 {
                _interp.assign_attr(n.0.clone(), "_name", Value::string(name.0.as_ref()))?;
                _interp.assign_attr(n.0.clone(), "namespaceURI", namespaceURI.0)?;
                _interp.assign_attr(n.0.clone(), "_prefix", pfx_val)?;
                _interp.assign_attr(n.0.clone(), "_localName", Value::string(local))?;
                return Ok(n.0);
            }
            Err(raise_dom_exc(
                _interp,
                "NotSupportedErr",
                "renameNode only supports Element and Attr nodes",
            ))
        }

        fn writexml(
            #[positional_only] self_val: PyValueRef<'_>,
            writer: PyValue,
            #[default(OptionalArg::Omitted)] indent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] addindent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] newl: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] standalone: OptValueRef<'_>,
        ) -> Result<Value> {
            let ind = indent.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let add = addindent.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let nl = newl.as_value().and_then(|v| v.as_str()).unwrap_or("");
            let mut decls = Vec::new();
            if let Some(enc_v) = encoding.as_value()
                && let Some(enc) = enc_v.as_str()
            {
                decls.push(format!("encoding=\"{enc}\""));
            }
            if let Some(st_v) = standalone.as_value()
                && !st_v.is_none()
            {
                let sv = if _interp.truthy_value(st_v)? { "yes" } else { "no" };
                decls.push(format!("standalone=\"{sv}\""));
            }
            let header = if decls.is_empty() {
                format!("<?xml version=\"1.0\" ?>{nl}")
            } else {
                format!("<?xml version=\"1.0\" {}?>{nl}", decls.join(" "))
            };
            write_str(_interp, &writer.0, &header)?;

            let children = _interp.get_attr(self_val.0, "childNodes")?;
            for child in _interp.collect_iterable(&children)? {
                call_method(
                    _interp,
                    &child,
                    "writexml",
                    &[
                        writer.0.clone(),
                        Value::string(ind),
                        Value::string(add),
                        Value::string(nl),
                    ],
                )?;
            }
            Ok(Value::none())
        }

        #[py_name = "cloneNode"]
        fn clone_node(#[positional_only] self_val: PyValueRef<'_>, deep: PyBool) -> Result<Value> {
            clone_node_impl(_interp, self_val.0, deep.0, self_val.0)
        }
    }

    #[py_name = "getDOMImplementation"]
    fn get_dom_implementation() -> Result<Value> {
        let cls = minidom_cls(_interp, "DOMImplementation")?;
        call_pos(_interp, cls, &[])
    }

    fn parse(
        file: PyValue,
        #[default(OptionalArg::Omitted)] parser: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] bufsize: OptValueRef<'_>,
    ) -> Result<Value> {
        let has_parser = parser.as_value().is_some_and(|v| !v.is_none());
        if !has_parser && bufsize.as_value().is_none_or(Value::is_none) {
            let eb = _interp.load_module("xml.dom.expatbuilder")?;
            let f = _interp.get_attr(&eb, "parse")?;
            return call_pos(_interp, f, &[file.0]);
        }
        let pd = _interp.load_module("xml.dom.pulldom")?;
        let pd_parse = _interp.get_attr(&pd, "parse")?;
        let p_arg = parser.as_value().cloned().unwrap_or_else(Value::none);
        let b_arg = bufsize.as_value().cloned().unwrap_or_else(Value::none);
        let events = call_pos(_interp, pd_parse, &[file.0, p_arg, b_arg])?;
        let mut doc = Value::none();
        loop {
            let ev = call_method(_interp, &events, "getEvent", &[])?;
            if ev.is_none() {
                break;
            }
            let parts = _interp.collect_iterable(&ev)?;
            if parts.len() == 2 && parts[0].as_str() == Some("START_DOCUMENT") {
                doc = parts[1].clone();
                call_method(_interp, &events, "expandNode", std::slice::from_ref(&doc))?;
                break;
            }
        }
        Ok(doc)
    }

    #[py_name = "parseString"]
    fn parse_string(
        string: PyValue,
        #[default(OptionalArg::Omitted)] parser: OptValueRef<'_>,
    ) -> Result<Value> {
        let has_parser = parser.as_value().is_some_and(|v| !v.is_none());
        if !has_parser {
            let eb = _interp.load_module("xml.dom.expatbuilder")?;
            let f = _interp.get_attr(&eb, "parseString")?;
            return call_pos(_interp, f, &[string.0]);
        }
        let pd = _interp.load_module("xml.dom.pulldom")?;
        let pd_parse = _interp.get_attr(&pd, "parseString")?;
        let p_arg = parser.as_value().cloned().unwrap_or_else(Value::none);
        let events = call_pos(_interp, pd_parse, &[string.0, p_arg])?;
        let mut doc = Value::none();
        loop {
            let ev = call_method(_interp, &events, "getEvent", &[])?;
            if ev.is_none() {
                break;
            }
            let parts = _interp.collect_iterable(&ev)?;
            if parts.len() == 2 && parts[0].as_str() == Some("START_DOCUMENT") {
                doc = parts[1].clone();
                call_method(_interp, &events, "expandNode", std::slice::from_ref(&doc))?;
                break;
            }
        }
        Ok(doc)
    }
}
