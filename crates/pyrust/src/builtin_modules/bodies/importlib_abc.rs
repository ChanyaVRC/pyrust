// `importlib.abc` module — 100% native-Rust Python 3.12 implementation using
// PyRust's expanded macro toolkit (`pyrust_module!`, `class`, `#[staticmethod]`,
// `#[native_state]`, `#[varargs]`, `#[varkwargs]`).
//
// Exposes Python 3.12's import ABC hierarchy (`Finder`, `MetaPathFinder`,
// `PathEntryFinder`, `Loader`, `ResourceLoader`, `InspectLoader`,
// `ExecutionLoader`, `FileLoader`, `SourceLoader`) inheriting from `abc.ABC`,
// plus re-exports of `Traversable`, `TraversableResources`, and
// `ResourceReader` from `importlib.resources.abc`.
//
// Reference: <https://docs.python.org/3/library/importlib.html#module-importlib.abc>

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyPath, PyStr, PyValue, PyValueRef,
};
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

fn py_eq(interp: &mut Interpreter, lhs: &Value, rhs: &Value) -> Result<bool> {
    let res = interp.eval_binary(lhs.clone(), BinaryOp::Eq, rhs.clone())?;
    interp.truthy_value(&res)
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

fn file_loader_check_name(
    interp: &mut Interpreter,
    self_val: &Value,
    fullname: &Value,
) -> Result<String> {
    let loader_name = interp
        .get_attr(self_val, "name")?
        .as_str()
        .unwrap_or_default()
        .to_string();
    if fullname.is_none() {
        return Ok(loader_name);
    }
    let req = fullname.as_str().unwrap_or_default();
    if loader_name != req {
        return Err(PyError::import_error(
            "ImportError",
            format!("loader for {loader_name} cannot handle {req}"),
            Some(req.to_string()),
        ));
    }
    Ok(req.to_string())
}

fn compute_is_package(
    interp: &mut Interpreter,
    self_val: &Value,
    fullname: &Value,
) -> Result<Value> {
    let get_fn = interp.get_attr(self_val, "get_filename")?;
    let filename_val = call_pos(interp, get_fn, std::slice::from_ref(fullname))?;
    let filename_str = filename_val.as_str().unwrap_or_default();
    let base_file = std::path::Path::new(filename_str)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(filename_str);
    let stem = base_file
        .rsplit_once('.')
        .map(|(b, _)| b)
        .unwrap_or(base_file);
    let fullname_str = fullname.as_str().unwrap_or_default();
    let tail = fullname_str
        .rsplit_once('.')
        .map(|(_, t)| t)
        .unwrap_or(fullname_str);
    Ok(Value::bool_(stem == "__init__" && tail != "__init__"))
}

fn compile_source_to_code(interp: &mut Interpreter, data: &Value, path: &Value) -> Result<Value> {
    let compile_fn = Value::builtin_function("compile");
    let mut kw = IndexMap::new();
    kw.insert("dont_inherit".to_string(), Value::bool_(true));
    call_with_args_kwargs(
        interp,
        compile_fn,
        &[data.clone(), path.clone(), Value::string("exec")],
        &kw,
    )
}

fn exec_module_via_get_code(
    interp: &mut Interpreter,
    self_val: &Value,
    module: &Value,
) -> Result<Value> {
    let mod_name = interp.get_attr(module, "__name__")?;
    let get_code_fn = interp.get_attr(self_val, "get_code")?;
    let code = call_pos(interp, get_code_fn, std::slice::from_ref(&mod_name))?;
    if code.is_none() {
        let name_str = mod_name.as_str().unwrap_or("<unknown>");
        return Err(PyError::import_error(
            "ImportError",
            format!("cannot load module '{name_str}' when get_code() returns None"),
            Some(name_str.to_string()),
        ));
    }
    let mod_dict = interp.get_attr(module, "__dict__")?;
    call_pos(interp, Value::builtin_function("exec"), &[code, mod_dict])?;
    Ok(Value::none())
}

struct AbcClassCtx<'a> {
    abc_meta: &'a Rc<RefCell<PyClass>>,
    abstract_method_cls: &'a Value,
    bound_method_cls: &'a Value,
}

fn mark_class_abc(
    interp: &mut Interpreter,
    ctx: &AbcClassCtx<'_>,
    cls_val: &Value,
    abc_base: Option<&Rc<RefCell<PyClass>>>,
    own_abstract_methods: &[&str],
    all_abstract_methods: &[&str],
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
        for &m in all_abstract_methods {
            abs_set.insert(PyKey::str_from(m));
        }
        cls.attrs.insert(
            "__abstractmethods__".to_string(),
            pyrust_builtins::frozenset::frozenset(abs_set),
        );
        cls.attrs.insert(
            "_abc_impl".to_string(),
            crate::builtin_modules::abc::new_abc_impl(),
        );
    }
    for &method_name in own_abstract_methods {
        let orig = cls_rc.borrow().attrs.get(method_name).cloned();
        if let Some(func) = orig {
            let wrapped = call_pos(
                interp,
                ctx.abstract_method_cls.clone(),
                &[func, ctx.bound_method_cls.clone()],
            )?;
            cls_rc
                .borrow_mut()
                .attrs
                .insert(method_name.to_string(), wrapped);
        }
    }
    Ok(())
}

