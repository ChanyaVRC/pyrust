// `urllib.response` module — 100% native Rust implementation of CPython 3.12's
// `Lib/urllib/response.py` (issues #3223, #3347).
//
// The response wrapper classes `addbase`, `addclosehook`, `addinfo`, and
// `addinfourl`.  Like CPython, `addbase` derives from
// `tempfile._TemporaryFileWrapper`: that base is wired in by
// `prepare_module` (the module imports `tempfile`, exactly as the CPython
// source does) so attribute delegation to the wrapped file, `close()`,
// `writelines()`, iteration, and the `file` / `name` / `delete` / `_closer`
// attributes all come from the same wrapper class CPython uses.
//
// Reference: <https://docs.python.org/3/library/urllib.request.html>

use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr};
use crate::value::{Value, ValueKind};

const MODULE_DOC: &str = "Response classes used by urllib.\n\n\
The base class, addbase, defines a minimal file-like interface,\n\
including read() and readline().  The typical response object is an\n\
addinfourl instance, which defines an info() method that returns\n\
headers and a geturl() method that returns the url.\n";

/// Name `addbase`'s `file` argument is registered under in the wrapper
/// (`tempfile._TemporaryFileWrapper.__init__(file, name, delete, ...)`).
const WRAPPER_NAME: &str = "<urllib response>";

fn call_with(
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
    for (name, value) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*name).to_string()),
            value: value.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

/// `super(<class_name>, self).<method>` where `<class_name>` is one of this
/// module's classes (resolved through the live module like the global lookup
/// CPython's `super(addbase, self)` performs).
fn super_method(
    interp: &mut Interpreter,
    class_name: &str,
    self_val: &Value,
    method: &str,
) -> Result<Value> {
    let module = interp.load_module("urllib.response")?;
    let class = interp.get_attr(&module, class_name)?;
    let (ValueKind::PyClass(class_rc), ValueKind::PyInstance(inst)) =
        (class.kind(), self_val.kind())
    else {
        return Err(PyError::named(
            "TypeError",
            "super(type, obj): obj must be an instance or subtype of type".to_string(),
        ));
    };
    let proxy = Value::super_proxy(Rc::clone(class_rc), Rc::clone(inst));
    interp.get_attr(&proxy, method)
}

/// `self.__class__.__name__`.
fn class_name_of(interp: &mut Interpreter, self_val: &Value) -> Result<Value> {
    let class = interp.get_attr(self_val, "__class__")?;
    interp.get_attr(&class, "__name__")
}

fn optional_or_none(arg: &OptValueRef<'_>) -> Value {
    arg.as_value().cloned().unwrap_or_else(Value::none)
}

/// Wire what the CPython source gets from its imports and class statements:
/// `import tempfile` and `class addbase(tempfile._TemporaryFileWrapper)`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    let Ok(tempfile) = interp.load_module("tempfile") else {
        return;
    };
    module_rc
        .borrow_mut()
        .insert_attr("tempfile".to_string(), tempfile.clone());
    // `@property def status` is bound in a class body in CPython, so the
    // property knows its name (`property 'status' of 'addinfourl' object ...`).
    let addinfourl = module_rc.borrow().attrs.get("addinfourl").cloned();
    if let Some(class) = addinfourl {
        let status = match class.kind() {
            ValueKind::PyClass(cls) => cls.borrow().attrs.get("status").cloned(),
            _ => None,
        };
        if let Some(status) = status
            && let Ok(set_name) = interp.get_attr(&status, "__set_name__")
        {
            let _ = call_with(interp, set_name, &[class, Value::string("status")], &[]);
        }
    }
    let Ok(wrapper) = interp.get_attr(&tempfile, "_TemporaryFileWrapper") else {
        return;
    };
    let ValueKind::PyClass(wrapper_rc) = wrapper.kind() else {
        return;
    };
    let addbase = module_rc.borrow().attrs.get("addbase").cloned();
    if let Some(addbase) = addbase
        && let ValueKind::PyClass(addbase_rc) = addbase.kind()
    {
        crate::builtin_modules::macros::link_class_bases(addbase_rc, vec![Rc::clone(wrapper_rc)]);
    }
}

