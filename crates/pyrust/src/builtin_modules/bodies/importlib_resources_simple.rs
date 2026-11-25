// `importlib.resources.simple` module — 100% native-Rust Python 3.12
// implementation (`SimpleReader`, `ResourceHandle`, `ResourceContainer`,
// `TraversableReader`).
//
// Reference: <https://docs.python.org/3/library/importlib.resources.html>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyClass, PyKey, PyModule, PySet, Value, ValueKind};

struct AbstractMethodState {
    func: Value,
    bound_cls: Value,
}

struct BoundAbstractMethodState {
    func: Value,
    instance: Value,
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_with_args_kwargs(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &IndexMap<String, Value>,
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some(k.clone()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn simple_cls(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("importlib.resources.simple")?;
    interp.get_attr(&m, name)
}

fn link_bases(cls_val: &Value, bases: &[&Value], abc_meta: &Rc<RefCell<PyClass>>) {
    let ValueKind::PyClass(cls_rc) = cls_val.kind() else {
        return;
    };
    let mut base_rcs: Vec<Rc<RefCell<PyClass>>> = Vec::new();
    for b in bases {
        if let ValueKind::PyClass(rc) = b.kind() {
            base_rcs.push(Rc::clone(rc));
        }
    }
    if !base_rcs.is_empty() {
        crate::builtin_modules::macros::link_class_bases(cls_rc, base_rcs);
    }
    cls_rc.borrow_mut().metatype = Some(Rc::clone(abc_meta));
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let abc_mod = interp.load_module("abc")?;
    let abc_meta_val = interp.get_attr(&abc_mod, "ABCMeta")?;
    let abc_cls_val = interp.get_attr(&abc_mod, "ABC")?;
    let ValueKind::PyClass(abc_meta) = abc_meta_val.kind() else {
        return Ok(());
    };

    let res_abc = interp.load_module("importlib.resources.abc")?;
    let trav_cls = interp.get_attr(&res_abc, "Traversable")?;
    let tr_res_cls = interp.get_attr(&res_abc, "TraversableResources")?;

    let abs_cls = module
        .borrow()
        .attrs
        .get("_AbstractMethod")
        .cloned()
        .ok_or_else(|| {
            PyError::runtime("importlib.resources.simple: missing _AbstractMethod".into())
        })?;
    let bound_cls = module
        .borrow()
        .attrs
        .get("_BoundAbstractMethod")
        .cloned()
        .ok_or_else(|| {
            PyError::runtime("importlib.resources.simple: missing _BoundAbstractMethod".into())
        })?;

    let sr_opt = module.borrow().attrs.get("SimpleReader").cloned();
    if let Some(sr) = sr_opt {
        link_bases(&sr, &[&abc_cls_val], abc_meta);
        if let ValueKind::PyClass(cls_rc) = sr.kind() {
            let mut abs_set = PySet::default();
            for name in ["package", "children", "resources", "open_binary"] {
                abs_set.insert(PyKey::str_from(name));
            }
            cls_rc.borrow_mut().attrs.insert(
                "__abstractmethods__".to_string(),
                pyrust_builtins::frozenset::frozenset(abs_set),
            );
            cls_rc
                .borrow_mut()
                .attrs
                .insert("_abc_registry".to_string(), Value::set(PySet::default()));
            for mname in ["children", "resources", "open_binary"] {
                let orig = cls_rc.borrow().attrs.get(mname).cloned();
                if let Some(func) = orig {
                    let wrapped = call_pos(interp, abs_cls.clone(), &[func, bound_cls.clone()])?;
                    cls_rc.borrow_mut().attrs.insert(mname.to_string(), wrapped);
                }
            }
            let prop_opt = cls_rc.borrow().attrs.get("package").cloned();
            if let Some(prop_val) = prop_opt {
                let fget = interp.get_attr(&prop_val, "fget").unwrap_or(prop_val);
                let wrapped = call_pos(interp, abs_cls.clone(), &[fget, bound_cls.clone()])?;
                let new_prop =
                    pyrust_builtins::property::property(wrapped, Value::none(), Value::none());
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("package".to_string(), new_prop);
            }
        }
        let tr_opt = module.borrow().attrs.get("TraversableReader").cloned();
        if let Some(tr) = tr_opt {
            link_bases(&tr, &[&tr_res_cls, &sr], abc_meta);
            if let ValueKind::PyClass(cls_rc) = tr.kind() {
                let mut abs_set = PySet::default();
                for name in ["package", "children", "resources", "open_binary"] {
                    abs_set.insert(PyKey::str_from(name));
                }
                cls_rc.borrow_mut().attrs.insert(
                    "__abstractmethods__".to_string(),
                    pyrust_builtins::frozenset::frozenset(abs_set),
                );
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("_abc_registry".to_string(), Value::set(PySet::default()));
            }
        }
    }

    for name in ["ResourceHandle", "ResourceContainer"] {
        let cls_opt = module.borrow().attrs.get(name).cloned();
        if let Some(cls_val) = cls_opt {
            link_bases(&cls_val, &[&trav_cls], abc_meta);
            if let ValueKind::PyClass(cls_rc) = cls_val.kind() {
                cls_rc.borrow_mut().attrs.insert(
                    "__abstractmethods__".to_string(),
                    pyrust_builtins::frozenset::frozenset(PySet::default()),
                );
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("_abc_registry".to_string(), Value::set(PySet::default()));
            }
        }
    }

    {
        let mut m = module.borrow_mut();
        m.insert_attr("Traversable".to_string(), trav_cls);
        m.insert_attr("TraversableResources".to_string(), tr_res_cls);
        m.insert_attr("abc".to_string(), abc_mod);
        m.attrs.shift_remove("_AbstractMethod");
        m.attrs.shift_remove("_BoundAbstractMethod");
    }
    Ok(())
}

pyrust_module! {
    #[native_state(AbstractMethodState)]
    class _AbstractMethod {
        constants {
            "__isabstractmethod__" => Value::bool_(true),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValue,
            bound_cls: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                AbstractMethodState {
                    func: func.0,
                    bound_cls: bound_cls.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __get__(
            #[positional_only] self_val: PyValueRef<'_>,
            obj: PyValue,
            #[default(OptionalArg::Omitted)] _objtype: OptValueRef<'_>,
        ) -> Result<Value> {
            if obj.0.is_none() {
                return Ok(self_val.0.clone());
            }
            let (func, bound_cls) =
                with_state(self_val.0, |st| Ok((st.func.clone(), st.bound_cls.clone())))?;
            call_pos(_interp, bound_cls, &[func, obj.0])
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let func = with_state(self_val.0, |st| Ok(st.func.clone()))?;
            call_with_args_kwargs(_interp, func, args, &kwargs)
        }
    }

    #[native_state(BoundAbstractMethodState)]
    class _BoundAbstractMethod {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            func: PyValue,
            instance: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                BoundAbstractMethodState {
                    func: func.0,
                    instance: instance.0,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let (func, inst) =
                with_state(self_val.0, |st| Ok((st.func.clone(), st.instance.clone())))?;
            let mut full_pos = Vec::with_capacity(args.len() + 1);
            full_pos.push(inst);
            full_pos.extend_from_slice(args);
            call_with_args_kwargs(_interp, func, &full_pos, &kwargs)
        }
    }

    class SimpleReader {
        #[getter]
        fn package(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "package is abstract"))
        }

        fn children(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "children is abstract"))
        }

        fn resources(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "resources is abstract"))
        }

