// `email.mime.application` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// `MIMEApplication`: class for generating `application/*` MIME documents.
//
// Reference: <https://docs.python.org/3/library/email.mime.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let nm_mod = interp.load_module("email.mime.nonmultipart")?;
    let nm_cls_val = interp.get_attr(&nm_mod, "MIMENonMultipart")?;
    let ValueKind::PyClass(nm_cls) = nm_cls_val.kind() else {
        return Err(PyError::runtime(
            "email.mime.nonmultipart.MIMENonMultipart is not a class".to_string(),
        ));
    };
    let cls_val = module.borrow().attrs.get("MIMEApplication").cloned();
    if let Some(val) = cls_val
        && let ValueKind::PyClass(cls_rc) = val.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(nm_cls)]);
    }
    Ok(())
}

fn all_list() -> Value {
    Value::list(vec![Value::string("MIMEApplication")])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email.mime"),
        "__all__" => all_list(),
    }

    class MIMEApplication {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            _data: PyValue,
            #[default(OptionalArg::Omitted)] _subtype: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _encoder: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            policy: OptValueRef<'_>,
            #[varkwargs] _params: IndexMap<String, Value>,
        ) -> Result<Value> {
            let subtype = match _subtype.as_value() {
                None => Value::string("octet-stream"),
                Some(v) if v.is_none() => {
                    return Err(PyError::named(
                        "TypeError",
                        "Invalid application MIME subtype".to_string(),
                    ));
                }
                Some(v) => v.clone(),
            };
            let encoder = match _encoder.as_value() {
                Some(v) => v.clone(),
                None => {
                    let enc_mod = _interp.load_module("email.encoders")?;
                    _interp.get_attr(&enc_mod, "encode_base64")?
                }
            };
            let nm_mod = _interp.load_module("email.mime.nonmultipart")?;
            let nm_cls = _interp.get_attr(&nm_mod, "MIMENonMultipart")?;
            let nm_init = _interp.get_attr(&nm_cls, "__init__")?;
            let mut expanded = vec![
                ExpandedCallArg {
                    name: None,
                    value: self_val.0.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string("application"),
                },
                ExpandedCallArg {
                    name: None,
                    value: subtype,
                },
                ExpandedCallArg {
                    name: Some("policy".to_string()),
                    value: policy.as_value().cloned().unwrap_or_else(Value::none),
                },
            ];
            for (k, v) in _params {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            _interp.call_function_expanded(nm_init, &expanded)?;
            let set_payload = _interp.get_attr(self_val.0, "set_payload")?;
            call_pos(_interp, set_payload, &[_data.0])?;
            call_pos(_interp, encoder, std::slice::from_ref(self_val.0))?;
            Ok(Value::none())
        }
    }
}
