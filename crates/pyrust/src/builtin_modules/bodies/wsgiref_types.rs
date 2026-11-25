// `wsgiref.types` module — 100% native-Rust Python 3.12 PEP 3333 static
// typing aliases and protocols (`StartResponse`, `WSGIEnvironment`,
// `WSGIApplication`, `InputStream`, `ErrorStream`, `FileWrapper`).
//
// Reference: <https://docs.python.org/3/library/wsgiref.html#module-wsgiref.types>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::Result;
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

/// Link PEP 3333 protocol classes to `typing.Protocol` and populate the
/// `WSGIEnvironment` and `WSGIApplication` typing aliases.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let typing_mod = interp.load_module("typing")?;
    let cabc_mod = interp.load_module("collections.abc")?;
    if let Ok(proto) = interp.get_attr(&typing_mod, "Protocol")
        && let ValueKind::PyClass(proto_rc) = proto.kind()
    {
        for cls_name in ["StartResponse", "InputStream", "ErrorStream", "FileWrapper"] {
            if let Some(cval) = module.borrow().attrs.get(cls_name).cloned()
                && let ValueKind::PyClass(c_rc) = cval.kind()
            {
                link_class_bases(c_rc, vec![Rc::clone(proto_rc)]);
            }
        }
    }

    let any_val = interp.get_attr(&typing_mod, "Any")?;
    let callable_val = interp
        .get_attr(&cabc_mod, "Callable")
        .or_else(|_| interp.get_attr(&typing_mod, "Callable"))?;
    let iterable_val = interp
        .get_attr(&cabc_mod, "Iterable")
        .or_else(|_| interp.get_attr(&typing_mod, "Iterable"))?;
    let builtins = interp.load_module("builtins")?;
    let dict_cls = interp.get_attr(&builtins, "dict")?;
    let str_cls = interp.get_attr(&builtins, "str")?;
    let bytes_cls = interp.get_attr(&builtins, "bytes")?;

    let wsgi_env = interp
        .eval_index(&dict_cls, Value::tuple(vec![str_cls, any_val]))
        .unwrap_or_else(|_| dict_cls.clone());
    let start_resp = module
        .borrow()
        .attrs
        .get("StartResponse")
        .cloned()
        .unwrap_or_else(Value::none);
    let iter_bytes = interp
        .eval_index(&iterable_val, bytes_cls)
        .unwrap_or_else(|_| iterable_val.clone());
    let wsgi_app = interp
        .eval_index(
            &callable_val,
            Value::tuple(vec![
                Value::list(vec![wsgi_env.clone(), start_resp]),
                iter_bytes,
            ]),
        )
        .unwrap_or_else(|_| callable_val.clone());
    let _ = call_pos;

    let mut m = module.borrow_mut();
    m.attrs.insert("WSGIEnvironment".to_string(), wsgi_env);
    m.attrs.insert("WSGIApplication".to_string(), wsgi_app);
    Ok(())
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("StartResponse"),
            Value::string("WSGIEnvironment"),
            Value::string("WSGIApplication"),
            Value::string("InputStream"),
            Value::string("ErrorStream"),
            Value::string("FileWrapper"),
        ]),
        "WSGIEnvironment" => Value::none(),
        "WSGIApplication" => Value::none(),
    }

    class StartResponse {
        fn __call__(
            #[positional_only] _self: PyValueRef<'_>,
            _status: PyValue,
            _headers: PyValue,
            #[default(OptionalArg::Omitted)] _exc_info: OptValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::none())
        }
    }

    class InputStream {
        fn read(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _size: OptValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::bytes(Vec::new()))
        }
    }

    class ErrorStream {
        fn flush(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }

    class FileWrapper {
        fn __call__(
            #[positional_only] _self: PyValueRef<'_>,
            _file: PyValue,
            #[default(OptionalArg::Omitted)] _block_size: OptValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::none())
        }
    }
}
