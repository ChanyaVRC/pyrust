// `xml` package module — exposes `xml.etree` (issue #3210).
//
// Follows PyRust's built-in package/submodule pattern (`urllib`, `html`,
// `importlib`): declaring `"etree"` as a `PyModule` constant lets
// `Interpreter::load_module` stitch it to the cached `sys.modules["xml.etree"]`
// object so `import xml`, `import xml.etree`, `from xml import etree`, and
// `import xml.etree.ElementTree` all share exact module identity.
//
// Reference: <https://docs.python.org/3/library/xml.html>

use crate::value::Value;
use pyrust_derive::pyrust_module;

pyrust_module! {
    constants {
        "__package__" => Value::string("xml"),
        "__all__" => Value::list(vec![
            Value::string("dom"),
            Value::string("parsers"),
            Value::string("sax"),
            Value::string("etree"),
        ]),
        "etree" => super::xml_etree::module(),
    }
}