        fn open_binary(
            #[positional_only] _self: PyValueRef<'_>,
            _resource: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "open_binary is abstract"))
        }

        #[getter]
        fn name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pkg = _interp.get_attr(self_val.0, "package")?;
            let s = pkg.as_str().unwrap_or_default();
            let last = s.rsplit_once('.').map(|(_, tail)| tail).unwrap_or(s);
            Ok(Value::string(last))
        }
    }

    class ResourceHandle {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            parent: PyValue,
            name: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "parent", parent.0)?;
            _interp.assign_attr(self_val.0.clone(), "name", name.0)?;
            Ok(Value::none())
        }

        fn is_file(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(true))
        }

        fn is_dir(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(false))
        }

        fn open(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] mode: OptValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mode_str = mode.as_value().and_then(|v| v.as_str()).unwrap_or("r");
            let parent = _interp.get_attr(self_val.0, "parent")?;
            let name = _interp.get_attr(self_val.0, "name")?;
            let ob = _interp.get_attr(&parent, "open_binary")?;
            let stream = call_pos(_interp, ob, &[name])?;
            if mode_str.contains('b') {
                return Ok(stream);
            }
            let io_mod = _interp.load_module("io")?;
            if let Ok(tiw) = _interp.get_attr(&io_mod, "TextIOWrapper") {
                let mut pos = Vec::with_capacity(args.len() + 1);
                pos.push(stream);
                pos.extend_from_slice(args);
                return call_with_args_kwargs(_interp, tiw, &pos, &kwargs);
            }
            // Fallback: decode bytes into StringIO.
            let read_fn = _interp.get_attr(&stream, "read")?;
            let raw = call_pos(_interp, read_fn, &[])?;
            let decode_fn = _interp.get_attr(&raw, "decode")?;
            let enc = kwargs
                .get("encoding")
                .filter(|v| !v.is_none())
                .cloned()
                .unwrap_or_else(|| Value::string("utf-8"));
            let errs = kwargs
                .get("errors")
                .filter(|v| !v.is_none())
                .cloned()
                .unwrap_or_else(|| Value::string("strict"));
            let text = call_pos(_interp, decode_fn, &[enc, errs])?;
            let sio = _interp.get_attr(&io_mod, "StringIO")?;
            call_pos(_interp, sio, &[text])
        }

        fn joinpath(
            #[positional_only] _self: PyValueRef<'_>,
            _name: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("RuntimeError", "Cannot traverse into a resource"))
        }

        fn iterdir(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotADirectoryError", "ResourceHandle is not a directory"))
        }
    }

    class ResourceContainer {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            reader: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "reader", reader.0)?;
            Ok(Value::none())
        }

        #[getter]
        fn name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let reader = _interp.get_attr(self_val.0, "reader")?;
            _interp.get_attr(&reader, "name")
        }

        fn is_dir(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(true))
        }

        fn is_file(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(false))
        }

        fn iterdir(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let reader = _interp.get_attr(self_val.0, "reader")?;
            let handle_cls = simple_cls(_interp, "ResourceHandle")?;
            let container_cls = simple_cls(_interp, "ResourceContainer")?;
            let mut out = Vec::new();
            let res_fn = _interp.get_attr(&reader, "resources")?;
            let res_iter = call_pos(_interp, res_fn, &[])?;
            for name in _interp.collect_iterable(&res_iter)? {
                out.push(call_pos(_interp, handle_cls.clone(), &[reader.clone(), name])?);
            }
            let ch_fn = _interp.get_attr(&reader, "children")?;
            let ch_iter = call_pos(_interp, ch_fn, &[])?;
            for child in _interp.collect_iterable(&ch_iter)? {
                out.push(call_pos(_interp, container_cls.clone(), &[child])?);
            }
            Ok(Value::list(out))
        }

        fn open(
            #[positional_only] _self: PyValueRef<'_>,
            #[varargs] _args: &[Value],
            #[varkwargs] _kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            Err(PyError::named("IsADirectoryError", "Cannot open a container"))
        }
    }

    class TraversableReader {
        fn files(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let container_cls = simple_cls(_interp, "ResourceContainer")?;
            call_pos(_interp, container_cls, std::slice::from_ref(self_val.0))
        }
    }
}
