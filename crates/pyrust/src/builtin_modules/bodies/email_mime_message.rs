// `email.mime.message` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// `MIMEMessage`: class for generating `message/*` MIME documents.
//
// Reference: <https://docs.python.org/3/library/email.mime.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue, PyValueRef};
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
    let cls_val = module.borrow().attrs.get("MIMEMessage").cloned();
    if let Some(val) = cls_val
        && let ValueKind::PyClass(cls_rc) = val.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(nm_cls)]);
    }
    Ok(())
}

fn all_list() -> Value {
    Value::list(vec![Value::string("MIMEMessage")])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email.mime"),
        "__all__" => all_list(),
    }

    class MIMEMessage {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            _msg: PyValue,
            #[default("rfc822".into())] _subtype: PyStr<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            policy: OptValueRef<'_>,
        ) -> Result<Value> {
            let nm_mod = _interp.load_module("email.mime.nonmultipart")?;
            let nm_cls = _interp.get_attr(&nm_mod, "MIMENonMultipart")?;
            let nm_init = _interp.get_attr(&nm_cls, "__init__")?;
            call_kw(
                _interp,
                nm_init,
                &[
                    self_val.0.clone(),
                    Value::string("message"),
                    Value::string(_subtype.0.as_ref()),
                ],
                &[(
                    "policy",
                    policy.as_value().cloned().unwrap_or_else(Value::none),
                )],
            )?;

            let msg_mod = _interp.load_module("email.message")?;
            let msg_cls = _interp.get_attr(&msg_mod, "Message")?;
            let is_msg = call_kw(
                _interp,
                Value::builtin_function("isinstance"),
                &[_msg.0.clone(), msg_cls.clone()],
                &[],
            )?;
            if !_interp.truthy_value(&is_msg)? {
                return Err(PyError::named(
                    "TypeError",
                    "Argument is not an instance of Message".to_string(),
                ));
            }
            let attach_fn = _interp.get_attr(&msg_cls, "attach")?;
            call_kw(_interp, attach_fn, &[self_val.0.clone(), _msg.0], &[])?;
            let set_dt = _interp.get_attr(self_val.0, "set_default_type")?;
            call_kw(_interp, set_dt, &[Value::string("message/rfc822")], &[])?;
            Ok(Value::none())
        }
    }
}
