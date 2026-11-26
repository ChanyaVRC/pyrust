// `pyexpat.errors` module — 100% native-Rust Python 3.12 Expat error code and
// message registry (issue #3260).
//
// Reference: <https://docs.python.org/3/library/pyexpat.html#module-xml.parsers.expat.errors>

use pyrust_derive::pyrust_module;

use crate::value::{PyDict, PyKey, Value};

/// Complete CPython 3.12.3 / Expat 2.6.0 error table: `(const_name, code, message)`.
pub(crate) const ERROR_TABLE: &[(&str, i64, &str)] = &[
    ("XML_ERROR_NO_MEMORY", 1, "out of memory"),
    ("XML_ERROR_SYNTAX", 2, "syntax error"),
    ("XML_ERROR_NO_ELEMENTS", 3, "no element found"),
    (
        "XML_ERROR_INVALID_TOKEN",
        4,
        "not well-formed (invalid token)",
    ),
    ("XML_ERROR_UNCLOSED_TOKEN", 5, "unclosed token"),
    ("XML_ERROR_PARTIAL_CHAR", 6, "partial character"),
    ("XML_ERROR_TAG_MISMATCH", 7, "mismatched tag"),
    ("XML_ERROR_DUPLICATE_ATTRIBUTE", 8, "duplicate attribute"),
    (
        "XML_ERROR_JUNK_AFTER_DOC_ELEMENT",
        9,
        "junk after document element",
    ),
    (
        "XML_ERROR_PARAM_ENTITY_REF",
        10,
        "illegal parameter entity reference",
    ),
    ("XML_ERROR_UNDEFINED_ENTITY", 11, "undefined entity"),
    (
        "XML_ERROR_RECURSIVE_ENTITY_REF",
        12,
        "recursive entity reference",
    ),
    ("XML_ERROR_ASYNC_ENTITY", 13, "asynchronous entity"),
    (
        "XML_ERROR_BAD_CHAR_REF",
        14,
        "reference to invalid character number",
    ),
    (
        "XML_ERROR_BINARY_ENTITY_REF",
        15,
        "reference to binary entity",
    ),
    (
        "XML_ERROR_ATTRIBUTE_EXTERNAL_ENTITY_REF",
        16,
        "reference to external entity in attribute",
    ),
    (
        "XML_ERROR_MISPLACED_XML_PI",
        17,
        "XML or text declaration not at start of entity",
    ),
    ("XML_ERROR_UNKNOWN_ENCODING", 18, "unknown encoding"),
    (
        "XML_ERROR_INCORRECT_ENCODING",
        19,
        "encoding specified in XML declaration is incorrect",
    ),
    (
        "XML_ERROR_UNCLOSED_CDATA_SECTION",
        20,
        "unclosed CDATA section",
    ),
    (
        "XML_ERROR_EXTERNAL_ENTITY_HANDLING",
        21,
        "error in processing external entity reference",
    ),
    ("XML_ERROR_NOT_STANDALONE", 22, "document is not standalone"),
    (
        "XML_ERROR_UNEXPECTED_STATE",
        23,
        "unexpected parser state - please send a bug report",
    ),
    (
        "XML_ERROR_ENTITY_DECLARED_IN_PE",
        24,
        "entity declared in parameter entity",
    ),
    (
        "XML_ERROR_FEATURE_REQUIRES_XML_DTD",
        25,
        "requested feature requires XML_DTD support in Expat",
    ),
    (
        "XML_ERROR_CANT_CHANGE_FEATURE_ONCE_PARSING",
        26,
        "cannot change setting once parsing has begun",
    ),
    ("XML_ERROR_UNBOUND_PREFIX", 27, "unbound prefix"),
    (
        "XML_ERROR_UNDECLARING_PREFIX",
        28,
        "must not undeclare prefix",
    ),
    (
        "XML_ERROR_INCOMPLETE_PE",
        29,
        "incomplete markup in parameter entity",
    ),
    ("XML_ERROR_XML_DECL", 30, "XML declaration not well-formed"),
    (
        "XML_ERROR_TEXT_DECL",
        31,
        "text declaration not well-formed",
    ),
    (
        "XML_ERROR_PUBLICID",
        32,
        "illegal character(s) in public id",
    ),
    ("XML_ERROR_SUSPENDED", 33, "parser suspended"),
    ("XML_ERROR_NOT_SUSPENDED", 34, "parser not suspended"),
    ("XML_ERROR_ABORTED", 35, "parsing aborted"),
    ("XML_ERROR_FINISHED", 36, "parsing finished"),
    (
        "XML_ERROR_SUSPEND_PE",
        37,
        "cannot suspend in external parameter entity",
    ),
    (
        "XML_ERROR_RESERVED_PREFIX_XML",
        38,
        "reserved prefix (xml) must not be undeclared or bound to another namespace name",
    ),
    (
        "XML_ERROR_RESERVED_PREFIX_XMLNS",
        39,
        "reserved prefix (xmlns) must not be declared or undeclared",
    ),
    (
        "XML_ERROR_RESERVED_NAMESPACE_URI",
        40,
        "prefix must not be bound to one of the reserved namespace names",
    ),
    ("XML_ERROR_INVALID_ARGUMENT", 41, "invalid argument"),
    (
        "XML_ERROR_NO_BUFFER",
        42,
        "a successful prior call to function XML_GetBuffer is required",
    ),
    (
        "XML_ERROR_AMPLIFICATION_LIMIT_BREACH",
        43,
        "limit on input amplification factor (from DTD and entities) breached",
    ),
];

