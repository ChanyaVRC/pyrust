// `importlib.resources` package module — 100% native-Rust Python 3.12
// implementation using PyRust's expanded macro toolkit (`pyrust_module!`,
// `class`, `#[native_state]`, `PyPath`).
//
// Exposes `files`, `as_file`, `contents`, `is_resource`, `open_binary`,
// `open_text`, `read_binary`, `read_text`, `path`, `ResourceReader`,
// `Package`, `Resource`, and submodules `abc`, `readers`, and `simple`.
//
// Reference: <https://docs.python.org/3/library/importlib.resources.html>

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBytesLike, PyPath, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value, ValueKind};

static TEMP_FILE_SEQ: AtomicU64 = AtomicU64::new(0);

struct AsFileState {
    traversable: Value,
    temp_path: Option<PathBuf>,
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

fn make_pathlib_path(interp: &mut Interpreter, p: &str) -> Result<Value> {
    let pathlib_mod = interp.load_module("pathlib")?;
    let path_cls = interp.get_attr(&pathlib_mod, "Path")?;
    call_pos(interp, path_cls, &[Value::string(p)])
}

fn resolve_package_module(interp: &mut Interpreter, anchor: Option<&Value>) -> Result<Value> {
    match anchor {
        None => {
            // Default anchor: caller's `__package__` or `__name__` via `sys._getframe(1)`.
            let sys_mod = interp.load_module("sys")?;
            let getframe = interp.get_attr(&sys_mod, "_getframe")?;
            let frame = call_pos(interp, getframe, &[Value::int(1)])?;
            let globals = interp.get_attr(&frame, "f_globals")?;
            let get_fn = interp.get_attr(&globals, "get")?;
            let pkg = call_pos(interp, get_fn.clone(), &[Value::string("__package__")])?;
            let mod_name = if let Some(s) = pkg.as_str()
                && !s.is_empty()
            {
                s.to_string()
            } else {
                let n = call_pos(interp, get_fn, &[Value::string("__name__")])?;
                n.as_str().unwrap_or("__main__").to_string()
            };
            interp.load_module(&mod_name)
        }
        Some(v) if v.is_none() => resolve_package_module(interp, None),
        Some(v) => {
            if let Some(name) = v.as_str() {
                interp.load_module(name)
            } else if matches!(v.kind(), ValueKind::PyModule(_)) {
                Ok(v.clone())
            } else {
                Err(PyError::named(
                    "TypeError",
                    "Expected a module object or module name string",
                ))
            }
        }
    }
}

fn resolve_files_for_module(interp: &mut Interpreter, module: &Value) -> Result<Value> {
    let fullname = interp
        .get_attr(module, "__name__")
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();

    // 1. Consult loader's `get_resource_reader(fullname)` if present.
    let spec = interp
        .get_attr(module, "__spec__")
        .ok()
        .filter(|v| !v.is_none());
    let loader = spec
        .as_ref()
        .and_then(|s| interp.get_attr(s, "loader").ok())
        .filter(|v| !v.is_none())
        .or_else(|| {
            interp
                .get_attr(module, "__loader__")
                .ok()
                .filter(|v| !v.is_none())
        });

    if let Some(ref ldr) = loader
        && let Ok(get_reader) = interp.get_attr(ldr, "get_resource_reader")
    {
        let reader = call_pos(interp, get_reader, &[Value::string(&fullname)])?;
        if !reader.is_none()
            && let Ok(files_fn) = interp.get_attr(&reader, "files")
        {
            return call_pos(interp, files_fn, &[]);
        }
    }

    // 2. Check package `__path__` (or `spec.submodule_search_locations`).
    let path_attr = interp
        .get_attr(module, "__path__")
        .ok()
        .filter(|v| !v.is_none())
        .or_else(|| {
            spec.as_ref()
                .and_then(|s| interp.get_attr(s, "submodule_search_locations").ok())
                .filter(|v| !v.is_none())
        });

    if let Some(path_val) = path_attr {
        let entries = interp.collect_iterable(&path_val)?;
        let file_attr = interp
            .get_attr(module, "__file__")
            .ok()
            .filter(|v| !v.is_none());
        if entries.len() == 1 && file_attr.is_some() {
            let p_str = entries[0].as_str().map(str::to_string).unwrap_or_default();
            return make_pathlib_path(interp, &p_str);
        }
        if !entries.is_empty() {
            let readers_mod = interp.load_module("importlib.resources.readers")?;
            let mp_cls = interp.get_attr(&readers_mod, "MultiplexedPath")?;
            return call_pos(interp, mp_cls, &entries);
        }
    }

    // 3. Fallback for regular non-package modules with `__file__`.
    if let Ok(file_val) = interp.get_attr(module, "__file__")
        && let Some(file_str) = file_val.as_str()
    {
        let parent = Path::new(file_str)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_string_lossy()
            .into_owned();
        return make_pathlib_path(interp, &parent);
    }

    Err(PyError::named(
        "TypeError",
        format!("{fullname:?} is not a package"),
    ))
}

fn validate_resource_name(resource: &PyPath) -> Result<String> {
    let s = resource.as_path().to_string_lossy().into_owned();
    if s.contains('/') || s.contains('\\') {
        return Err(PyError::named(
            "ValueError",
            format!("{s:?} must be only a file name"),
        ));
    }
    Ok(s)
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

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let res_abc = interp.load_module("importlib.resources.abc")?;
    if let Ok(rr) = interp.get_attr(&res_abc, "ResourceReader") {
        module
            .borrow_mut()
            .insert_attr("ResourceReader".to_string(), rr);
    }
    // Typing aliases matching CPython 3.12 (`Package`, `Resource`).
    if let Ok(typing) = interp.load_module("typing")
        && let Ok(any_val) = interp.get_attr(&typing, "Any")
    {
        module
            .borrow_mut()
            .insert_attr("Package".to_string(), any_val.clone());
        module
            .borrow_mut()
            .insert_attr("Resource".to_string(), any_val);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("Package"),
            Value::string("Resource"),
            Value::string("ResourceReader"),
            Value::string("as_file"),
            Value::string("contents"),
            Value::string("files"),
            Value::string("is_resource"),
            Value::string("open_binary"),
            Value::string("open_text"),
            Value::string("path"),
            Value::string("read_binary"),
            Value::string("read_text"),
        ]),
        "abc" => super::importlib_resources_abc::module(),
        "readers" => super::importlib_resources_readers::module(),
        "simple" => super::importlib_resources_simple::module(),
    }

    #[native_state(AsFileState)]
    class _AsFileContext {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            traversable: PyValue,
        ) -> Result<Value> {
            set_state(
                self_val.0,
                AsFileState {
                    traversable: traversable.0,
                    temp_path: None,
                },
            )?;
            Ok(Value::none())
        }

        fn __enter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let trav = with_state(self_val.0, |st| Ok(st.traversable.clone()))?;
            // Fast path: if `traversable` already implements `__fspath__`
            // (e.g., `pathlib.Path`), yield a `pathlib.Path` directly without
            // copying to a temp file.
            if let Ok(fspath_fn) = _interp.get_attr(&trav, "__fspath__")
                && let Ok(p_val) = call_pos(_interp, fspath_fn, &[])
                && let Some(p_str) = p_val.as_str()
            {
                return make_pathlib_path(_interp, p_str);
            }
            // Virtual / zip Traversable: materialize bytes into a temporary
            // file and delete it on `__exit__`.
            let rb_fn = _interp.get_attr(&trav, "read_bytes")?;
            let bytes_val = call_pos(_interp, rb_fn, &[])?;
            let raw_bytes: Vec<u8> = PyBytesLike::try_from_value_with_interp(
                _interp,
                &bytes_val,
                "as_file",
                "read_bytes",
            )?
            .into_vec();
            let suffix = _interp
                .get_attr(&trav, "name")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .and_then(|n| {
                    Path::new(&n)
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| format!(".{e}"))
                })
                .unwrap_or_default();
            let seq = TEMP_FILE_SEQ.fetch_add(1, Ordering::Relaxed);
            let tmp_name = format!(
                "pyrust_resource_{}_{}{}",
                std::process::id(),
                seq,
                suffix
            );
            let tmp_path = std::env::temp_dir().join(tmp_name);
            std::fs::write(&tmp_path, &raw_bytes)
                .map_err(|e| PyError::named("OSError", format!("{e}")))?;
            let tmp_str = tmp_path.to_string_lossy().into_owned();
            with_state_mut(self_val.0, |st| {
                st.temp_path = Some(tmp_path);
                Ok(())
            })?;
            make_pathlib_path(_interp, &tmp_str)
        }

        fn __exit__(
            #[positional_only] self_val: PyValueRef<'_>,
            _exc_type: PyValue,
            _exc_val: PyValue,
            _exc_tb: PyValue,
        ) -> Result<Value> {
            let to_remove = with_state_mut(self_val.0, |st| Ok(st.temp_path.take()))?;
            if let Some(p) = to_remove {
                let _ = std::fs::remove_file(p);
            }
            Ok(Value::bool_(false))
        }
    }

    fn files(
        #[default(OptionalArg::Omitted)] anchor: OptValueRef<'_>,
    ) -> Result<Value> {
        let module = resolve_package_module(_interp, anchor.as_value())?;
        resolve_files_for_module(_interp, &module)
    }

    fn as_file(path: PyValue) -> Result<Value> {
        let res_mod = _interp.load_module("importlib.resources")?;
        let ctx_cls = _interp.get_attr(&res_mod, "_AsFileContext")?;
        call_pos(_interp, ctx_cls, &[path.0])
    }

    fn contents(package: PyValue) -> Result<Value> {
        let module = resolve_package_module(_interp, Some(&package.0))?;
        let root = resolve_files_for_module(_interp, &module)?;
        let iterdir_fn = _interp.get_attr(&root, "iterdir")?;
        let entries = call_pos(_interp, iterdir_fn, &[])?;
        let items = _interp.collect_iterable(&entries)?;
        let mut names = Vec::with_capacity(items.len());
        for item in items {
            names.push(_interp.get_attr(&item, "name")?);
        }
        Ok(Value::list(names))
    }

    fn is_resource(package: PyValue, name: PyPath) -> Result<Value> {
        let res_name = validate_resource_name(&name)?;
        let module = resolve_package_module(_interp, Some(&package.0))?;
        let root = resolve_files_for_module(_interp, &module)?;
        let jp = _interp.get_attr(&root, "joinpath")?;
        let target = call_pos(_interp, jp, &[Value::string(&res_name)])?;
        let is_file_fn = _interp.get_attr(&target, "is_file")?;
        call_pos(_interp, is_file_fn, &[])
    }

    fn open_binary(package: PyValue, resource: PyPath) -> Result<Value> {
        let res_name = validate_resource_name(&resource)?;
        let module = resolve_package_module(_interp, Some(&package.0))?;
        let root = resolve_files_for_module(_interp, &module)?;
        let jp = _interp.get_attr(&root, "joinpath")?;
        let target = call_pos(_interp, jp, &[Value::string(&res_name)])?;
        let open_fn = _interp.get_attr(&target, "open")?;
        call_pos(_interp, open_fn, &[Value::string("rb")])
    }

    fn open_text(
        package: PyValue,
        resource: PyPath,
        #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] errors: OptValueRef<'_>,
    ) -> Result<Value> {
        let res_name = validate_resource_name(&resource)?;
        let module = resolve_package_module(_interp, Some(&package.0))?;
        let root = resolve_files_for_module(_interp, &module)?;
        let jp = _interp.get_attr(&root, "joinpath")?;
        let target = call_pos(_interp, jp, &[Value::string(&res_name)])?;
        let open_fn = _interp.get_attr(&target, "open")?;
        let enc = encoding
            .as_value()
            .cloned()
            .unwrap_or_else(|| Value::string("utf-8"));
        let errs = errors
            .as_value()
            .cloned()
            .unwrap_or_else(|| Value::string("strict"));
        let mut kw = IndexMap::new();
        kw.insert("encoding".to_string(), enc);
        kw.insert("errors".to_string(), errs);
        call_with_args_kwargs(_interp, open_fn, &[Value::string("r")], &kw)
    }

    fn read_binary(package: PyValue, resource: PyPath) -> Result<Value> {
        let res_name = validate_resource_name(&resource)?;
        let module = resolve_package_module(_interp, Some(&package.0))?;
        let root = resolve_files_for_module(_interp, &module)?;
        let jp = _interp.get_attr(&root, "joinpath")?;
        let target = call_pos(_interp, jp, &[Value::string(&res_name)])?;
        let open_fn = _interp.get_attr(&target, "open")?;
        let strm = call_pos(_interp, open_fn, &[Value::string("rb")])?;
        let res = (|| -> Result<Value> {
            let read_fn = _interp.get_attr(&strm, "read")?;
            call_pos(_interp, read_fn, &[])
        })();
        close_stream_best_effort(_interp, &strm);
        res
    }

    fn read_text(
        package: PyValue,
        resource: PyPath,
        #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] errors: OptValueRef<'_>,
    ) -> Result<Value> {
        let res_name = validate_resource_name(&resource)?;
        let module = resolve_package_module(_interp, Some(&package.0))?;
        let root = resolve_files_for_module(_interp, &module)?;
        let jp = _interp.get_attr(&root, "joinpath")?;
        let target = call_pos(_interp, jp, &[Value::string(&res_name)])?;
        let open_fn = _interp.get_attr(&target, "open")?;
        let enc = encoding
            .as_value()
            .cloned()
            .unwrap_or_else(|| Value::string("utf-8"));
        let errs = errors
            .as_value()
            .cloned()
            .unwrap_or_else(|| Value::string("strict"));
        let mut kw = IndexMap::new();
        kw.insert("encoding".to_string(), enc);
        kw.insert("errors".to_string(), errs);
        let strm = call_with_args_kwargs(_interp, open_fn, &[Value::string("r")], &kw)?;
        let res = (|| -> Result<Value> {
            let read_fn = _interp.get_attr(&strm, "read")?;
            call_pos(_interp, read_fn, &[])
        })();
        close_stream_best_effort(_interp, &strm);
        res
    }

    fn path(package: PyValue, resource: PyPath) -> Result<Value> {
        let res_name = validate_resource_name(&resource)?;
        let module = resolve_package_module(_interp, Some(&package.0))?;
        let root = resolve_files_for_module(_interp, &module)?;
        let jp = _interp.get_attr(&root, "joinpath")?;
        let target = call_pos(_interp, jp, &[Value::string(&res_name)])?;
        let res_mod = _interp.load_module("importlib.resources")?;
        let ctx_cls = _interp.get_attr(&res_mod, "_AsFileContext")?;
        call_pos(_interp, ctx_cls, &[target])
    }
}
