//! Interpreter-facing `os.fspath` protocol dispatch, `os.PathLike` ABC hooks,
//! and post-load `os` module wiring (`PathLike` -> `abc.ABC`, `supports_*` sets).
//!
//! Path protocol lookup is kept separate from host I/O: it calls Python code
//! and validates the returned object, but never touches the filesystem.

use std::cell::RefCell;
use std::rc::Rc;

use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, reject_keyword_args_expanded, value_type_name_str,
};
use crate::value::{PyKey, PyModule, PySet, Value, ValueKind};

pub(super) fn fspath(
    interpreter: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
) -> Result<Value> {
    reject_keyword_args_expanded(fn_name, args)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() takes exactly 1 argument ({} given)",
                args.len()
            ),
        ));
    }

    let object = args[0].value.clone();
    if matches!(object.kind(), ValueKind::Str(_) | ValueKind::Bytes(_)) {
        return Ok(object);
    }

    if let Ok(method) = interpreter.get_attr(&object, "__fspath__") {
        let result = interpreter.call_function_expanded(method, &[])?;
        if matches!(result.kind(), ValueKind::Str(_) | ValueKind::Bytes(_)) {
            return Ok(result);
        }
        return Err(PyError::named(
            "TypeError",
            format!(
                "expected {}.__fspath__() to return str or bytes, not {}",
                value_type_name_str(&object),
                value_type_name_str(&result),
            ),
        ));
    }

    Err(PyError::named(
        "TypeError",
        format!(
            "expected str, bytes or os.PathLike object, not {}",
            value_type_name_str(&object),
        ),
    ))
}

/// CPython `os.PathLike.__subclasshook__(cls, subclass)`.
pub(super) fn pathlike_subclasshook(cls: &Value, subclass: &Value) -> Result<Value> {
    let ValueKind::PyClass(cls_rc) = cls.kind() else {
        return Ok(Value::not_implemented());
    };
    // Only apply structural check directly for `os.PathLike` itself (matching CPython).
    if cls_rc.borrow().name != "PathLike" {
        return Ok(Value::not_implemented());
    }
    let ValueKind::PyClass(sub_rc) = subclass.kind() else {
        return Ok(Value::not_implemented());
    };
    if let Some(attr) = crate::interpreter::lookup_class_attr(sub_rc, "__fspath__")
        && !attr.is_none()
    {
        return Ok(Value::bool_(true));
    }
    Ok(Value::not_implemented())
}

/// CPython `os.PathLike.__class_getitem__(cls, item)` -> `types.GenericAlias`.
pub(super) fn pathlike_class_getitem(cls: &Value, item: &Value) -> Result<Value> {
    let args = match item.kind() {
        ValueKind::Tuple(_) => item.clone(),
        _ => Value::tuple(vec![item.clone()]),
    };
    Ok(pyrust_builtins::generic_alias::generic_alias(
        cls.clone(),
        args,
    ))
}

/// Wire `os.PathLike` onto `abc.ABC` / `abc.ABCMeta` and populate the
/// `supports_fd` / `supports_follow_symlinks` sets using the module's actual
/// function values.
pub(super) fn prepare_os_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };

    // 1. Wire `os.PathLike` to inherit from `abc.ABC` with metaclass `abc.ABCMeta`.
    if let Ok(abc_mod) = interp.load_module("abc") {
        let abc_meta = interp.get_attr(&abc_mod, "ABCMeta").ok();
        let abc_base = interp.get_attr(&abc_mod, "ABC").ok();
        let pathlike_val = mod_rc.borrow().attrs.get("PathLike").cloned();
        if let (
            Some(ValueKind::PyClass(meta_rc)),
            Some(ValueKind::PyClass(base_rc)),
            Some(ValueKind::PyClass(pl_rc)),
        ) = (
            abc_meta.as_ref().map(Value::kind),
            abc_base.as_ref().map(Value::kind),
            pathlike_val.as_ref().map(Value::kind),
        ) {
            {
                let mut pl = pl_rc.borrow_mut();
                pl.metatype = Some(Rc::clone(meta_rc));
                let mut abs_set = PySet::default();
                abs_set.insert(PyKey::str_from("__fspath__"));
                pl.attrs.insert(
                    "__abstractmethods__".to_string(),
                    pyrust_builtins::frozenset::frozenset(abs_set),
                );
                pl.attrs
                    .insert("_abc_registry".to_string(), Value::set(PySet::default()));
            }
            link_class_bases(pl_rc, vec![Rc::clone(base_rc)]);
        }
    }
}

/// Bind `os.path` through the module cache (single construction), mirror POSIX
/// exports on Unix, and populate `os.supports_fd` / `os.supports_follow_symlinks`.
pub(super) fn post_load_os_module(
    interp: &mut Interpreter,
    mod_rc: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // 1a. Bind `os.path` via `load_module` so `os.path` constructs at most once.
    let os_path = interp.load_module("os.path")?;
    mod_rc
        .borrow_mut()
        .insert_attr("path".to_string(), os_path);

    // 1b. On Unix, mirror POSIX-only exports and install `os.environb`.
    #[cfg(unix)]
    crate::builtin_modules::posix::install_posix_on_os(interp, mod_rc);

    // 2. Populate `os.supports_fd` and `os.supports_follow_symlinks`.
    let mut fd_set = PySet::default();
    let fd_names: &[&str] = if cfg!(unix) {
        &["stat", "truncate", "chown", "statvfs", "pathconf"]
    } else {
        &["stat", "truncate"]
    };
    for &fn_name in fd_names {
        if let Some(f) = mod_rc.borrow().attrs.get(fn_name).cloned()
            && let Ok(key) = interp.value_to_pykey(&f)
        {
            fd_set.insert(key);
        }
    }
    let mut follow_set = PySet::default();
    let follow_names: &[&str] = if cfg!(unix) {
        &["stat", "utime", "chmod", "chown"]
    } else {
        &["stat", "utime", "chmod"]
    };
    for &fn_name in follow_names {
        if let Some(f) = mod_rc.borrow().attrs.get(fn_name).cloned()
            && let Ok(key) = interp.value_to_pykey(&f)
        {
            follow_set.insert(key);
        }
    }
    let mut mod_mut = mod_rc.borrow_mut();
    mod_mut.insert_attr("supports_fd".to_string(), Value::set(fd_set));
    mod_mut.insert_attr(
        "supports_follow_symlinks".to_string(),
        Value::set(follow_set),
    );
    Ok(())
}
