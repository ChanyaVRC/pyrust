// `email.mime.multipart` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// `MIMEMultipart`: base class for `multipart/*` MIME documents.
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
    let base_mod = interp.load_module("email.mime.base")?;
    let base_cls_val = interp.get_attr(&base_mod, "MIMEBase")?;
    let ValueKind::PyClass(base_cls) = base_cls_val.kind() else {
        return Err(PyError::runtime(
            "email.mime.base.MIMEBase is not a class".to_string(),
        ));
    };
    let cls_val = module.borrow().attrs.get("MIMEMultipart").cloned();
    if let Some(val) = cls_val
        && let ValueKind::PyClass(cls_rc) = val.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(base_cls)]);
    }
    Ok(())
}

fn all_list() -> Value {
    Value::list(vec![Value::string("MIMEMultipart")])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email.mime"),
        "__all__" => all_list(),
    }

    class MIMEMultipart {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default("mixed".into())] _subtype: PyStr<'_>,
            #[default(OptionalArg::Omitted)] boundary: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _subparts: OptValueRef<'_>,
            #[keyword_only]
            #[default(OptionalArg::Omitted)]
            policy: OptValueRef<'_>,
            #[varkwargs] _params: IndexMap<String, Value>,
        ) -> Result<Value> {
            let base_mod = _interp.load_module("email.mime.base")?;
            let base_cls = _interp.get_attr(&base_mod, "MIMEBase")?;
            let base_init = _interp.get_attr(&base_cls, "__init__")?;
            let mut expanded = vec![
                ExpandedCallArg {
                    name: None,
                    value: self_val.0.clone(),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string("multipart"),
                },
                ExpandedCallArg {
                    name: None,
                    value: Value::string(_subtype.0.as_ref()),
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
            _interp.call_function_expanded(base_init, &expanded)?;

            _interp.assign_attr(self_val.0.clone(), "_payload", Value::list(Vec::new()))?;

            if let Some(sp) = _subparts.as_value()
                && !sp.is_none()
                && _interp.truthy_value(sp)?
            {
                let parts = _interp.collect_iterable(sp)?;
                let attach_fn = _interp.get_attr(self_val.0, "attach")?;
                for p in parts {
                    call_pos(_interp, attach_fn.clone(), &[p])?;
                }
            }
            if let Some(b) = boundary.as_value()
                && !b.is_none()
                && _interp.truthy_value(b)?
            {
                let set_b = _interp.get_attr(self_val.0, "set_boundary")?;
                call_pos(_interp, set_b, std::slice::from_ref(b))?;
            }
            Ok(Value::none())
        }
    }
}