fn all_list() -> Value {
    Value::list(
        ["addbase", "addclosehook", "addinfo", "addinfourl"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
    }

    class addbase {
        constants {
            "__doc__" => Value::string(
                "Base class for addinfo and addclosehook. Is a good idea for garbage collection."
            ),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            fp: PyValueRef<'_>,
        ) -> Result<Value> {
            let init = super_method(_interp, "addbase", self_val.0, "__init__")?;
            call_with(
                _interp,
                init,
                &[fp.0.clone(), Value::string(WRAPPER_NAME)],
                &[("delete", Value::bool_(false))],
            )?;
            // Keep reference around as this was part of the original API.
            _interp.assign_attr(self_val.0.clone(), "fp", fp.0.clone())?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let name = class_name_of(_interp, self_val.0)?;
            let file = _interp.get_attr(self_val.0, "file")?;
            let name = name.as_str().unwrap_or_default().to_string();
            let file = render_value_repr(_interp, &file)?;
            let id = self_val.0.object_id();
            let id = render_value_repr(_interp, &id)?;
            Ok(Value::string(format!("<{name} at {id} whose fp = {file}>")))
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let fp = _interp.get_attr(self_val.0, "fp")?;
            let closed = _interp.get_attr(&fp, "closed")?;
            if _interp.truthy_value(&closed)? {
                return Err(PyError::named(
                    "ValueError",
                    "I/O operation on closed file".to_string(),
                ));
            }
            Ok(self_val.0.clone())
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[positional_only] _exc_type: PyValueRef<'_>,
            #[positional_only] _value: PyValueRef<'_>,
            #[positional_only] _traceback: PyValueRef<'_>,
        ) -> Result<Value> {
            let close = _interp.get_attr(self_val.0, "close")?;
            call_with(_interp, close, &[], &[])?;
            Ok(Value::none())
        }
    }

    class addclosehook : "addbase" {
        constants {
            "__doc__" => Value::string("Class to add a close hook to an open file."),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            fp: PyValueRef<'_>,
            closehook: PyValueRef<'_>,
            #[varargs] hookargs: &[Value],
        ) -> Result<Value> {
            let init = super_method(_interp, "addclosehook", self_val.0, "__init__")?;
            call_with(_interp, init, std::slice::from_ref(fp.0), &[])?;
            _interp.assign_attr(self_val.0.clone(), "closehook", closehook.0.clone())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "hookargs",
                Value::tuple(hookargs.to_vec()),
            )?;
            Ok(Value::none())
        }

        fn close(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let run_hook = |interp: &mut Interpreter| -> Result<()> {
                let closehook = interp.get_attr(self_val.0, "closehook")?;
                let hookargs = interp.get_attr(self_val.0, "hookargs")?;
                if interp.truthy_value(&closehook)? {
                    interp.assign_attr(self_val.0.clone(), "closehook", Value::none())?;
                    interp.assign_attr(self_val.0.clone(), "hookargs", Value::none())?;
                    let args = interp.collect_iterable(&hookargs)?;
                    call_with(interp, closehook, &args, &[])?;
                }
                Ok(())
            };
            let hook_result = run_hook(_interp);
            // `finally: super().close()` — runs whether or not the hook raised.
            let close_result = super_method(_interp, "addclosehook", self_val.0, "close")
                .and_then(|close| call_with(_interp, close, &[], &[]));
            close_result?;
            hook_result?;
            Ok(Value::none())
        }
    }

    class addinfo : "addbase" {
        constants {
            "__doc__" => Value::string("class to add an info() method to an open file."),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            fp: PyValueRef<'_>,
            headers: PyValueRef<'_>,
        ) -> Result<Value> {
            let init = super_method(_interp, "addinfo", self_val.0, "__init__")?;
            call_with(_interp, init, std::slice::from_ref(fp.0), &[])?;
            _interp.assign_attr(self_val.0.clone(), "headers", headers.0.clone())?;
            Ok(Value::none())
        }

        fn info(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "headers")
        }
    }

    class addinfourl : "addinfo" {
        constants {
            "__doc__" => Value::string("class to add info() and geturl() methods to an open file."),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            fp: PyValueRef<'_>,
            headers: PyValueRef<'_>,
            url: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] code: OptValueRef<'_>,
        ) -> Result<Value> {
            let init = super_method(_interp, "addinfourl", self_val.0, "__init__")?;
            call_with(_interp, init, &[fp.0.clone(), headers.0.clone()], &[])?;
            _interp.assign_attr(self_val.0.clone(), "url", url.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "code", optional_or_none(&code))?;
            Ok(Value::none())
        }

        #[getter]
        fn status(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "code")
        }

        fn getcode(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "code")
        }

        fn geturl(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "url")
        }
    }
}
