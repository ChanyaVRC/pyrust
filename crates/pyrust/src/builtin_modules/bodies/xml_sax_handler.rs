// `xml.sax.handler` module — 100% native-Rust Python 3.12 SAX handler base
// classes, feature URIs, and property URIs (issue #3260).
//
// Reference: <https://docs.python.org/3/library/xml.sax.handler.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyValue, PyValueRef};
use crate::value::Value;

pub(crate) const FEATURE_NAMESPACES: &str = "http://xml.org/sax/features/namespaces";
pub(crate) const FEATURE_NAMESPACE_PREFIXES: &str =
    "http://xml.org/sax/features/namespace-prefixes";
pub(crate) const FEATURE_STRING_INTERNING: &str = "http://xml.org/sax/features/string-interning";
pub(crate) const FEATURE_VALIDATION: &str = "http://xml.org/sax/features/validation";
pub(crate) const FEATURE_EXTERNAL_GES: &str =
    "http://xml.org/sax/features/external-general-entities";
pub(crate) const FEATURE_EXTERNAL_PES: &str =
    "http://xml.org/sax/features/external-parameter-entities";

pub(crate) const PROPERTY_LEXICAL_HANDLER: &str = "http://xml.org/sax/properties/lexical-handler";
pub(crate) const PROPERTY_DOM_NODE: &str = "http://xml.org/sax/properties/dom-node";
pub(crate) const PROPERTY_DECLARATION_HANDLER: &str =
    "http://xml.org/sax/properties/declaration-handler";
pub(crate) const PROPERTY_XML_STRING: &str = "http://xml.org/sax/properties/xml-string";
pub(crate) const PROPERTY_ENCODING: &str = "http://www.python.org/sax/properties/encoding";
pub(crate) const PROPERTY_INTERNING_DICT: &str =
    "http://www.python.org/sax/properties/interning-dict";

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.sax"),
        "version" => Value::string("2.0beta"),
        "feature_namespaces" => Value::string(FEATURE_NAMESPACES),
        "feature_namespace_prefixes" => Value::string(FEATURE_NAMESPACE_PREFIXES),
        "feature_string_interning" => Value::string(FEATURE_STRING_INTERNING),
        "feature_validation" => Value::string(FEATURE_VALIDATION),
        "feature_external_ges" => Value::string(FEATURE_EXTERNAL_GES),
        "feature_external_pes" => Value::string(FEATURE_EXTERNAL_PES),
        "all_features" => Value::list(vec![
            Value::string(FEATURE_NAMESPACES),
            Value::string(FEATURE_NAMESPACE_PREFIXES),
            Value::string(FEATURE_STRING_INTERNING),
            Value::string(FEATURE_VALIDATION),
            Value::string(FEATURE_EXTERNAL_GES),
            Value::string(FEATURE_EXTERNAL_PES),
        ]),
        "property_lexical_handler" => Value::string(PROPERTY_LEXICAL_HANDLER),
        "property_dom_node" => Value::string(PROPERTY_DOM_NODE),
        "property_declaration_handler" => Value::string(PROPERTY_DECLARATION_HANDLER),
        "property_xml_string" => Value::string(PROPERTY_XML_STRING),
        "property_encoding" => Value::string(PROPERTY_ENCODING),
        "property_interning_dict" => Value::string(PROPERTY_INTERNING_DICT),
        "all_properties" => Value::list(vec![
            Value::string(PROPERTY_LEXICAL_HANDLER),
            Value::string(PROPERTY_DOM_NODE),
            Value::string(PROPERTY_DECLARATION_HANDLER),
            Value::string(PROPERTY_XML_STRING),
            Value::string(PROPERTY_ENCODING),
            Value::string(PROPERTY_INTERNING_DICT),
        ]),
    }

    class ErrorHandler {
        fn error(#[positional_only] _self: PyValueRef<'_>, exception: PyValue) -> Result<Value> {
            Err(PyError::raised(exception.0))
        }

        #[py_name = "fatalError"]
        fn fatal_error(
            #[positional_only] _self: PyValueRef<'_>,
            exception: PyValue,
        ) -> Result<Value> {
            Err(PyError::raised(exception.0))
        }

        fn warning(#[positional_only] _self: PyValueRef<'_>, exception: PyValue) -> Result<Value> {
            let msg = _interp.render_value_as_str(&exception.0)?;
            if let Ok(sys_mod) = _interp.load_module("sys")
                && let Ok(stderr) = _interp.get_attr(&sys_mod, "stderr")
                && let Ok(write_fn) = _interp.get_attr(&stderr, "write")
            {
                let _ = _interp.call_function_expanded(
                    write_fn,
                    &[crate::interpreter::ExpandedCallArg {
                        name: None,
                        value: Value::string(format!("{msg}\n")),
                    }],
                );
            }
            Ok(Value::none())
        }
    }

    class ContentHandler {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_locator", Value::none())?;
            Ok(Value::none())
        }

        #[py_name = "setDocumentLocator"]
        fn set_document_locator(
            #[positional_only] self_val: PyValueRef<'_>,
            locator: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_locator", locator.0)?;
            Ok(Value::none())
        }

        #[py_name = "startDocument"]
        fn start_document(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "endDocument"]
        fn end_document(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "startPrefixMapping"]
        fn start_prefix_mapping(
            #[positional_only] _self: PyValueRef<'_>,
            _prefix: PyValue,
            _uri: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "endPrefixMapping"]
        fn end_prefix_mapping(
            #[positional_only] _self: PyValueRef<'_>,
            _prefix: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "startElement"]
        fn start_element(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
            _attrs: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "endElement"]
        fn end_element(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "startElementNS"]
        fn start_element_ns(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
            _qname: PyValue,
            _attrs: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "endElementNS"]
        fn end_element_ns(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
            _qname: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn characters(
            #[positional_only] _self: PyValueRef<'_>,
            _content: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "ignorableWhitespace"]
        fn ignorable_whitespace(
            #[positional_only] _self: PyValueRef<'_>,
            _whitespace: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "processingInstruction"]
        fn processing_instruction(
            #[positional_only] _self: PyValueRef<'_>,
            _target: PyValue,
            _data: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "skippedEntity"]
        fn skipped_entity(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }
    }

    class DTDHandler {
        #[py_name = "notationDecl"]
        fn notation_decl(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
            _public_id: PyValue,
            _system_id: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "unparsedEntityDecl"]
        fn unparsed_entity_decl(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
            _public_id: PyValue,
            _system_id: PyValue,
            _ndata: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }
    }

    class EntityResolver {
        #[py_name = "resolveEntity"]
        fn resolve_entity(
            #[positional_only] _self: PyValueRef<'_>,
            _public_id: PyValue,
            system_id: PyValue,
        ) -> Result<Value> {
            Ok(system_id.0)
        }
    }

    class LexicalHandler {
        fn comment(#[positional_only] _self: PyValueRef<'_>, _content: PyValue) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "startDTD"]
        fn start_dtd(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
            _public_id: PyValue,
            _system_id: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "endDTD"]
        fn end_dtd(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "startCDATA"]
        fn start_cdata(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }

        #[py_name = "endCDATA"]
        fn end_cdata(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }
}
