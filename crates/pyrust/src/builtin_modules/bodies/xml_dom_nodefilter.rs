// `xml.dom.NodeFilter` module — 100% native-Rust Python 3.12 W3C DOM Level 2
// Traversal `NodeFilter` interface (issue #3260).
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/xml/dom/NodeFilter.py>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyValue, PyValueRef};
use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.dom"),
    }

    class NodeFilter {
        constants {
            "FILTER_ACCEPT" => Value::int(1),
            "FILTER_REJECT" => Value::int(2),
            "FILTER_SKIP" => Value::int(3),
            "SHOW_ALL" => Value::int(0xFFFF_FFFF_i64),
            "SHOW_ELEMENT" => Value::int(0x0000_0001),
            "SHOW_ATTRIBUTE" => Value::int(0x0000_0002),
            "SHOW_TEXT" => Value::int(0x0000_0004),
            "SHOW_CDATA_SECTION" => Value::int(0x0000_0008),
            "SHOW_ENTITY_REFERENCE" => Value::int(0x0000_0010),
            "SHOW_ENTITY" => Value::int(0x0000_0020),
            "SHOW_PROCESSING_INSTRUCTION" => Value::int(0x0000_0040),
            "SHOW_COMMENT" => Value::int(0x0000_0080),
            "SHOW_DOCUMENT" => Value::int(0x0000_0100),
            "SHOW_DOCUMENT_TYPE" => Value::int(0x0000_0200),
            "SHOW_DOCUMENT_FRAGMENT" => Value::int(0x0000_0400),
            "SHOW_NOTATION" => Value::int(0x0000_0800),
        }

        #[py_name = "acceptNode"]
        fn accept_node(#[positional_only] _self: PyValueRef<'_>, _node: PyValue) -> Result<Value> {
            Err(PyError::named("NotImplementedError", ""))
        }
    }
}
