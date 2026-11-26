// `xml.dom.minicompat` module — 100% native-Rust Python 3.12 DOM compatibility
// helpers: `NodeList`, `EmptyNodeList`, `StringTypes`, and `defproperty`
// (issue #3260).
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/xml/dom/minicompat.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value};

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let builtins = interp.load_module("builtins")?;
    let str_cls = interp.get_attr(&builtins, "str")?;
    module
        .borrow_mut()
        .insert_attr("StringTypes".to_string(), Value::tuple(vec![str_cls]));
    Ok(())
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_pos(interp, m, args)
}

fn raise_no_mod_err(interp: &mut Interpreter, msg: &str) -> PyError {
    if let Ok(dom_mod) = interp.load_module("xml.dom")
        && let Ok(cls) = interp.get_attr(&dom_mod, "NoModificationAllowedErr")
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(msg)])
    {
        return PyError::raised(inst);
    }
    PyError::named("RuntimeError", msg)
}

struct DefPropGetter {
    getter_name: String,
}

struct DefPropSetter {
    prop_name: String,
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.dom"),
        "__all__" => Value::list(vec![
            Value::string("NodeList"),
            Value::string("EmptyNodeList"),
            Value::string("StringTypes"),
            Value::string("defproperty"),
        ]),
        "StringTypes" => Value::tuple(vec![Value::builtin_function("str")]),
    }

    #[native_state(DefPropGetter)]
    class _DefPropGetter {
        fn __call__(#[positional_only] self_val: PyValueRef<'_>, obj: PyValue) -> Result<Value> {
            let gname = with_state(self_val.0, |st| Ok(st.getter_name.clone()))?;
            call_method(_interp, &obj.0, &gname, &[])
        }
    }

    #[native_state(DefPropSetter)]
    class _DefPropSetter {
        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            _obj: PyValue,
            _val: PyValue,
        ) -> Result<Value> {
            let pname = with_state(self_val.0, |st| Ok(st.prop_name.clone()))?;
            Err(raise_no_mod_err(
                _interp,
                &format!("attempt to modify read-only attribute {pname}"),
            ))
        }
    }

    fn defproperty(klass: PyValue, name: PyStr<'_>, doc: PyValue) -> Result<Value> {
        let getter_name = format!("_get_{}", name.0);
        let mc_mod = _interp.load_module("xml.dom.minicompat")?;
        let g_cls = _interp.get_attr(&mc_mod, "_DefPropGetter")?;
        let s_cls = _interp.get_attr(&mc_mod, "_DefPropSetter")?;
        let getter = call_pos(_interp, g_cls, &[])?;
        crate::builtin_modules::macros::native_state::set_native_state(
            &getter,
            DefPropGetter { getter_name },
        )?;
        let setter = call_pos(_interp, s_cls, &[])?;
        crate::builtin_modules::macros::native_state::set_native_state(
            &setter,
            DefPropSetter {
                prop_name: name.0.to_string(),
            },
        )?;
        let builtins = _interp.load_module("builtins")?;
        let prop_cls = _interp.get_attr(&builtins, "property")?;
        let prop = call_pos(_interp, prop_cls, &[getter, setter, Value::none(), doc.0])?;
        _interp.assign_attr(klass.0, &name.0, prop)?;
        Ok(Value::none())
    }

    class NodeList("list") {
        fn item(#[positional_only] self_val: PyValueRef<'_>, index: PyIndex) -> Result<Value> {
            let idx = index.expect_i64(FN_NAME, "index")?;
            let len_v = call_pos(_interp, Value::builtin_function("len"), std::slice::from_ref(self_val.0))?;
            let len = len_v.as_int().unwrap_or(0);
            if idx >= 0 && idx < len {
                call_method(_interp, self_val.0, "__getitem__", &[Value::int(idx)])
            } else {
                Ok(Value::none())
            }
        }

        #[getter]
        fn length(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_pos(_interp, Value::builtin_function("len"), std::slice::from_ref(self_val.0))
        }

        #[setter("length")]
        fn set_length(#[positional_only] _self: PyValueRef<'_>, _val: PyValue) -> Result<Value> {
            Err(raise_no_mod_err(
                _interp,
                "attempt to modify read-only attribute 'length'",
            ))
        }

        fn __getstate__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            call_pos(_interp, Value::builtin_function("list"), std::slice::from_ref(self_val.0))
        }

        fn __setstate__(#[positional_only] self_val: PyValueRef<'_>, state: PyValue) -> Result<Value> {
            call_method(_interp, self_val.0, "clear", &[])?;
            call_method(_interp, self_val.0, "extend", &[state.0])?;
            Ok(Value::none())
        }
    }

    class EmptyNodeList("tuple") {
        fn __add__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let mc_mod = _interp.load_module("xml.dom.minicompat")?;
            let nl_cls = _interp.get_attr(&mc_mod, "NodeList")?;
            let nl = call_pos(_interp, nl_cls, &[])?;
            call_method(_interp, &nl, "extend", &[other.0])?;
            Ok(nl)
        }

        fn __radd__(#[positional_only] _self: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let mc_mod = _interp.load_module("xml.dom.minicompat")?;
            let nl_cls = _interp.get_attr(&mc_mod, "NodeList")?;
            let nl = call_pos(_interp, nl_cls, &[])?;
            call_method(_interp, &nl, "extend", &[other.0])?;
            Ok(nl)
        }

        fn item(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _index: OptValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        #[getter]
        fn length(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::int(0))
        }

        #[setter("length")]
        fn set_length(#[positional_only] _self: PyValueRef<'_>, _val: PyValue) -> Result<Value> {
            Err(raise_no_mod_err(
                _interp,
                "attempt to modify read-only attribute 'length'",
            ))
        }
    }
}