/// Wire `abc.ABC` / `abc.ABCMeta`, abstract-method descriptors, virtual
/// subclass registrations against `importlib.machinery`, and re-exports from
/// `importlib.resources.abc`.
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

    let abstract_method_cls = module
        .borrow()
        .attrs
        .get("_AbstractMethod")
        .cloned()
        .ok_or_else(|| PyError::runtime("importlib.abc: missing _AbstractMethod".into()))?;
    let bound_method_cls = module
        .borrow()
        .attrs
        .get("_BoundAbstractMethod")
        .cloned()
        .ok_or_else(|| PyError::runtime("importlib.abc: missing _BoundAbstractMethod".into()))?;
    let ctx = AbcClassCtx {
        abc_meta,
        abstract_method_cls: &abstract_method_cls,
        bound_method_cls: &bound_method_cls,
    };

    let specs: &[(&str, bool, &[&str], &[&str])] = &[
        ("Finder", true, &[], &[]),
        ("MetaPathFinder", true, &[], &[]),
        ("PathEntryFinder", true, &[], &[]),
        ("Loader", true, &[], &[]),
        ("ResourceLoader", false, &["get_data"], &["get_data"]),
        ("InspectLoader", false, &["get_source"], &["get_source"]),
        (
            "ExecutionLoader",
            false,
            &["get_filename"],
            &["get_filename", "get_source"],
        ),
        ("FileLoader", false, &[], &["get_source"]),
        ("SourceLoader", false, &[], &["get_filename", "get_data"]),
    ];

    for &(name, attach_abc_base, own_abs, all_abs) in specs {
        let cls_opt = module.borrow().attrs.get(name).cloned();
        if let Some(cls_val) = cls_opt {
            let base = if attach_abc_base { Some(abc_cls) } else { None };
            mark_class_abc(interp, &ctx, &cls_val, base, own_abs, all_abs)?;
        }
    }

    // Re-export Traversable, TraversableResources, ResourceReader from
    // `importlib.resources.abc` so they share exact class identity.
    if let Ok(res_abc) = interp.load_module("importlib.resources.abc") {
        for name in ["Traversable", "TraversableResources", "ResourceReader"] {
            if let Ok(val) = interp.get_attr(&res_abc, name) {
                module.borrow_mut().insert_attr(name.to_string(), val);
            }
        }
    }

    // Register concrete `importlib.machinery` classes as virtual subclasses of
    // the appropriate ABCs (matching CPython 3.12's `importlib/abc.py`).
    if let Ok(machinery) = interp.load_module("importlib.machinery") {
        let registrations: &[(&str, &[&str])] = &[
            (
                "MetaPathFinder",
                &["BuiltinImporter", "FrozenImporter", "PathFinder"],
            ),
            ("PathEntryFinder", &["FileFinder"]),
            (
                "InspectLoader",
                &["BuiltinImporter", "FrozenImporter", "NamespaceLoader"],
            ),
            ("ExecutionLoader", &["ExtensionFileLoader"]),
            (
                "FileLoader",
                &[
                    "SourceFileLoader",
                    "SourcelessFileLoader",
                    "ExtensionFileLoader",
                ],
            ),
            ("SourceLoader", &["SourceFileLoader"]),
        ];
        for &(abc_name, concrete_names) in registrations {
            let Some(abc_target) = module.borrow().attrs.get(abc_name).cloned() else {
                continue;
            };
            let Ok(reg_fn) = interp.get_attr(&abc_target, "register") else {
                continue;
            };
            for &concrete_name in concrete_names {
                if let Ok(concrete) = interp.get_attr(&machinery, concrete_name) {
                    let _ = call_pos(interp, reg_fn.clone(), &[concrete]);
                }
            }
        }
        module
            .borrow_mut()
            .insert_attr("machinery".to_string(), machinery);
    }

    module.borrow_mut().insert_attr("abc".to_string(), abc_mod);
    if let Ok(warnings_mod) = interp.load_module("warnings") {
        module
            .borrow_mut()
            .insert_attr("warnings".to_string(), warnings_mod);
    }

    // Remove private descriptor helpers from public module dict.
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
            Value::string("Loader"),
            Value::string("MetaPathFinder"),
            Value::string("PathEntryFinder"),
            Value::string("ResourceLoader"),
            Value::string("InspectLoader"),
            Value::string("ExecutionLoader"),
            Value::string("FileLoader"),
            Value::string("SourceLoader"),
        ]),
    }

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

    class Finder {
        fn invalidate_caches(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }

    class MetaPathFinder {
        fn find_spec(
            #[positional_only] _self: PyValueRef<'_>,
            _fullname: PyValue,
            _path: PyValue,
            #[default(OptionalArg::Omitted)] _target: OptValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn invalidate_caches(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }

    class PathEntryFinder {
        fn find_spec(
            #[positional_only] _self: PyValueRef<'_>,
            _fullname: PyValue,
            #[default(OptionalArg::Omitted)] _target: OptValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn invalidate_caches(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::none())
        }
    }

    class Loader {
        fn create_module(
            #[positional_only] _self: PyValueRef<'_>,
            _spec: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn load_module(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            if _interp.get_attr(self_val.0, "exec_module").is_err() {
                return Err(PyError::import_error(
                    "ImportError",
                    "Loader.load_module() is not implemented".to_string(),
                    fullname.0.as_str().map(str::to_string),
                ));
            }
            let util = _interp.load_module("importlib.util")?;
            let spec_from_loader = _interp.get_attr(&util, "spec_from_loader")?;
            let spec = call_pos(_interp, spec_from_loader, &[fullname.0.clone(), self_val.0.clone()])?;
            let mod_from_spec = _interp.get_attr(&util, "module_from_spec")?;
            let module = call_pos(_interp, mod_from_spec, &[spec])?;
            let sys_mod = _interp.load_module("sys")?;
            let modules = _interp.get_attr(&sys_mod, "modules")?;
            let setitem = _interp.get_attr(&modules, "__setitem__")?;
            call_pos(_interp, setitem, &[fullname.0, module.clone()])?;
            let exec_fn = _interp.get_attr(self_val.0, "exec_module")?;
            call_pos(_interp, exec_fn, std::slice::from_ref(&module))?;
            Ok(module)
        }

        fn module_repr(
            #[positional_only] _self: PyValueRef<'_>,
            _module: PyValue,
        ) -> Result<Value> {
            Err(PyError::named(
                "NotImplementedError",
                "module_repr is not implemented",
            ))
        }
    }

    class ResourceLoader("Loader") {
        fn get_data(
            #[positional_only] _self: PyValueRef<'_>,
            _path: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("OSError", "get_data is abstract"))
        }
    }

    class InspectLoader("Loader") {
        fn is_package(
            #[positional_only] _self: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            Err(PyError::import_error(
                "ImportError",
                "is_package is not implemented".to_string(),
                fullname.0.as_str().map(str::to_string),
            ))
        }

        fn get_source(
            #[positional_only] _self: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            Err(PyError::import_error(
                "ImportError",
                "get_source is abstract".to_string(),
                fullname.0.as_str().map(str::to_string),
            ))
        }

        fn get_code(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            let get_src = _interp.get_attr(self_val.0, "get_source")?;
            let source = call_pos(_interp, get_src, &[fullname.0])?;
            if source.is_none() {
                return Ok(Value::none());
            }
            let stc = _interp.get_attr(self_val.0, "source_to_code")?;
            call_pos(_interp, stc, &[source])
        }

        fn exec_module(
            #[positional_only] self_val: PyValueRef<'_>,
            module: PyValue,
        ) -> Result<Value> {
            exec_module_via_get_code(_interp, self_val.0, &module.0)
        }

        #[staticmethod]
        fn source_to_code(
            data: PyValue,
            #[default(OptionalArg::Omitted)] path: OptValueRef<'_>,
        ) -> Result<Value> {
            let path_val = path
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::string("<string>"));
            compile_source_to_code(_interp, &data.0, &path_val)
        }
    }

    class ExecutionLoader("InspectLoader") {
        fn get_filename(
            #[positional_only] _self: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            Err(PyError::import_error(
                "ImportError",
                "get_filename is abstract".to_string(),
                fullname.0.as_str().map(str::to_string),
            ))
        }

        fn get_code(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            let get_src = _interp.get_attr(self_val.0, "get_source")?;
            let source = call_pos(_interp, get_src, std::slice::from_ref(&fullname.0))?;
            if source.is_none() {
                return Ok(Value::none());
            }
            let get_fn = _interp.get_attr(self_val.0, "get_filename")?;
            let path = match call_pos(_interp, get_fn, &[fullname.0]) {
                Ok(p) => p,
                Err(e) if e.class_name_is("ImportError") => Value::string("<string>"),
                Err(e) => return Err(e),
            };
            let stc = _interp.get_attr(self_val.0, "source_to_code")?;
            call_pos(_interp, stc, &[source, path])
        }
    }

    class FileLoader("ResourceLoader", "ExecutionLoader") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyStr<'_>,
            path: PyPath,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "name", Value::string(fullname.0))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "path",
                Value::string(path.as_path().to_string_lossy()),
            )?;
            Ok(Value::none())
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let self_cls = _interp.get_attr(self_val.0, "__class__")?;
            let Ok(other_cls) = _interp.get_attr(&other.0, "__class__") else {
                return Ok(Value::bool_(false));
            };
            if self_cls.object_id() != other_cls.object_id() {
                return Ok(Value::bool_(false));
            }
            let self_dict = _interp.get_attr(self_val.0, "__dict__")?;
            let other_dict = _interp.get_attr(&other.0, "__dict__")?;
            let eq = py_eq(_interp, &self_dict, &other_dict)?;
            Ok(Value::bool_(eq))
        }

        fn __hash__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let name = _interp.get_attr(self_val.0, "name")?;
            let path = _interp.get_attr(self_val.0, "path")?;
            let h1 = call_pos(_interp, Value::builtin_function("hash"), &[name])?
                .as_int()
                .unwrap_or(0);
            let h2 = call_pos(_interp, Value::builtin_function("hash"), &[path])?
                .as_int()
                .unwrap_or(0);
            Ok(Value::int(h1 ^ h2))
        }

        fn get_filename(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            file_loader_check_name(_interp, self_val.0, &fullname.0)?;
            _interp.get_attr(self_val.0, "path")
        }

        fn get_data(#[positional_only] _self: PyValueRef<'_>, path: PyPath) -> Result<Value> {
            let bytes = std::fs::read(path.as_path())
                .map_err(|e| PyError::named("OSError", format!("{e}")))?;
            Ok(Value::bytes(bytes))
        }

        fn is_package(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            compute_is_package(_interp, self_val.0, &fullname.0)
        }
    }

    class SourceLoader("ResourceLoader", "ExecutionLoader") {
        fn path_mtime(
            #[positional_only] _self: PyValueRef<'_>,
            _path: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("OSError", "path_mtime is not implemented"))
        }

        fn path_stats(
            #[positional_only] self_val: PyValueRef<'_>,
            path: PyValue,
        ) -> Result<Value> {
            let pm = _interp.get_attr(self_val.0, "path_mtime")?;
            let mtime = call_pos(_interp, pm, &[path.0])?;
            let mut d = crate::value::PyDict::default();
            d.insert(PyKey::str_from("mtime"), mtime);
            Ok(Value::dict(d))
        }

        fn set_data(
            #[positional_only] _self: PyValueRef<'_>,
            _path: PyValue,
            _data: PyValue,
        ) -> Result<Value> {
            Ok(Value::none())
        }

        fn get_source(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            let get_fn = _interp.get_attr(self_val.0, "get_filename")?;
            let path = call_pos(_interp, get_fn, std::slice::from_ref(&fullname.0))?;
            let get_data_fn = _interp.get_attr(self_val.0, "get_data")?;
            let source_bytes = match call_pos(_interp, get_data_fn, &[path]) {
                Ok(b) => b,
                Err(e) if e.class_name_is("OSError") => {
                    return Err(PyError::import_error(
                        "ImportError",
                        format!("source not available through get_data(): {e}"),
                        fullname.0.as_str().map(str::to_string),
                    ));
                }
                Err(e) => return Err(e),
            };
            let util = _interp.load_module("importlib.util")?;
            let decode_fn = _interp.get_attr(&util, "decode_source")?;
            call_pos(_interp, decode_fn, &[source_bytes])
        }

        fn get_code(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            let get_src = _interp.get_attr(self_val.0, "get_source")?;
            let source = call_pos(_interp, get_src, std::slice::from_ref(&fullname.0))?;
            if source.is_none() {
                return Ok(Value::none());
            }
            let get_fn = _interp.get_attr(self_val.0, "get_filename")?;
            let path = match call_pos(_interp, get_fn, &[fullname.0]) {
                Ok(p) => p,
                Err(e) if e.class_name_is("ImportError") => Value::string("<string>"),
                Err(e) => return Err(e),
            };
            let stc = _interp.get_attr(self_val.0, "source_to_code")?;
            call_pos(_interp, stc, &[source, path])
        }

        fn is_package(
            #[positional_only] self_val: PyValueRef<'_>,
            fullname: PyValue,
        ) -> Result<Value> {
            compute_is_package(_interp, self_val.0, &fullname.0)
        }

        fn exec_module(
            #[positional_only] self_val: PyValueRef<'_>,
            module: PyValue,
        ) -> Result<Value> {
            exec_module_via_get_code(_interp, self_val.0, &module.0)
        }
    }
}
