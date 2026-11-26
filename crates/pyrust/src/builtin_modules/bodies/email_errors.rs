// `email.errors` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// Complete exception and parsing-defect hierarchy for the `email` package.
//
// Reference: <https://docs.python.org/3/library/email.errors.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
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

/// Call Python's `str(val)` and return the resulting Rust `String`.
pub(crate) fn py_str(interp: &mut Interpreter, val: &Value) -> Result<String> {
    if let Some(s) = val.as_str() {
        return Ok(s.to_string());
    }
    let str_cls = Value::py_class(
        crate::interpreter::primitive_class_by_name("str")
            .ok_or_else(|| PyError::runtime("str class not found".to_string()))?,
    );
    let res = call_pos(interp, str_cls, std::slice::from_ref(val))?;
    Ok(res.as_str().unwrap_or("").to_string())
}

/// Raise a named exception class from `email.errors` with a single message string.
pub(crate) fn raise_error(
    interp: &mut Interpreter,
    exc_name: &'static str,
    message: &str,
) -> PyError {
    if let Ok(m) = interp.load_module("email.errors")
        && let Ok(cls) = interp.get_attr(&m, exc_name)
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(message)])
    {
        return PyError::raised(inst);
    }
    PyError::named(exc_name, message.to_string())
}

/// Raise a named exception class from `email.errors` with an arbitrary Value argument.
pub(crate) fn raise_error_val(
    interp: &mut Interpreter,
    exc_name: &'static str,
    arg: Value,
) -> PyError {
    if let Ok(m) = interp.load_module("email.errors")
        && let Ok(cls) = interp.get_attr(&m, exc_name)
        && let Ok(inst) = call_pos(interp, cls, &[arg])
    {
        return PyError::raised(inst);
    }
    PyError::named(exc_name, String::new())
}

/// Construct a defect instance from `email.errors`.
pub(crate) fn make_defect(
    interp: &mut Interpreter,
    defect_name: &str,
    arg: Option<Value>,
) -> Result<Value> {
    let m = interp.load_module("email.errors")?;
    let cls = interp.get_attr(&m, defect_name)?;
    match arg {
        Some(v) => call_pos(interp, cls, &[v]),
        None => call_pos(interp, cls, &[]),
    }
}

pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // Backward-compatibility alias matching CPython's `email.errors`.
    let cls = module
        .borrow()
        .attrs
        .get("MissingHeaderBodySeparatorDefect")
        .cloned();
    if let Some(cls) = cls {
        module
            .borrow_mut()
            .attrs
            .insert("MalformedHeaderDefect".to_string(), cls);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email"),
    }

    /// Base class for errors in the email package.
    exception MessageError(Exception);

    /// Base class for message parsing errors.
    exception MessageParseError(MessageError);

    /// Error while parsing headers.
    exception HeaderParseError(MessageParseError);

    /// Couldn't find terminating boundary.
    exception BoundaryError(MessageParseError);

    /// Conversion to a multipart is prohibited.
    exception MultipartConversionError(MessageError, TypeError);

    /// An illegal charset was given.
    exception CharsetError(MessageError);

    /// Error while writing/folding a header.
    exception HeaderWriteError(MessageError);

    /// Base class for a message defect.
    class MessageDefect("ValueError") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] line: OptValueRef<'_>,
        ) -> Result<Value> {
            let line_val = line.as_value().cloned().unwrap_or_else(Value::none);
            if !line_val.is_none() {
                let value_err = crate::interpreter::lookup_exc_class("ValueError")
                    .ok_or_else(|| PyError::runtime("ValueError not found".to_string()))?;
                let init_fn = _interp.get_attr(&Value::py_class(value_err), "__init__")?;
                call_pos(_interp, init_fn, &[self_val.0.clone(), line_val.clone()])?;
            }
            _interp.assign_attr(self_val.0.clone(), "line", line_val)?;
            Ok(Value::none())
        }
    }

    /// A message claimed to be a multipart but had no boundary parameter.
    exception NoBoundaryInMultipartDefect(MessageDefect);

    /// The claimed start boundary was never found.
    exception StartBoundaryNotFoundDefect(MessageDefect);

    /// A start boundary was found, but not the corresponding close boundary.
    exception CloseBoundaryNotFoundDefect(MessageDefect);

    /// A message had a continuation line as its first header line.
    exception FirstHeaderLineIsContinuationDefect(MessageDefect);

    /// A 'Unix-from' header was found in the middle of a header block.
    exception MisplacedEnvelopeHeaderDefect(MessageDefect);

    /// Found line with no leading whitespace and no colon before blank line.
    exception MissingHeaderBodySeparatorDefect(MessageDefect);

    /// A message claimed to be a multipart but no subparts were found.
    exception MultipartInvariantViolationDefect(MessageDefect);

    /// An invalid content transfer encoding was set on the multipart itself.
    exception InvalidMultipartContentTransferEncodingDefect(MessageDefect);

    /// Header contained bytes that could not be decoded.
    exception UndecodableBytesDefect(MessageDefect);

    /// base64 encoded sequence had an incorrect length.
    exception InvalidBase64PaddingDefect(MessageDefect);

    /// base64 encoded sequence had characters not in base64 alphabet.
    exception InvalidBase64CharactersDefect(MessageDefect);

    /// base64 encoded sequence had invalid length (1 mod 4).
    exception InvalidBase64LengthDefect(MessageDefect);

    /// Base class for a header defect.
    class HeaderDefect("MessageDefect") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let m = _interp.load_module("email.errors")?;
            let md = _interp.get_attr(&m, "MessageDefect")?;
            let init_fn = _interp.get_attr(&md, "__init__")?;
            let mut expanded = Vec::with_capacity(1 + args.len() + kwargs.len());
            expanded.push(ExpandedCallArg {
                name: None,
                value: self_val.0.clone(),
            });
            for a in args {
                expanded.push(ExpandedCallArg {
                    name: None,
                    value: a.clone(),
                });
            }
            for (k, v) in kwargs {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            _interp.call_function_expanded(init_fn, &expanded)?;
            Ok(Value::none())
        }
    }

    /// Header is not valid, message gives details.
    exception InvalidHeaderDefect(HeaderDefect);

    /// A header that must have a value had none.
    exception HeaderMissingRequiredValue(HeaderDefect);

    /// ASCII characters outside the ascii-printable range found.
    class NonPrintableDefect("HeaderDefect") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            non_printables: PyValue,
        ) -> Result<Value> {
            let m = _interp.load_module("email.errors")?;
            let hd = _interp.get_attr(&m, "HeaderDefect")?;
            let init_fn = _interp.get_attr(&hd, "__init__")?;
            call_pos(_interp, init_fn, &[self_val.0.clone(), non_printables.0.clone()])?;
            _interp.assign_attr(self_val.0.clone(), "non_printables", non_printables.0)?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let np = _interp.get_attr(self_val.0, "non_printables")?;
            let np_str = py_str(_interp, &np)?;
            Ok(Value::string(format!(
                "the following ASCII non-printables found in header: {np_str}"
            )))
        }
    }

    /// Header uses syntax declared obsolete by RFC 5322.
    exception ObsoleteHeaderDefect(HeaderDefect);

    /// local_part contains non-ASCII characters.
    exception NonASCIILocalPartDefect(HeaderDefect);

    /// Header has unparsable or invalid date.
    exception InvalidDateDefect(HeaderDefect);
}
