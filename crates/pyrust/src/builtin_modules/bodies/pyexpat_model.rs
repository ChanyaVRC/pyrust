// `pyexpat.model` module — 100% native-Rust Python 3.12 Expat content-model
// constants (issue #3260).
//
// Reference: <https://docs.python.org/3/library/pyexpat.html#module-xml.parsers.expat.model>

use pyrust_derive::pyrust_module;

use crate::value::Value;

pyrust_module! {
    constants {
        "XML_CTYPE_EMPTY" => Value::int(1),
        "XML_CTYPE_ANY" => Value::int(2),
        "XML_CTYPE_MIXED" => Value::int(3),
        "XML_CTYPE_NAME" => Value::int(4),
        "XML_CTYPE_CHOICE" => Value::int(5),
        "XML_CTYPE_SEQ" => Value::int(6),
        "XML_CQUANT_NONE" => Value::int(0),
        "XML_CQUANT_OPT" => Value::int(1),
        "XML_CQUANT_REP" => Value::int(2),
        "XML_CQUANT_PLUS" => Value::int(3),
    }
}