pub(crate) fn error_string(code: i64) -> Option<&'static str> {
    ERROR_TABLE
        .iter()
        .find(|(_, c, _)| *c == code)
        .map(|(_, _, msg)| *msg)
}

fn build_messages_dict() -> Value {
    let mut d = PyDict::default();
    for &(_, code, msg) in ERROR_TABLE {
        d.insert(PyKey::Int(code), Value::string(msg));
    }
    Value::dict(d)
}

fn build_codes_dict() -> Value {
    let mut d = PyDict::default();
    for &(_, code, msg) in ERROR_TABLE {
        d.insert(PyKey::str_from(msg), Value::int(code));
    }
    Value::dict(d)
}

pyrust_module! {
    constants {
        "messages" => build_messages_dict(),
        "codes" => build_codes_dict(),
        "XML_ERROR_NO_MEMORY" => Value::string("out of memory"),
        "XML_ERROR_SYNTAX" => Value::string("syntax error"),
        "XML_ERROR_NO_ELEMENTS" => Value::string("no element found"),
        "XML_ERROR_INVALID_TOKEN" => Value::string("not well-formed (invalid token)"),
        "XML_ERROR_UNCLOSED_TOKEN" => Value::string("unclosed token"),
        "XML_ERROR_PARTIAL_CHAR" => Value::string("partial character"),
        "XML_ERROR_TAG_MISMATCH" => Value::string("mismatched tag"),
        "XML_ERROR_DUPLICATE_ATTRIBUTE" => Value::string("duplicate attribute"),
        "XML_ERROR_JUNK_AFTER_DOC_ELEMENT" => Value::string("junk after document element"),
        "XML_ERROR_PARAM_ENTITY_REF" => Value::string("illegal parameter entity reference"),
        "XML_ERROR_UNDEFINED_ENTITY" => Value::string("undefined entity"),
        "XML_ERROR_RECURSIVE_ENTITY_REF" => Value::string("recursive entity reference"),
        "XML_ERROR_ASYNC_ENTITY" => Value::string("asynchronous entity"),
        "XML_ERROR_BAD_CHAR_REF" => Value::string("reference to invalid character number"),
        "XML_ERROR_BINARY_ENTITY_REF" => Value::string("reference to binary entity"),
        "XML_ERROR_ATTRIBUTE_EXTERNAL_ENTITY_REF" => Value::string("reference to external entity in attribute"),
        "XML_ERROR_MISPLACED_XML_PI" => Value::string("XML or text declaration not at start of entity"),
        "XML_ERROR_UNKNOWN_ENCODING" => Value::string("unknown encoding"),
        "XML_ERROR_INCORRECT_ENCODING" => Value::string("encoding specified in XML declaration is incorrect"),
        "XML_ERROR_UNCLOSED_CDATA_SECTION" => Value::string("unclosed CDATA section"),
        "XML_ERROR_EXTERNAL_ENTITY_HANDLING" => Value::string("error in processing external entity reference"),
        "XML_ERROR_NOT_STANDALONE" => Value::string("document is not standalone"),
        "XML_ERROR_UNEXPECTED_STATE" => Value::string("unexpected parser state - please send a bug report"),
        "XML_ERROR_ENTITY_DECLARED_IN_PE" => Value::string("entity declared in parameter entity"),
        "XML_ERROR_FEATURE_REQUIRES_XML_DTD" => Value::string("requested feature requires XML_DTD support in Expat"),
        "XML_ERROR_CANT_CHANGE_FEATURE_ONCE_PARSING" => Value::string("cannot change setting once parsing has begun"),
        "XML_ERROR_UNBOUND_PREFIX" => Value::string("unbound prefix"),
        "XML_ERROR_UNDECLARING_PREFIX" => Value::string("must not undeclare prefix"),
        "XML_ERROR_INCOMPLETE_PE" => Value::string("incomplete markup in parameter entity"),
        "XML_ERROR_XML_DECL" => Value::string("XML declaration not well-formed"),
        "XML_ERROR_TEXT_DECL" => Value::string("text declaration not well-formed"),
        "XML_ERROR_PUBLICID" => Value::string("illegal character(s) in public id"),
        "XML_ERROR_SUSPENDED" => Value::string("parser suspended"),
        "XML_ERROR_NOT_SUSPENDED" => Value::string("parser not suspended"),
        "XML_ERROR_ABORTED" => Value::string("parsing aborted"),
        "XML_ERROR_FINISHED" => Value::string("parsing finished"),
        "XML_ERROR_SUSPEND_PE" => Value::string("cannot suspend in external parameter entity"),
        "XML_ERROR_RESERVED_PREFIX_XML" => Value::string("reserved prefix (xml) must not be undeclared or bound to another namespace name"),
        "XML_ERROR_RESERVED_PREFIX_XMLNS" => Value::string("reserved prefix (xmlns) must not be declared or undeclared"),
        "XML_ERROR_RESERVED_NAMESPACE_URI" => Value::string("prefix must not be bound to one of the reserved namespace names"),
        "XML_ERROR_INVALID_ARGUMENT" => Value::string("invalid argument"),
        "XML_ERROR_NO_BUFFER" => Value::string("a successful prior call to function XML_GetBuffer is required"),
        "XML_ERROR_AMPLIFICATION_LIMIT_BREACH" => Value::string("limit on input amplification factor (from DTD and entities) breached"),
    }
}
