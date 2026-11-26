// `xml.dom.pulldom` module — 100% native-Rust Python 3.12 pull-based DOM event
// stream parser (issue #3260).
//
// Provides `parse`, `parseString`, `DOMEventStream`, `PullDOM`, `ErrorHandler`,
// `SAX2DOM`, `START_ELEMENT`, `END_ELEMENT`, `COMMENT`, `START_DOCUMENT`,
// `END_DOCUMENT`, `PROCESSING_INSTRUCTION`, `IGNORABLE_WHITESPACE`,
// `CHARACTERS`, and `default_bufsize`.
//
// Reference: <https://docs.python.org/3/library/xml.dom.pulldom.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, values_are_identical};
use crate::value::{PyDict, PyModule, Value, ValueKind};

const DEFAULT_BUFSIZE: i64 = (1 << 14) - 20;

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

fn push_event(
    interp: &mut Interpreter,
    self_val: &Value,
    ev_type: &str,
    node: Value,
) -> Result<()> {
    let queue = interp.get_attr(self_val, "_event_queue")?;
    call_method(
        interp,
        &queue,
        "append",
        &[Value::tuple(vec![Value::string(ev_type), node])],
    )?;
    Ok(())
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let h_mod = interp.load_module("xml.sax.handler")?;
    let ch_cls = interp.get_attr(&h_mod, "ContentHandler")?;
    let pd_val = module.borrow().get_attr_value("PullDOM");
    if let (Some(pd), ValueKind::PyClass(bc)) = (pd_val.as_ref(), ch_cls.kind())
        && let ValueKind::PyClass(pdc) = pd.kind()
    {
        link_class_bases(pdc, vec![Rc::clone(bc)]);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.dom"),
        "START_ELEMENT" => Value::string("START_ELEMENT"),
        "END_ELEMENT" => Value::string("END_ELEMENT"),
        "COMMENT" => Value::string("COMMENT"),
        "START_DOCUMENT" => Value::string("START_DOCUMENT"),
        "END_DOCUMENT" => Value::string("END_DOCUMENT"),
        "PROCESSING_INSTRUCTION" => Value::string("PROCESSING_INSTRUCTION"),
        "IGNORABLE_WHITESPACE" => Value::string("IGNORABLE_WHITESPACE"),
        "CHARACTERS" => Value::string("CHARACTERS"),
        "default_bufsize" => Value::int(DEFAULT_BUFSIZE),
    }

    class ErrorHandler {
        fn warning(#[positional_only] _self: PyValueRef<'_>, exception: PyValue) -> Result<Value> {
            let msg = _interp.render_value_as_str(&exception.0)?;
            if let Ok(sys_m) = _interp.load_module("sys")
                && let Ok(stderr) = _interp.get_attr(&sys_m, "stderr")
            {
                let _ = call_method(_interp, &stderr, "write", &[Value::string(format!("{msg}\n"))]);
            }
            Ok(Value::none())
        }

        fn error(#[positional_only] _self: PyValueRef<'_>, exception: PyValue) -> Result<Value> {
            Err(PyError::raised(exception.0))
        }

        #[py_name = "fatalError"]
        fn fatal_error(#[positional_only] _self: PyValueRef<'_>, exception: PyValue) -> Result<Value> {
            Err(PyError::raised(exception.0))
        }
    }

    class PullDOM {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] documentFactory: OptValueRef<'_>,
        ) -> Result<Value> {
            let h_mod = _interp.load_module("xml.sax.handler")?;
            let ch_cls = _interp.get_attr(&h_mod, "ContentHandler")?;
            let ch_init = _interp.get_attr(&ch_cls, "__init__")?;
            call_pos(_interp, ch_init, std::slice::from_ref(self_val.0))?;

            let df = documentFactory.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "documentFactory", df)?;
            _interp.assign_attr(self_val.0.clone(), "document", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "_event_queue", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "elementStack", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "pending_events", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "_ns_contexts", Value::list(vec![Value::dict(PyDict::default())]))?;
            _interp.assign_attr(self_val.0.clone(), "_current_context", Value::dict(PyDict::default()))?;
            _interp.assign_attr(self_val.0.clone(), "_attach_children", Value::bool_(false))?;
            Ok(Value::none())
        }

        fn pop(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stack = _interp.get_attr(self_val.0, "elementStack")?;
            call_method(_interp, &stack, "pop", &[])
        }

        fn clear(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "document", Value::none())?;
            Ok(Value::none())
        }

        #[py_name = "startPrefixMapping"]
        fn start_prefix_mapping(
            #[positional_only] self_val: PyValueRef<'_>,
            prefix: PyValue,
            uri: PyValue,
        ) -> Result<Value> {
            let ctxs = _interp.get_attr(self_val.0, "_ns_contexts")?;
            let cur = _interp.get_attr(self_val.0, "_current_context")?;
            let cp = call_method(_interp, &cur, "copy", &[])?;
            call_method(_interp, &ctxs, "append", &[cp])?;
            call_method(_interp, &cur, "__setitem__", &[uri.0, prefix.0])?;
            Ok(Value::none())
        }

        #[py_name = "endPrefixMapping"]
        fn end_prefix_mapping(
            #[positional_only] self_val: PyValueRef<'_>,
            _prefix: PyValue,
        ) -> Result<Value> {
            let ctxs = _interp.get_attr(self_val.0, "_ns_contexts")?;
            let prev = call_method(_interp, &ctxs, "pop", &[])?;
            _interp.assign_attr(self_val.0.clone(), "_current_context", prev)?;
            Ok(Value::none())
        }

        #[py_name = "startDocument"]
        fn start_document(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut df = _interp.get_attr(self_val.0, "documentFactory")?;
            if df.is_none() {
                let md = _interp.load_module("xml.dom.minidom")?;
                df = call_method(_interp, &md, "getDOMImplementation", &[])?;
                _interp.assign_attr(self_val.0.clone(), "documentFactory", df.clone())?;
            }
            let doc = call_method(
                _interp,
                &df,
                "createDocument",
                &[Value::none(), Value::none(), Value::none()],
            )?;
            _interp.assign_attr(self_val.0.clone(), "document", doc.clone())?;
            let stack = _interp.get_attr(self_val.0, "elementStack")?;
            call_method(_interp, &stack, "clear", &[])?;
            call_method(_interp, &stack, "append", std::slice::from_ref(&doc))?;
            push_event(_interp, self_val.0, "START_DOCUMENT", doc)?;
            Ok(Value::none())
        }

        #[py_name = "endDocument"]
        fn end_document(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            push_event(_interp, self_val.0, "END_DOCUMENT", doc)?;
            Ok(Value::none())
        }

        #[py_name = "startElement"]
        fn start_element(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            attrs: PyValue,
        ) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            let node = call_method(_interp, &doc, "createElement", &[Value::string(name.0.as_ref())])?;
            let items = call_method(_interp, &attrs.0, "items", &[])?;
            for pair in _interp.collect_iterable(&items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    call_method(_interp, &node, "setAttribute", &[kv[0].clone(), kv[1].clone()])?;
                }
            }
            let attach = _interp
                .get_attr(self_val.0, "_attach_children")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            let stack = _interp.get_attr(self_val.0, "elementStack")?;
            if attach {
                let items_s = _interp.collect_iterable(&stack)?;
                if let Some(parent) = items_s.last() {
                    call_method(_interp, parent, "appendChild", std::slice::from_ref(&node))?;
                }
            }
            call_method(_interp, &stack, "append", std::slice::from_ref(&node))?;
            push_event(_interp, self_val.0, "START_ELEMENT", node)?;
            Ok(Value::none())
        }

        #[py_name = "endElement"]
        fn end_element(
            #[positional_only] self_val: PyValueRef<'_>,
            _name: PyValue,
        ) -> Result<Value> {
            let stack = _interp.get_attr(self_val.0, "elementStack")?;
            let node = call_method(_interp, &stack, "pop", &[])?;
            push_event(_interp, self_val.0, "END_ELEMENT", node)?;
            Ok(Value::none())
        }

        #[py_name = "startElementNS"]
        fn start_element_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            tagName: PyValue,
            attrs: PyValue,
        ) -> Result<Value> {
            let parts = _interp.collect_iterable(&name.0)?;
            let uri = parts.first().cloned().unwrap_or_else(Value::none);
            let local = parts.get(1).cloned().unwrap_or_else(|| Value::string(""));
            let qname = if !tagName.0.is_none() {
                tagName.0
            } else if !uri.is_none() {
                let ctx = _interp.get_attr(self_val.0, "_current_context")?;
                let pfx = call_method(_interp, &ctx, "get", &[uri.clone(), Value::none()])?;
                if let Some(p) = pfx.as_str()
                    && !p.is_empty()
                {
                    Value::string(format!("{p}:{}", local.as_str().unwrap_or("")))
                } else {
                    local
                }
            } else {
                local
            };
            let doc = _interp.get_attr(self_val.0, "document")?;
            let node = call_method(_interp, &doc, "createElementNS", &[uri, qname])?;
            let items = call_method(_interp, &attrs.0, "items", &[])?;
            for pair in _interp.collect_iterable(&items)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() == 2 {
                    let ak = _interp.collect_iterable(&kv[0])?;
                    let a_uri = ak.first().cloned().unwrap_or_else(Value::none);
                    let a_qname = call_method(
                        _interp,
                        &attrs.0,
                        "getQNameByName",
                        std::slice::from_ref(&kv[0]),
                    )
                    .unwrap_or_else(|_| ak.get(1).cloned().unwrap_or_else(|| Value::string("")));
                    call_method(_interp, &node, "setAttributeNS", &[a_uri, a_qname, kv[1].clone()])?;
                }
            }
            let attach = _interp
                .get_attr(self_val.0, "_attach_children")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            let stack = _interp.get_attr(self_val.0, "elementStack")?;
            if attach {
                let items_s = _interp.collect_iterable(&stack)?;
                if let Some(parent) = items_s.last() {
                    call_method(_interp, parent, "appendChild", std::slice::from_ref(&node))?;
                }
            }
            call_method(_interp, &stack, "append", std::slice::from_ref(&node))?;
            push_event(_interp, self_val.0, "START_ELEMENT", node)?;
            Ok(Value::none())
        }

        #[py_name = "endElementNS"]
        fn end_element_ns(
            #[positional_only] self_val: PyValueRef<'_>,
            _name: PyValue,
            _tagName: PyValue,
        ) -> Result<Value> {
            let stack = _interp.get_attr(self_val.0, "elementStack")?;
            let node = call_method(_interp, &stack, "pop", &[])?;
            push_event(_interp, self_val.0, "END_ELEMENT", node)?;
            Ok(Value::none())
        }

        fn characters(
            #[positional_only] self_val: PyValueRef<'_>,
            chars: PyStr<'_>,
        ) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            let node = call_method(
                _interp,
                &doc,
                "createTextNode",
                &[Value::string(chars.0.as_ref())],
            )?;
            let attach = _interp
                .get_attr(self_val.0, "_attach_children")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if attach {
                let stack = _interp.get_attr(self_val.0, "elementStack")?;
                let items_s = _interp.collect_iterable(&stack)?;
                if let Some(parent) = items_s.last() {
                    call_method(_interp, parent, "appendChild", std::slice::from_ref(&node))?;
                }
            }
            push_event(_interp, self_val.0, "CHARACTERS", node)?;
            Ok(Value::none())
        }

        #[py_name = "ignorableWhitespace"]
        fn ignorable_whitespace(
            #[positional_only] self_val: PyValueRef<'_>,
            chars: PyStr<'_>,
        ) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            let node = call_method(
                _interp,
                &doc,
                "createTextNode",
                &[Value::string(chars.0.as_ref())],
            )?;
            let attach = _interp
                .get_attr(self_val.0, "_attach_children")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if attach {
                let stack = _interp.get_attr(self_val.0, "elementStack")?;
                let items_s = _interp.collect_iterable(&stack)?;
                if let Some(parent) = items_s.last() {
                    call_method(_interp, parent, "appendChild", std::slice::from_ref(&node))?;
                }
            }
            push_event(_interp, self_val.0, "IGNORABLE_WHITESPACE", node)?;
            Ok(Value::none())
        }

        #[py_name = "processingInstruction"]
        fn processing_instruction(
            #[positional_only] self_val: PyValueRef<'_>,
            target: PyValue,
            data: PyValue,
        ) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            let node = call_method(_interp, &doc, "createProcessingInstruction", &[target.0, data.0])?;
            let attach = _interp
                .get_attr(self_val.0, "_attach_children")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if attach {
                let stack = _interp.get_attr(self_val.0, "elementStack")?;
                let items_s = _interp.collect_iterable(&stack)?;
                if let Some(parent) = items_s.last() {
                    call_method(_interp, parent, "appendChild", std::slice::from_ref(&node))?;
                }
            }
            push_event(_interp, self_val.0, "PROCESSING_INSTRUCTION", node)?;
            Ok(Value::none())
        }

        fn comment(#[positional_only] self_val: PyValueRef<'_>, s: PyValue) -> Result<Value> {
            let doc = _interp.get_attr(self_val.0, "document")?;
            let node = call_method(_interp, &doc, "createComment", &[s.0])?;
            let attach = _interp
                .get_attr(self_val.0, "_attach_children")
                .ok()
                .and_then(|v| _interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if attach {
                let stack = _interp.get_attr(self_val.0, "elementStack")?;
                let items_s = _interp.collect_iterable(&stack)?;
                if let Some(parent) = items_s.last() {
                    call_method(_interp, parent, "appendChild", std::slice::from_ref(&node))?;
                }
            }
            push_event(_interp, self_val.0, "COMMENT", node)?;
            Ok(Value::none())
        }
    }

    class SAX2DOM("PullDOM") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] documentFactory: OptValueRef<'_>,
        ) -> Result<Value> {
            let pd_mod = _interp.load_module("xml.dom.pulldom")?;
            let pd_cls = _interp.get_attr(&pd_mod, "PullDOM")?;
            let pd_init = _interp.get_attr(&pd_cls, "__init__")?;
            let df = documentFactory.as_value().cloned().unwrap_or_else(Value::none);
            call_pos(_interp, pd_init, &[self_val.0.clone(), df])?;
            _interp.assign_attr(self_val.0.clone(), "_attach_children", Value::bool_(true))?;
            Ok(Value::none())
        }
    }

    class DOMEventStream {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            stream: PyValue,
            parser: PyValue,
            bufsize: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "stream", stream.0)?;
            _interp.assign_attr(self_val.0.clone(), "parser", parser.0)?;
            _interp.assign_attr(self_val.0.clone(), "bufsize", bufsize.0)?;
            _interp.assign_attr(self_val.0.clone(), "_eof", Value::bool_(false))?;
            call_method(_interp, self_val.0, "reset", &[])?;
            Ok(Value::none())
        }

        fn reset(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pd_mod = _interp.load_module("xml.dom.pulldom")?;
            let pd_cls = _interp.get_attr(&pd_mod, "PullDOM")?;
            let pulldom = call_pos(_interp, pd_cls, &[])?;
            let parser = _interp.get_attr(self_val.0, "parser")?;
            let h_mod = _interp.load_module("xml.sax.handler")?;
            let feat_ns = _interp.get_attr(&h_mod, "feature_namespaces")?;
            let _ = call_method(_interp, &parser, "setFeature", &[feat_ns, Value::int(1)]);
            call_method(_interp, &parser, "setContentHandler", std::slice::from_ref(&pulldom))?;
            _interp.assign_attr(self_val.0.clone(), "pulldom", pulldom)?;
            _interp.assign_attr(self_val.0.clone(), "_eof", Value::bool_(false))?;
            Ok(Value::none())
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ev = call_method(_interp, self_val.0, "getEvent", &[])?;
            if ev.is_none() {
                return Err(PyError::named("StopIteration", ""));
            }
            Ok(ev)
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
            call_method(_interp, self_val.0, "clear", &[])?;
            Ok(Value::none())
        }

        #[py_name = "getEvent"]
        fn get_event(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pulldom = _interp.get_attr(self_val.0, "pulldom")?;
            let queue = _interp.get_attr(&pulldom, "_event_queue")?;
            let parser = _interp.get_attr(self_val.0, "parser")?;
            let stream = _interp.get_attr(self_val.0, "stream")?;
            let bufsize = _interp.get_attr(self_val.0, "bufsize")?;

            loop {
                let qlen = call_pos(_interp, Value::builtin_function("len"), std::slice::from_ref(&queue))?
                    .as_int()
                    .unwrap_or(0);
                if qlen > 0 {
                    let ev = call_method(_interp, &queue, "pop", &[Value::int(0)])?;
                    let parts = _interp.collect_iterable(&ev)?;
                    // Skip END_DOCUMENT in iteration to match CPython's DOMEventStream
                    if parts.first().and_then(|v| v.as_str()) == Some("END_DOCUMENT") {
                        return Ok(Value::none());
                    }
                    return Ok(ev);
                }
                let eof = _interp.get_attr(self_val.0, "_eof")?;
                if _interp.truthy_value(&eof)? {
                    return Ok(Value::none());
                }
                let buf = call_method(_interp, &stream, "read", std::slice::from_ref(&bufsize))?;
                let empty = match buf.kind() {
                    ValueKind::Bytes(b) => b.is_empty(),
                    ValueKind::Str(s) => s.is_empty(),
                    _ => !_interp.truthy_value(&buf)?,
                };
                if empty {
                    _interp.assign_attr(self_val.0.clone(), "_eof", Value::bool_(true))?;
                    call_method(_interp, &parser, "close", &[])?;
                } else {
                    call_method(_interp, &parser, "feed", &[buf])?;
                }
            }
        }

        #[py_name = "expandNode"]
        fn expand_node(#[positional_only] self_val: PyValueRef<'_>, node: PyValue) -> Result<Value> {
            let mut parents: Vec<Value> = vec![node.0.clone()];
            let is_doc = _interp
                .get_attr(&node.0, "nodeType")
                .ok()
                .and_then(|v| v.as_int())
                == Some(9);
            loop {
                let ev = call_method(_interp, self_val.0, "getEvent", &[])?;
                if ev.is_none() {
                    break;
                }
                let parts = _interp.collect_iterable(&ev)?;
                if parts.len() != 2 {
                    break;
                }
                let ev_type = parts[0].as_str().unwrap_or("");
                let cur_node = parts[1].clone();
                match ev_type {
                    "START_ELEMENT" => {
                        if let Some(p) = parents.last() {
                            call_method(_interp, p, "appendChild", std::slice::from_ref(&cur_node))?;
                        }
                        parents.push(cur_node);
                    }
                    "END_ELEMENT" => {
                        if values_are_identical(&cur_node, &node.0) {
                            break;
                        }
                        parents.pop();
                        if !is_doc && parents.is_empty() {
                            break;
                        }
                    }
                    "CHARACTERS" | "IGNORABLE_WHITESPACE" | "COMMENT" | "PROCESSING_INSTRUCTION" => {
                        if let Some(p) = parents.last() {
                            call_method(_interp, p, "appendChild", &[cur_node])?;
                        }
                    }
                    "END_DOCUMENT" => break,
                    _ => {}
                }
            }
            Ok(Value::none())
        }

        fn clear(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            if let Ok(pd) = _interp.get_attr(self_val.0, "pulldom")
                && !pd.is_none()
            {
                let _ = call_method(_interp, &pd, "clear", &[]);
            }
            _interp.assign_attr(self_val.0.clone(), "pulldom", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "parser", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "stream", Value::none())?;
            Ok(Value::none())
        }
    }

    fn parse(
        stream_or_string: PyValue,
        #[default(OptionalArg::Omitted)] parser: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] bufsize: OptValueRef<'_>,
    ) -> Result<Value> {
        let bs = match bufsize.as_value() {
            Some(v) if !v.is_none() => v.clone(),
            _ => Value::int(DEFAULT_BUFSIZE),
        };
        let stream = if let Some(path_s) = stream_or_string.0.as_str() {
            call_pos(
                _interp,
                Value::builtin_function("open"),
                &[Value::string(path_s), Value::string("rb")],
            )?
        } else {
            stream_or_string.0
        };
        let p = match parser.as_value() {
            Some(v) if !v.is_none() => v.clone(),
            _ => {
                let sax = _interp.load_module("xml.sax")?;
                let mp = _interp.get_attr(&sax, "make_parser")?;
                call_pos(_interp, mp, &[])?
            }
        };
        let pd_mod = _interp.load_module("xml.dom.pulldom")?;
        let cls = _interp.get_attr(&pd_mod, "DOMEventStream")?;
        call_pos(_interp, cls, &[stream, p, bs])
    }

    #[py_name = "parseString"]
    fn parse_string(
        string: PyValue,
        #[default(OptionalArg::Omitted)] parser: OptValueRef<'_>,
    ) -> Result<Value> {
        let io_mod = _interp.load_module("io")?;
        let str_len = string.0.as_str().map(|s| s.len() as i64);
        let (stream, bufsize) = if let Some(len) = str_len {
            let sio_cls = _interp.get_attr(&io_mod, "StringIO")?;
            let sio = call_pos(_interp, sio_cls, &[string.0])?;
            (sio, Value::int(len.max(1)))
        } else {
            let len = call_pos(
                _interp,
                Value::builtin_function("len"),
                std::slice::from_ref(&string.0),
            )?
            .as_int()
            .unwrap_or(DEFAULT_BUFSIZE);
            let bio_cls = _interp.get_attr(&io_mod, "BytesIO")?;
            let bio = call_pos(_interp, bio_cls, &[string.0])?;
            (bio, Value::int(len.max(1)))
        };
        let p = match parser.as_value() {
            Some(v) if !v.is_none() => v.clone(),
            _ => {
                let sax = _interp.load_module("xml.sax")?;
                let mp = _interp.get_attr(&sax, "make_parser")?;
                call_pos(_interp, mp, &[])?
            }
        };
        let pd_mod = _interp.load_module("xml.dom.pulldom")?;
        let cls = _interp.get_attr(&pd_mod, "DOMEventStream")?;
        call_pos(_interp, cls, &[stream, p, bufsize])
    }
}
