// `email.mime.nonmultipart` module — 100% native-Rust Python 3.12 implementation (issue #3257).
//
// `MIMENonMultipart`: base class for non-multipart MIME documents.
//
// Reference: <https://docs.python.org/3/library/email.mime.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

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
    let cls_val = module.borrow().attrs.get("MIMENonMultipart").cloned();
    if let Some(val) = cls_val
        && let ValueKind::PyClass(cls_rc) = val.kind()
    {
        link_class_bases(cls_rc, vec![Rc::clone(base_cls)]);
    }
    Ok(())
}

fn all_list() -> Value {
    Value::list(vec![Value::string("MIMENonMultipart")])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("email.mime"),
        "__all__" => all_list(),
    }

    class MIMENonMultipart {
        fn attach(
            #[positional_only] _self_val: PyValueRef<'_>,
            _payload: PyValue,
        ) -> Result<Value> {
            Err(super::email_errors::raise_error(
                _interp,
                "MultipartConversionError",
                "Cannot attach additional subparts to non-multipart/*",
            ))
        }
    }
}
