// `importlib.resources.readers` module — 100% native-Rust Python 3.12
// implementation (`FileReader`, `ZipReader`, `MultiplexedPath`,
// `NamespaceReader`, plus native zip-archive `Traversable`).
//
// Reference: <https://docs.python.org/3/library/importlib.resources.html>

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr};
use crate::value::{PyModule, PySet, Value, ValueKind};

struct ZipTraversableState {
    archive_path: String,
    at: String,
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn make_pathlib_path(interp: &mut Interpreter, path_val: Value) -> Result<Value> {
    let pathlib_mod = interp.load_module("pathlib")?;
    let path_cls = interp.get_attr(&pathlib_mod, "Path")?;
    call_pos(interp, path_cls, &[path_val])
}

fn as_traversable_path(interp: &mut Interpreter, val: &Value) -> Result<Value> {
    if val.as_str().is_some() {
        return make_pathlib_path(interp, val.clone());
    }
    if interp.get_attr(val, "is_dir").is_ok() && interp.get_attr(val, "iterdir").is_ok() {
        return Ok(val.clone());
    }
    make_pathlib_path(interp, val.clone())
}

fn zip_namelist(interp: &mut Interpreter, archive_path: &str) -> Result<Vec<String>> {
    let zipfile_mod = interp.load_module("zipfile")?;
    let zip_cls = interp.get_attr(&zipfile_mod, "ZipFile")?;
    let zf = call_pos(
        interp,
        zip_cls,
        &[Value::string(archive_path), Value::string("r")],
    )?;
    let res = (|| -> Result<Vec<String>> {
        let namelist_fn = interp.get_attr(&zf, "namelist")?;
        let nl = call_pos(interp, namelist_fn, &[])?;
        let items = interp.collect_iterable(&nl)?;
        Ok(items
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect())
    })();
    if let Ok(close_fn) = interp.get_attr(&zf, "close") {
        let _ = call_pos(interp, close_fn, &[]);
    }
    res
}

fn zip_read_bytes(interp: &mut Interpreter, archive_path: &str, member: &str) -> Result<Value> {
    let zipfile_mod = interp.load_module("zipfile")?;
    let zip_cls = interp.get_attr(&zipfile_mod, "ZipFile")?;
    let zf = call_pos(
        interp,
        zip_cls,
        &[Value::string(archive_path), Value::string("r")],
    )?;
    let res = (|| -> Result<Value> {
        let read_fn = interp.get_attr(&zf, "read")?;
        call_pos(interp, read_fn, &[Value::string(member)])
    })();
    if let Ok(close_fn) = interp.get_attr(&zf, "close") {
        let _ = call_pos(interp, close_fn, &[]);
    }
    res
}

fn readers_cls(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("importlib.resources.readers")?;
    interp.get_attr(&m, name)
}

fn attach_base(cls_val: &Value, base_val: &Value) {
    let (ValueKind::PyClass(cls_rc), ValueKind::PyClass(base_rc)) =
        (cls_val.kind(), base_val.kind())
    else {
        return;
    };
    {
        let mut cls = cls_rc.borrow_mut();
        if cls.base.is_none() {
            cls.base = Some(Rc::clone(base_rc));
            base_rc
                .borrow()
                .subclasses
                .borrow_mut()
                .push(Rc::downgrade(cls_rc));
        }
        if cls.metatype.is_none() {
            cls.metatype = base_rc.borrow().metatype.clone();
        }
        cls.attrs.insert(
            "__abstractmethods__".to_string(),
            pyrust_builtins::frozenset::frozenset(PySet::default()),
        );
        cls.attrs
            .insert("_abc_registry".to_string(), Value::set(PySet::default()));
    }
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let res_abc = interp.load_module("importlib.resources.abc")?;
    let trav_cls = interp.get_attr(&res_abc, "Traversable")?;
    let tr_res_cls = interp.get_attr(&res_abc, "TraversableResources")?;

    for name in ["FileReader", "ZipReader", "NamespaceReader"] {
        let cls_opt = module.borrow().attrs.get(name).cloned();
        if let Some(cls_val) = cls_opt {
            attach_base(&cls_val, &tr_res_cls);
        }
    }
    for name in ["MultiplexedPath", "_ZipTraversable"] {
        let cls_opt = module.borrow().attrs.get(name).cloned();
        if let Some(cls_val) = cls_opt {
            attach_base(&cls_val, &trav_cls);
        }
    }
    module.borrow_mut().insert_attr("abc".to_string(), res_abc);
    Ok(())
}

fn multiplexed_join_single(
    interp: &mut Interpreter,
    paths: &[Value],
    segment: &str,
) -> Result<Value> {
    let mut matching_dirs: Vec<Value> = Vec::new();
    for p in paths {
        let jp = interp.get_attr(p, "joinpath")?;
        let candidate = call_pos(interp, jp, &[Value::string(segment)])?;
        let is_file = interp
            .get_attr(&candidate, "is_file")
            .and_then(|f| call_pos(interp, f, &[]))
            .and_then(|v| interp.truthy_value(&v))
            .unwrap_or(false);
        if is_file {
            return Ok(candidate);
        }
        let is_dir = interp
            .get_attr(&candidate, "is_dir")
            .and_then(|f| call_pos(interp, f, &[]))
            .and_then(|v| interp.truthy_value(&v))
            .unwrap_or(false);
        if is_dir {
            matching_dirs.push(candidate);
        }
    }
    if matching_dirs.len() == 1 {
        return Ok(matching_dirs.into_iter().next().unwrap());
    }
    if matching_dirs.len() > 1 {
        let mp_cls = readers_cls(interp, "MultiplexedPath")?;
        return call_pos(interp, mp_cls, &matching_dirs);
    }
    // Non-existent child: return `paths[0].joinpath(segment)` so `.is_file()` /
    // `.is_dir()` return False and `.open()` raises `FileNotFoundError`.
    let jp = interp.get_attr(&paths[0], "joinpath")?;
    call_pos(interp, jp, &[Value::string(segment)])
}

pyrust_module! {
    #[native_state(ZipTraversableState)]
    class _ZipTraversable {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            archive_path: PyStr<'_>,
            #[default(OptionalArg::Omitted)] at: OptValueRef<'_>,
        ) -> Result<Value> {
            let at_str = at
                .as_value()
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim_start_matches('/')
                .to_string();
            set_state(
                self_val.0,
                ZipTraversableState {
                    archive_path: archive_path.0.to_string(),
                    at: at_str,
                },
            )?;
            Ok(Value::none())
        }

        #[getter]
        fn name(#[state] st: &ZipTraversableState) -> Result<Value> {
            let trimmed = st.at.trim_end_matches('/');
            let base = trimmed.rsplit_once('/').map(|(_, b)| b).unwrap_or(trimmed);
            Ok(Value::string(base))
        }

        fn is_dir(#[state] st: &ZipTraversableState) -> Result<Value> {
            if st.at.is_empty() || st.at.ends_with('/') {
                return Ok(Value::bool_(true));
            }
            let prefix = format!("{}/", st.at);
            let names = zip_namelist(_interp, &st.archive_path)?;
            let found = names.iter().any(|n| n == &prefix || n.starts_with(&prefix));
            Ok(Value::bool_(found))
        }

        fn is_file(#[state] st: &ZipTraversableState) -> Result<Value> {
            if st.at.is_empty() || st.at.ends_with('/') {
                return Ok(Value::bool_(false));
            }
            let names = zip_namelist(_interp, &st.archive_path)?;
            Ok(Value::bool_(names.iter().any(|n| n == &st.at)))
        }

        fn iterdir(#[state] st: &ZipTraversableState) -> Result<Value> {
            let prefix = if st.at.is_empty() {
                String::new()
            } else if st.at.ends_with('/') {
                st.at.clone()
            } else {
                format!("{}/", st.at)
            };
            let names = zip_namelist(_interp, &st.archive_path)?;
            let cls = readers_cls(_interp, "_ZipTraversable")?;
            let mut seen: HashSet<String> = HashSet::new();
            let mut out = Vec::new();
            for n in names {
                let Some(rest) = n.strip_prefix(&prefix) else {
                    continue;
                };
                if rest.is_empty() {
                    continue;
                }
                let child = match rest.find('/') {
                    Some(idx) => &rest[..=idx],
                    None => rest,
                };
                let child_key = child.trim_end_matches('/').to_string();
                if seen.insert(child_key.clone()) {
                    let child_at = format!("{prefix}{child}");
                    out.push(call_pos(
                        _interp,
                        cls.clone(),
                        &[Value::string(&st.archive_path), Value::string(&child_at)],
                    )?);
                }
            }
            Ok(Value::list(out))
        }

        fn joinpath(
            #[state] st: &ZipTraversableState,
            #[varargs] descendants: &[Value],
        ) -> Result<Value> {
            let mut cur = st.at.trim_end_matches('/').to_string();
            for desc in descendants {
                let s = desc
                    .as_str()
                    .ok_or_else(|| PyError::named("TypeError", "joinpath segments must be str"))?;
                for part in s.split('/').filter(|p| !p.is_empty()) {
                    if cur.is_empty() {
                        cur = part.to_string();
                    } else {
                        cur = format!("{cur}/{part}");
                    }
                }
            }
            let cls = readers_cls(_interp, "_ZipTraversable")?;
            call_pos(_interp, cls, &[Value::string(&st.archive_path), Value::string(&cur)])
        }

        fn __truediv__(#[positional_only] self_val: PyValueRef<'_>, child: PyValue) -> Result<Value> {
            let jp = _interp.get_attr(self_val.0, "joinpath")?;
            call_pos(_interp, jp, &[child.0])
        }

        fn read_bytes(#[state] st: &ZipTraversableState) -> Result<Value> {
            zip_read_bytes(_interp, &st.archive_path, st.at.trim_end_matches('/'))
        }

        fn read_text(
            #[state] st: &ZipTraversableState,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] errors: OptValueRef<'_>,
        ) -> Result<Value> {
            let raw = zip_read_bytes(_interp, &st.archive_path, st.at.trim_end_matches('/'))?;
            let decode_fn = _interp.get_attr(&raw, "decode")?;
            let enc = encoding
                .as_value()
                .filter(|v| !v.is_none())
                .cloned()
                .unwrap_or_else(|| Value::string("utf-8"));
            let errs = errors
                .as_value()
                .filter(|v| !v.is_none())
                .cloned()
                .unwrap_or_else(|| Value::string("strict"));
            call_pos(_interp, decode_fn, &[enc, errs])
        }

        fn open(
            #[state] st: &ZipTraversableState,
            #[default(OptionalArg::Omitted)] mode: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] errors: OptValueRef<'_>,
        ) -> Result<Value> {
            let mode_str = mode
                .as_value()
                .and_then(|v| v.as_str())
                .unwrap_or("r");
            let raw = zip_read_bytes(_interp, &st.archive_path, st.at.trim_end_matches('/'))?;
            let io_mod = _interp.load_module("io")?;
            if mode_str.contains('b') {
                let bio = _interp.get_attr(&io_mod, "BytesIO")?;
                call_pos(_interp, bio, &[raw])
            } else {
                let decode_fn = _interp.get_attr(&raw, "decode")?;
                let enc = encoding
                    .as_value()
                    .filter(|v| !v.is_none())
                    .cloned()
                    .unwrap_or_else(|| Value::string("utf-8"));
                let errs = errors
                    .as_value()
                    .filter(|v| !v.is_none())
                    .cloned()
                    .unwrap_or_else(|| Value::string("strict"));
                let text = call_pos(_interp, decode_fn, &[enc, errs])?;
                let sio = _interp.get_attr(&io_mod, "StringIO")?;
                call_pos(_interp, sio, &[text])
            }
        }

        fn __repr__(#[state] st: &ZipTraversableState) -> Result<Value> {
            Ok(Value::string(format!(
                "Path({:?}, {:?})",
                st.archive_path, st.at
            )))
        }
    }

