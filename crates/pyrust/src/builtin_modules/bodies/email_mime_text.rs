// `email.mime.text` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// `MIMEText`: class for generating `text/*` MIME documents.
//
// Reference: <https://docs.python.org/3/library/email.mime.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
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
    let cls_val = module.borrow().attrs.get("MIMEText").cloned();
    if let Some(val) = cls_val
        && let ValueKind::PyClass(cls_rc) = val.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(nm_cls)]);
    }
    Ok(())
}

fn all_list() -> Value {
    Value::list(vec![Value::string("MIMEText")])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email.mime"),
        "__all__" => all_list(),
    }

    class MIMEText {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            _text: PyStr<'_>,
            #[default("plain".into())] _subtype: PyStr<'_>,
            #[default(OptionalArg::Omitted)] _charset: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            policy: OptValueRef<'_>,
        ) -> Result<Value> {
            let cs_val = match _charset.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    if _text.0.is_ascii() {
                        Value::string("us-ascii")
                    } else {
                        Value::string("utf-8")
                    }
                }
            };
            let cs_str = super::email_errors::py_str(_interp, &cs_val)?;
            let nm_mod = _interp.load_module("email.mime.nonmultipart")?;
            let nm_cls = _interp.get_attr(&nm_mod, "MIMENonMultipart")?;
            let nm_init = _interp.get_attr(&nm_cls, "__init__")?;
            call_kw(
                _interp,
                nm_init,
                &[
                    self_val.0.clone(),
                    Value::string("text"),
                    Value::string(_subtype.0.as_ref()),
                ],
                &[
                    (
                        "policy",
                        policy.as_value().cloned().unwrap_or_else(Value::none),
                    ),
                    ("charset", Value::string(cs_str)),
                ],
            )?;
            let set_payload = _interp.get_attr(self_val.0, "set_payload")?;
            call_kw(
                _interp,
                set_payload,
                &[Value::string(_text.0.as_ref()), cs_val],
                &[],
            )?;
            Ok(Value::none())
        }
    }
}
