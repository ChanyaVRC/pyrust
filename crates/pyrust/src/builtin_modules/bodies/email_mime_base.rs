// `email.mime.base` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// `MIMEBase`: base class for MIME specializations inheriting `email.message.Message`.
//
// Reference: <https://docs.python.org/3/library/email.mime.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
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
    let msg_mod = interp.load_module("email.message")?;
    let msg_cls_val = interp.get_attr(&msg_mod, "Message")?;
    let ValueKind::PyClass(msg_cls) = msg_cls_val.kind() else {
        return Err(PyError::runtime(
            "email.message.Message is not a class".to_string(),
        ));
    };
    let cls_val = module.borrow().attrs.get("MIMEBase").cloned();
    if let Some(val) = cls_val
        && let ValueKind::PyClass(cls_rc) = val.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(msg_cls)]);
    }
    Ok(())
}

fn all_list() -> Value {
    Value::list(vec![Value::string("MIMEBase")])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email.mime"),
        "__all__" => all_list(),
    }

    class MIMEBase {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            _maintype: PyStr<'_>,
            _subtype: PyStr<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            policy: OptValueRef<'_>,
            #[varkwargs] _params: IndexMap<String, Value>,
        ) -> Result<Value> {
            let pol = match policy.as_value() {
                Some(v) if !v.is_none() => v.clone(),
                _ => {
                    let pm = _interp.load_module("email.policy")?;
                    _interp.get_attr(&pm, "compat32")?
                }
            };
            let msg_mod = _interp.load_module("email.message")?;
            let msg_cls = _interp.get_attr(&msg_mod, "Message")?;
            let msg_init = _interp.get_attr(&msg_cls, "__init__")?;
            call_kw(
                _interp,
                msg_init,
                std::slice::from_ref(self_val.0),
                &[("policy", pol)],
            )?;

            let ctype = format!("{}/{}", _maintype.0.as_ref(), _subtype.0.as_ref());
            let add_hdr = _interp.get_attr(self_val.0, "add_header")?;
            let mut expanded = vec![
                ExpandedCallArg {
                    name: None,
                    value: Value::string("Content-Type"),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string(ctype),
                },
            ];
            for (k, v) in _params {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            _interp.call_function_expanded(add_hdr, &expanded)?;

            let setitem = _interp.get_attr(self_val.0, "__setitem__")?;
            call_kw(
                _interp,
                setitem,
                &[Value::string("MIME-Version"), Value::string("1.0")],
                &[],
            )?;
            Ok(Value::none())
        }
    }
}
