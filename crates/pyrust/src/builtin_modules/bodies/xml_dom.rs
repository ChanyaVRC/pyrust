// `xml.dom` package module — 100% native-Rust Python 3.12 W3C DOM Level 2
// core constants, `Node`, `DOMException` hierarchy, `UserDataHandler`,
// namespace URIs, and DOM implementation registry exports (issue #3260).
//
// Reference: <https://docs.python.org/3/library/xml.dom.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValueRef;
use crate::interpreter::{ExpandedCallArg, Interpreter, values_are_identical};
use crate::value::{PyModule, Value};

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let domreg_mod = interp.load_module("xml.dom.domreg")?;
    let mut m = module.borrow_mut();
    m.insert_attr("domreg".to_string(), domreg_mod.clone());
    if let Ok(f) = interp.get_attr(&domreg_mod, "registerDOMImplementation") {
        m.insert_attr("registerDOMImplementation".to_string(), f);
    }
    if let Ok(f) = interp.get_attr(&domreg_mod, "getDOMImplementation") {
        m.insert_attr("getDOMImplementation".to_string(), f);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.dom"),
        "INDEX_SIZE_ERR" => Value::int(1),
        "DOMSTRING_SIZE_ERR" => Value::int(2),
        "HIERARCHY_REQUEST_ERR" => Value::int(3),
        "WRONG_DOCUMENT_ERR" => Value::int(4),
        "INVALID_CHARACTER_ERR" => Value::int(5),
        "NO_DATA_ALLOWED_ERR" => Value::int(6),
        "NO_MODIFICATION_ALLOWED_ERR" => Value::int(7),
        "NOT_FOUND_ERR" => Value::int(8),
        "NOT_SUPPORTED_ERR" => Value::int(9),
        "INUSE_ATTRIBUTE_ERR" => Value::int(10),
        "INVALID_STATE_ERR" => Value::int(11),
        "SYNTAX_ERR" => Value::int(12),
        "INVALID_MODIFICATION_ERR" => Value::int(13),
        "NAMESPACE_ERR" => Value::int(14),
        "INVALID_ACCESS_ERR" => Value::int(15),
        "VALIDATION_ERR" => Value::int(16),
        "XML_NAMESPACE" => Value::string("http://www.w3.org/XML/1998/namespace"),
        "XMLNS_NAMESPACE" => Value::string("http://www.w3.org/2000/xmlns/"),
        "XHTML_NAMESPACE" => Value::string("http://www.w3.org/1999/xhtml"),
        "EMPTY_NAMESPACE" => Value::none(),
        "EMPTY_PREFIX" => Value::none(),
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
        }
    }

    class DOMException("Exception") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] _kw: indexmap::IndexMap<String, Value>,
        ) -> Result<Value> {
            let dom_mod = _interp.load_module("xml.dom")?;
            let dom_exc_cls = _interp.get_attr(&dom_mod, "DOMException")?;
            let self_cls = _interp.get_attr(self_val.0, "__class__")?;
            if values_are_identical(&self_cls, &dom_exc_cls) {
                return Err(PyError::named(
                    "RuntimeError",
                    "DOMException should not be instantiated directly",
                ));
            }
            _interp.assign_attr(self_val.0.clone(), "args", Value::tuple(args.to_vec()))?;
            Ok(Value::none())
        }

        fn _get_code(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "code")
        }
    }

    class IndexSizeErr("DOMException") {
        constants { "code" => Value::int(1), }
    }
    class DomstringSizeErr("DOMException") {
        constants { "code" => Value::int(2), }
    }
    class HierarchyRequestErr("DOMException") {
        constants { "code" => Value::int(3), }
    }
    class WrongDocumentErr("DOMException") {
        constants { "code" => Value::int(4), }
    }
    class InvalidCharacterErr("DOMException") {
        constants { "code" => Value::int(5), }
    }
    class NoDataAllowedErr("DOMException") {
        constants { "code" => Value::int(6), }
    }
    class NoModificationAllowedErr("DOMException") {
        constants { "code" => Value::int(7), }
    }
    class NotFoundErr("DOMException") {
        constants { "code" => Value::int(8), }
    }
    class NotSupportedErr("DOMException") {
        constants { "code" => Value::int(9), }
    }
    class InuseAttributeErr("DOMException") {
        constants { "code" => Value::int(10), }
    }
    class InvalidStateErr("DOMException") {
        constants { "code" => Value::int(11), }
    }
    class SyntaxErr("DOMException") {
        constants { "code" => Value::int(12), }
    }
    class InvalidModificationErr("DOMException") {
        constants { "code" => Value::int(13), }
    }
    class NamespaceErr("DOMException") {
        constants { "code" => Value::int(14), }
    }
    class InvalidAccessErr("DOMException") {
        constants { "code" => Value::int(15), }
    }
    class ValidationErr("DOMException") {
        constants { "code" => Value::int(16), }
    }

    class UserDataHandler {
        constants {
            "NODE_CLONED" => Value::int(1),
            "NODE_IMPORTED" => Value::int(2),
            "NODE_DELETED" => Value::int(3),
            "NODE_RENAMED" => Value::int(4),
        }
    }
}
