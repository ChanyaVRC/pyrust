// `importlib.resources.abc` module — 100% native-Rust Python 3.12
// implementation (`ResourceReader`, `Traversable`, `TraversableResources`,
// `TraversalError`).
//
// Reference: <https://docs.python.org/3/library/importlib.resources.abc.html>

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

fn close_stream_best_effort(interp: &mut Interpreter, strm: &Value) {
    if let Ok(exit_fn) = interp.get_attr(strm, "__exit__") {
        let _ = call_pos(
            interp,
            exit_fn,
            &[Value::none(), Value::none(), Value::none()],
        );
    } else if let Ok(close_fn) = interp.get_attr(strm, "close") {
        let _ = call_pos(interp, close_fn, &[]);
    }
}

struct AbcWiringCtx<'a> {
    abc_meta: &'a Rc<RefCell<PyClass>>,
    abs_cls: &'a Value,
    bound_cls: &'a Value,
}

fn mark_abc(
    interp: &mut Interpreter,
    ctx: &AbcWiringCtx<'_>,
    cls_val: &Value,
    abc_base: Option<&Rc<RefCell<PyClass>>>,
    own_abs_methods: &[&str],
    own_abs_props: &[&str],
    all_abs: &[&str],
) -> Result<()> {
    let ValueKind::PyClass(cls_rc) = cls_val.kind() else {
        return Ok(());
    };
    {
        let mut cls = cls_rc.borrow_mut();
        cls.metatype = Some(Rc::clone(ctx.abc_meta));
        if let Some(base) = abc_base
            && cls.base.is_none()
        {
            cls.base = Some(Rc::clone(base));
            base.borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(cls_rc));
        }
        let mut abs_set = PySet::default();
        for &m in all_abs {
            abs_set.insert(PyKey::str_from(m));
        }
        cls.attrs.insert(
            "__abstractmethods__".to_string(),
            pyrust_builtins::frozenset::frozenset(abs_set),
        );
        cls.attrs
            .insert("_abc_registry".to_string(), Value::set(PySet::default()));
    }
    for &method_name in own_abs_methods {
        let orig = cls_rc.borrow().attrs.get(method_name).cloned();
        if let Some(func) = orig {
            let wrapped = call_pos(interp, ctx.abs_cls.clone(), &[func, ctx.bound_cls.clone()])?;
            cls_rc
                .borrow_mut()
                .attrs
                .insert(method_name.to_string(), wrapped);
        }
    }
    for &prop_name in own_abs_props {
        let orig = cls_rc.borrow().attrs.get(prop_name).cloned();
        if let Some(prop_val) = orig {
            let fget = interp.get_attr(&prop_val, "fget").unwrap_or(prop_val);
            let wrapped_fget =
                call_pos(interp, ctx.abs_cls.clone(), &[fget, ctx.bound_cls.clone()])?;
            let new_prop =
                pyrust_builtins::property::property(wrapped_fget, Value::none(), Value::none());
            cls_rc
                .borrow_mut()
                .attrs
                .insert(prop_name.to_string(), new_prop);
        }
    }
    Ok(())
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let abc_mod = interp.load_module("abc")?;
    let abc_meta_val = interp.get_attr(&abc_mod, "ABCMeta")?;
    let abc_cls_val = interp.get_attr(&abc_mod, "ABC")?;
    let (ValueKind::PyClass(abc_meta), ValueKind::PyClass(abc_cls)) =
        (abc_meta_val.kind(), abc_cls_val.kind())
    else {
        return Ok(());
    };

    let abs_cls = module
        .borrow()
        .attrs
        .get("_AbstractMethod")
        .cloned()
        .ok_or_else(|| {
            PyError::runtime("importlib.resources.abc: missing _AbstractMethod".into())
        })?;
    let bound_cls = module
        .borrow()
        .attrs
        .get("_BoundAbstractMethod")
        .cloned()
        .ok_or_else(|| {
            PyError::runtime("importlib.resources.abc: missing _BoundAbstractMethod".into())
        })?;
    let ctx = AbcWiringCtx {
        abc_meta,
        abs_cls: &abs_cls,
        bound_cls: &bound_cls,
    };

    let rr_opt = module.borrow().attrs.get("ResourceReader").cloned();
    if let Some(rr) = rr_opt {
        mark_abc(
            interp,
            &ctx,
            &rr,
            Some(abc_cls),
            &["open_resource", "resource_path", "is_resource", "contents"],
            &[],
            &["open_resource", "resource_path", "is_resource", "contents"],
        )?;
    }

    let trav_opt = module.borrow().attrs.get("Traversable").cloned();
    if let Some(trav) = trav_opt {
        mark_abc(
            interp,
            &ctx,
            &trav,
            Some(abc_cls),
            &["iterdir", "is_dir", "is_file", "open"],
            &["name"],
            &["iterdir", "is_dir", "is_file", "open", "name"],
        )?;
        // Register `pathlib.Path` as a virtual subclass of `Traversable`.
        if let Ok(pathlib_mod) = interp.load_module("pathlib")
            && let Ok(path_cls) = interp.get_attr(&pathlib_mod, "Path")
            && let Ok(reg_fn) = interp.get_attr(&trav, "register")
        {
            let _ = call_pos(interp, reg_fn, &[path_cls]);
        }
    }

    let tr_opt = module.borrow().attrs.get("TraversableResources").cloned();
    if let Some(tr) = tr_opt {
        mark_abc(interp, &ctx, &tr, None, &["files"], &[], &["files"])?;
    }

    module.borrow_mut().insert_attr("abc".to_string(), abc_mod);
    {
        let mut m = module.borrow_mut();
        m.attrs.shift_remove("_AbstractMethod");
        m.attrs.shift_remove("_BoundAbstractMethod");
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("ResourceReader"),
            Value::string("Traversable"),
            Value::string("TraversableResources"),
        ]),
    }

    exception TraversalError(Exception);

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

    class ResourceReader {
        fn open_resource(
            #[positional_only] _self: PyValueRef<'_>,
            _resource: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("FileNotFoundError", "open_resource is abstract"))
        }

        fn resource_path(
            #[positional_only] _self: PyValueRef<'_>,
            _resource: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("FileNotFoundError", "resource_path is abstract"))
        }

        fn is_resource(
            #[positional_only] _self: PyValueRef<'_>,
            _path: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("FileNotFoundError", "is_resource is abstract"))
        }

        fn contents(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("FileNotFoundError", "contents is abstract"))
        }
    }

    class Traversable {
        fn iterdir(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "iterdir is abstract"))
        }

        fn read_bytes(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let open_fn = _interp.get_attr(self_val.0, "open")?;
            let strm = call_pos(_interp, open_fn, &[Value::string("rb")])?;
            let read_res = (|| -> Result<Value> {
                let read_fn = _interp.get_attr(&strm, "read")?;
                call_pos(_interp, read_fn, &[])
            })();
            close_stream_best_effort(_interp, &strm);
            read_res
        }

        fn read_text(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] errors: OptValueRef<'_>,
        ) -> Result<Value> {
            let open_fn = _interp.get_attr(self_val.0, "open")?;
            let mut kw = IndexMap::new();
            if let Some(enc) = encoding.as_value() {
                kw.insert("encoding".to_string(), enc.clone());
            }
            if let Some(err) = errors.as_value() {
                kw.insert("errors".to_string(), err.clone());
            }
            let strm = call_with_args_kwargs(_interp, open_fn, &[], &kw)?;
            let read_res = (|| -> Result<Value> {
                let read_fn = _interp.get_attr(&strm, "read")?;
                call_pos(_interp, read_fn, &[])
            })();
            close_stream_best_effort(_interp, &strm);
            read_res
        }

        fn is_dir(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "is_dir is abstract"))
        }

        fn is_file(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "is_file is abstract"))
        }

        fn joinpath(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] descendants: &[Value],
        ) -> Result<Value> {
            let mut current = self_val.0.clone();
            for desc in descendants {
                let desc_str = desc.as_str().ok_or_else(|| {
                    PyError::named("TypeError", "joinpath segments must be str")
                })?;
                for part in desc_str.split('/').filter(|s| !s.is_empty()) {
                    let iterdir_fn = _interp.get_attr(&current, "iterdir")?;
                    let items = call_pos(_interp, iterdir_fn, &[])?;
                    let collected = _interp.collect_iterable(&items)?;
                    let mut found = None;
                    for item in collected {
                        if let Ok(n) = _interp.get_attr(&item, "name")
                            && n.as_str() == Some(part)
                        {
                            found = Some(item);
                            break;
                        }
                    }
                    match found {
                        Some(next) => current = next,
                        None => {
                            return Err(PyError::named(
                                "FileNotFoundError",
                                format!("resource not found: {part}"),
                            ));
                        }
                    }
                }
            }
            Ok(current)
        }

        fn __truediv__(
            #[positional_only] self_val: PyValueRef<'_>,
            child: PyValue,
        ) -> Result<Value> {
            let jp = _interp.get_attr(self_val.0, "joinpath")?;
            call_pos(_interp, jp, &[child.0])
        }

        fn open(
            #[positional_only] _self: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] _mode: OptValueRef<'_>,
            #[varargs] _args: &[Value],
            #[varkwargs] _kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "open is abstract"))
        }

        #[getter]
        fn name(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "name is abstract"))
        }
    }

    class TraversableResources("ResourceReader") {
        fn files(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "files is abstract"))
        }

        fn open_resource(
            #[positional_only] self_val: PyValueRef<'_>,
            resource: PyValue,
        ) -> Result<Value> {
            let files_fn = _interp.get_attr(self_val.0, "files")?;
            let root = call_pos(_interp, files_fn, &[])?;
            let jp = _interp.get_attr(&root, "joinpath")?;
            let target = call_pos(_interp, jp, &[resource.0])?;
            let open_fn = _interp.get_attr(&target, "open")?;
            call_pos(_interp, open_fn, &[Value::string("rb")])
        }

        fn resource_path(
            #[positional_only] _self: PyValueRef<'_>,
            resource: PyValue,
        ) -> Result<Value> {
            let msg = resource
                .0
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| "resource".to_string());
            Err(PyError::named("FileNotFoundError", msg))
        }

        fn is_resource(
            #[positional_only] self_val: PyValueRef<'_>,
            path: PyValue,
        ) -> Result<Value> {
            let files_fn = _interp.get_attr(self_val.0, "files")?;
            let root = call_pos(_interp, files_fn, &[])?;
            let jp = _interp.get_attr(&root, "joinpath")?;
            let target = call_pos(_interp, jp, &[path.0])?;
            let is_file_fn = _interp.get_attr(&target, "is_file")?;
            call_pos(_interp, is_file_fn, &[])
        }

        fn contents(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let files_fn = _interp.get_attr(self_val.0, "files")?;
            let root = call_pos(_interp, files_fn, &[])?;
            let iterdir_fn = _interp.get_attr(&root, "iterdir")?;
            let items = call_pos(_interp, iterdir_fn, &[])?;
            let collected = _interp.collect_iterable(&items)?;
            let mut names = Vec::with_capacity(collected.len());
            for item in collected {
                let n = _interp.get_attr(&item, "name")?;
                names.push(n);
            }
            Ok(Value::list(names))
        }
    }
}