    class FileReader {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            loader: PyValue,
        ) -> Result<Value> {
            let loader_path = _interp.get_attr(&loader.0, "path")?;
            let path_obj = make_pathlib_path(_interp, loader_path)?;
            let parent = _interp.get_attr(&path_obj, "parent")?;
            _interp.assign_attr(self_val.0.clone(), "path", parent)?;
            Ok(Value::none())
        }

        fn resource_path(
            #[positional_only] self_val: PyValueRef<'_>,
            resource: PyValue,
        ) -> Result<Value> {
            let p = _interp.get_attr(self_val.0, "path")?;
            let jp = _interp.get_attr(&p, "joinpath")?;
            let full = call_pos(_interp, jp, &[resource.0])?;
            call_pos(_interp, Value::builtin_function("str"), &[full])
        }

        fn files(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "path")
        }
    }

    class ZipReader {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            loader: PyValue,
            module: PyStr<'_>,
        ) -> Result<Value> {
            let archive = _interp.get_attr(&loader.0, "archive")?;
            let loader_prefix = _interp
                .get_attr(&loader.0, "prefix")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
                .replace('\\', "/");
            let tail = module
                .0
                .rsplit_once('.')
                .map(|(_, t)| t)
                .unwrap_or(&module.0);
            let prefix = format!("{loader_prefix}{tail}/");
            _interp.assign_attr(self_val.0.clone(), "archive", archive)?;
            _interp.assign_attr(self_val.0.clone(), "prefix", Value::string(prefix))?;
            Ok(Value::none())
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
            match call_pos(_interp, open_fn, &[Value::string("rb")]) {
                Ok(v) => Ok(v),
                Err(e) if e.class_name_is("KeyError") => {
                    Err(PyError::named("FileNotFoundError", format!("{e}")))
                }
                Err(e) => Err(e),
            }
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

        fn files(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let archive = _interp.get_attr(self_val.0, "archive")?;
            let prefix = _interp.get_attr(self_val.0, "prefix")?;
            let cls = readers_cls(_interp, "_ZipTraversable")?;
            call_pos(_interp, cls, &[archive, prefix])
        }
    }

    class MultiplexedPath {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] paths: &[Value],
        ) -> Result<Value> {
            let mut resolved = Vec::with_capacity(paths.len());
            for p in paths {
                let trav = as_traversable_path(_interp, p)?;
                let is_file = _interp
                    .get_attr(&trav, "is_file")
                    .and_then(|f| call_pos(_interp, f, &[]))
                    .and_then(|v| _interp.truthy_value(&v))
                    .unwrap_or(false);
                if is_file {
                    return Err(PyError::named(
                        "NotADirectoryError",
                        "MultiplexedPath only supports directories",
                    ));
                }
                resolved.push(trav);
            }
            if resolved.is_empty() {
                return Err(PyError::named(
                    "FileNotFoundError",
                    "MultiplexedPath must contain at least one path",
                ));
            }
            _interp.assign_attr(self_val.0.clone(), "_paths", Value::list(resolved))?;
            Ok(Value::none())
        }

        #[getter]
        fn name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let paths = _interp.get_attr(self_val.0, "_paths")?;
            let items = _interp.collect_iterable(&paths)?;
            let first = items
                .first()
                .ok_or_else(|| PyError::named("FileNotFoundError", "empty MultiplexedPath"))?;
            _interp.get_attr(first, "name")
        }

        fn is_dir(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(true))
        }

        fn is_file(#[positional_only] _self: PyValueRef<'_>) -> Result<Value> {
            Ok(Value::bool_(false))
        }

        fn iterdir(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let paths_val = _interp.get_attr(self_val.0, "_paths")?;
            let paths = _interp.collect_iterable(&paths_val)?;
            let mut seen_files: HashSet<String> = HashSet::new();
            let mut dir_order: Vec<String> = Vec::new();
            let mut dir_buckets: IndexMap<String, Vec<Value>> = IndexMap::new();
            let mut out: Vec<Value> = Vec::new();

            for p in paths {
                let is_dir = _interp
                    .get_attr(&p, "is_dir")
                    .and_then(|f| call_pos(_interp, f, &[]))
                    .and_then(|v| _interp.truthy_value(&v))
                    .unwrap_or(false);
                if !is_dir {
                    continue;
                }
                let iterdir_fn = _interp.get_attr(&p, "iterdir")?;
                let entries_iter = call_pos(_interp, iterdir_fn, &[])?;
                let entries = _interp.collect_iterable(&entries_iter)?;
                for entry in entries {
                    let name_val = _interp.get_attr(&entry, "name")?;
                    let name_str = name_val.as_str().unwrap_or_default().to_string();
                    let entry_is_dir = _interp
                        .get_attr(&entry, "is_dir")
                        .and_then(|f| call_pos(_interp, f, &[]))
                        .and_then(|v| _interp.truthy_value(&v))
                        .unwrap_or(false);
                    if entry_is_dir {
                        if !dir_buckets.contains_key(&name_str) {
                            dir_order.push(name_str.clone());
                        }
                        dir_buckets.entry(name_str).or_default().push(entry);
                    } else if seen_files.insert(name_str) {
                        out.push(entry);
                    }
                }
            }

            let mp_cls = readers_cls(_interp, "MultiplexedPath")?;
            for dir_name in dir_order {
                if let Some(group) = dir_buckets.shift_remove(&dir_name) {
                    if group.len() == 1 {
                        out.push(group.into_iter().next().unwrap());
                    } else {
                        out.push(call_pos(_interp, mp_cls.clone(), &group)?);
                    }
                }
            }

            Ok(Value::list(out))
        }

        fn joinpath(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] descendants: &[Value],
        ) -> Result<Value> {
            if descendants.is_empty() {
                return Ok(self_val.0.clone());
            }
            let mut current = self_val.0.clone();
            for desc in descendants {
                let desc_str = desc
                    .as_str()
                    .ok_or_else(|| PyError::named("TypeError", "joinpath segments must be str"))?;
                for part in desc_str.split('/').filter(|s| !s.is_empty()) {
                    if let Ok(paths_val) = _interp.get_attr(&current, "_paths") {
                        let paths = _interp.collect_iterable(&paths_val)?;
                        current = multiplexed_join_single(_interp, &paths, part)?;
                    } else {
                        let jp = _interp.get_attr(&current, "joinpath")?;
                        current = call_pos(_interp, jp, &[Value::string(part)])?;
                    }
                }
            }
            Ok(current)
        }

        fn __truediv__(#[positional_only] self_val: PyValueRef<'_>, child: PyValue) -> Result<Value> {
            let jp = _interp.get_attr(self_val.0, "joinpath")?;
            call_pos(_interp, jp, &[child.0])
        }

        fn open(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
            #[varkwargs] _kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let r = render_value_repr(_interp, self_val.0)?;
            Err(PyError::named("FileNotFoundError", format!("{r} is not a file")))
        }

        fn read_bytes(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let r = render_value_repr(_interp, self_val.0)?;
            Err(PyError::named("FileNotFoundError", format!("{r} is not a file")))
        }

        fn read_text(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] _args: &[Value],
            #[varkwargs] _kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let r = render_value_repr(_interp, self_val.0)?;
            Err(PyError::named("FileNotFoundError", format!("{r} is not a file")))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let paths_val = _interp.get_attr(self_val.0, "_paths")?;
            let paths = _interp.collect_iterable(&paths_val)?;
            let mut parts = Vec::with_capacity(paths.len());
            for p in paths {
                let s = call_pos(_interp, Value::builtin_function("str"), &[p])?;
                let r = render_value_repr(_interp, &s)?;
                parts.push(r);
            }
            Ok(Value::string(format!("MultiplexedPath({})", parts.join(", "))))
        }
    }

    class NamespaceReader {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            namespace_path: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&namespace_path.0)?;
            let mp_cls = readers_cls(_interp, "MultiplexedPath")?;
            let mp = call_pos(_interp, mp_cls, &items)?;
            _interp.assign_attr(self_val.0.clone(), "path", mp)?;
            Ok(Value::none())
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

        fn files(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "path")
        }
    }
}
