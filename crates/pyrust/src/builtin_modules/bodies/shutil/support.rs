//! Interpreter plumbing shared by the native `shutil` port: the per-interpreter
//! module state, module-global lookups, call helpers, and the emulation of
//! the source's `try`/`except` clauses and `rmtree` error hooks.
//!
//! CPython's `shutil` resolves its dependencies (`os`, `sys`, `stat`,
//! `fnmatch`, ...) and its own helpers (`Error`, `copy2`, `COPY_BUFSIZE`, ...)
//! as module globals at call time, so the native code looks them up in the
//! module namespace as well: patching `shutil.os` or `os.rename` is seen the
//! same way.

use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::Sig;
use crate::error::{PyError, Result};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, class_is_subclass_of, lookup_exc_class, primitive_class_by_name,
    render_value_repr,
};
use crate::value::{PyClass, PyModule, Value, ValueKind};

pub(super) type ClassRc = Rc<RefCell<PyClass>>;

/// The `def` signature of a function of `Lib/shutil.py` (no `*args`).
pub(super) const fn sig(
    qualname: &'static str,
    params: &'static [&'static str],
    defaults: usize,
    kwonly: &'static [&'static str],
) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly,
        var_kw: false,
    }
}

// ---------------------------------------------------------------------------
// Per-interpreter state
// ---------------------------------------------------------------------------

/// What the native functions of the most recently imported `shutil` module
/// share: its namespace (the source's module globals), the archive and unpack
/// registries, and the hidden classes standing in for the source's closures.
pub(super) struct State {
    pub(super) module: Rc<RefCell<PyModule>>,
    /// `_ARCHIVE_FORMATS` (also published as the module attribute).
    pub(super) archive_formats: Value,
    /// `_UNPACK_FORMATS` (also published as the module attribute).
    pub(super) unpack_formats: Value,
    /// Class of the callable `ignore_patterns` returns.
    pub(super) ignore_class: Option<ClassRc>,
    /// Class of the `_set_uid_gid` tar filter `_make_tarball` builds.
    pub(super) owner_filter_class: Option<ClassRc>,
    /// `_ntuple_diskusage`, created on the first `disk_usage()` call so that
    /// `import shutil` does not have to import `collections`.
    pub(super) ntuple_diskusage: Option<Value>,
}

// Per-interpreter state (`gil_local!`):
// (a) Why: the archive / unpack registries (`register_archive_format` and
//     friends mutate them) and the hidden closure classes belong to the
//     importing interpreter; native functions have no globals of their own,
//     so they reach the module namespace through this slot.
// (b) Each interpreter (GIL context) has its own slot, so registrations never
//     leak between interpreters.  A re-import replaces the slot: the newest
//     module generation is the one whose globals are used.
pyrust_core::gil_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub(super) fn install_state(state: State) {
    STATE.with(|slot| *slot.borrow_mut() = Some(state));
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Result<R> {
    STATE.with(|slot| {
        slot.borrow_mut()
            .as_mut()
            .map(f)
            .ok_or_else(|| PyError::runtime("shutil: module state is not initialised".into()))
    })
}

pub(super) fn archive_formats() -> Result<Value> {
    with_state(|state| state.archive_formats.clone())
}

pub(super) fn unpack_formats() -> Result<Value> {
    with_state(|state| state.unpack_formats.clone())
}

pub(super) fn ignore_class() -> Result<ClassRc> {
    with_state(|state| state.ignore_class.clone())?
        .ok_or_else(|| PyError::runtime("shutil: ignore_patterns class is missing".into()))
}

pub(super) fn owner_filter_class() -> Result<ClassRc> {
    with_state(|state| state.owner_filter_class.clone())?
        .ok_or_else(|| PyError::runtime("shutil: tar owner filter class is missing".into()))
}

pub(super) fn cached_ntuple_diskusage() -> Option<Value> {
    with_state(|state| state.ntuple_diskusage.clone())
        .ok()
        .flatten()
}

/// Remember `_ntuple_diskusage` and publish it (with `collections`) in the
/// module namespace, as the source's module-level definition does.
pub(super) fn store_ntuple_diskusage(class: Value, collections: Value) {
    let _ = with_state(|state| {
        state.ntuple_diskusage = Some(class.clone());
        let mut module = state.module.borrow_mut();
        if module.get_attr_value("collections").is_none() {
            module.insert_attr("collections".to_string(), collections);
        }
        module.insert_attr("_ntuple_diskusage".to_string(), class);
    });
}

/// A global of the `shutil` module, read at call time like the source does.
pub(super) fn global(name: &str) -> Option<Value> {
    with_state(|state| state.module.borrow().get_attr_value(name))
        .ok()
        .flatten()
}

/// A module the source imports at module level (`os`, `sys`, `stat`, ...):
/// the module global when bound, else the module itself.
pub(super) fn dep(interp: &mut Interpreter, name: &str) -> Result<Value> {
    match global(name) {
        Some(module) => Ok(module),
        None => interp.load_module(name),
    }
}

/// A class defined by the module (`Error`, `SameFileError`, ...).
pub(super) fn global_class(name: &str) -> Result<ClassRc> {
    match global(name).as_ref().map(Value::kind) {
        Some(ValueKind::PyClass(class)) => Ok(Rc::clone(class)),
        _ => Err(PyError::named(
            "NameError",
            format!("name '{name}' is not defined"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

pub(super) fn call_kw(
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
    expanded.extend(kw.iter().map(|(name, value)| ExpandedCallArg {
        name: Some((*name).to_string()),
        value: value.clone(),
    }));
    interp.call_function_expanded(callee, &expanded)
}

pub(super) fn call(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    call_kw(interp, callee, pos, &[])
}

/// `callee(*pos, **kwargs)` with keyword names known only at run time.
pub(super) fn call_dyn(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kwargs: Vec<(String, Value)>,
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    expanded.extend(kwargs.into_iter().map(|(name, value)| ExpandedCallArg {
        name: Some(name),
        value,
    }));
    interp.call_function_expanded(callee, &expanded)
}

pub(super) fn method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call(interp, callee, pos)
}

pub(super) fn method_kw(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_kw(interp, callee, pos, kw)
}

/// A builtin (`open`, `callable`, `set`, `str`, ...) the way the source reaches
/// it through the builtins namespace.
pub(super) fn builtin(
    interp: &mut Interpreter,
    name: &'static str,
    pos: &[Value],
) -> Result<Value> {
    call(interp, Value::builtin_function(name), pos)
}

pub(super) fn os_module(interp: &mut Interpreter) -> Result<Value> {
    dep(interp, "os")
}

/// `os.<name>`.
pub(super) fn os_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let os = os_module(interp)?;
    interp.get_attr(&os, name)
}

/// `os.<name>(*pos)`.
pub(super) fn os_call(interp: &mut Interpreter, name: &str, pos: &[Value]) -> Result<Value> {
    let callee = os_attr(interp, name)?;
    call(interp, callee, pos)
}

/// `os.<name>(*pos, **kw)`.
pub(super) fn os_call_kw(
    interp: &mut Interpreter,
    name: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let callee = os_attr(interp, name)?;
    call_kw(interp, callee, pos, kw)
}

/// `os.path`.
pub(super) fn os_path(interp: &mut Interpreter) -> Result<Value> {
    os_attr(interp, "path")
}

/// `os.path.<name>`.
pub(super) fn path_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let path = os_path(interp)?;
    interp.get_attr(&path, name)
}

/// `os.path.<name>(*pos)`.
pub(super) fn path_call(interp: &mut Interpreter, name: &str, pos: &[Value]) -> Result<Value> {
    let callee = path_attr(interp, name)?;
    call(interp, callee, pos)
}

/// `bool(os.path.<name>(*pos))`.
pub(super) fn path_test(interp: &mut Interpreter, name: &str, pos: &[Value]) -> Result<bool> {
    let result = path_call(interp, name, pos)?;
    truthy(interp, &result)
}

/// `os.name == 'nt'`.
pub(super) fn os_name_is_nt(interp: &mut Interpreter) -> Result<bool> {
    let name = os_attr(interp, "name")?;
    Ok(name.as_str() == Some("nt"))
}

/// `sys.audit(event, *args)`, when the runtime provides it.
pub(super) fn audit(interp: &mut Interpreter, event: &str, args: &[Value]) -> Result<()> {
    let sys = dep(interp, "sys")?;
    if let Some(hook) = getattr_opt(interp, &sys, "audit")? {
        let mut pos = Vec::with_capacity(args.len() + 1);
        pos.push(Value::string(event));
        pos.extend_from_slice(args);
        call(interp, hook, &pos)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

pub(super) fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

pub(super) fn binary(
    interp: &mut Interpreter,
    a: &Value,
    op: BinaryOp,
    b: &Value,
) -> Result<Value> {
    interp.eval_binary(a.clone(), op, b.clone())
}

/// `a == b`.
pub(super) fn equal(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    let result = binary(interp, a, BinaryOp::Eq, b)?;
    truthy(interp, &result)
}

/// `item in container`.
pub(super) fn contains(interp: &mut Interpreter, container: &Value, item: &Value) -> Result<bool> {
    let result = interp.eval_in(container.clone(), item.clone())?;
    truthy(interp, &result)
}

/// `str(value)`.
pub(super) fn str_of(interp: &mut Interpreter, value: &Value) -> Result<Value> {
    builtin(interp, "str", std::slice::from_ref(value))
}

/// `repr(value)`.
pub(super) fn repr_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    render_value_repr(interp, value)
}

/// `fmt % args` (`args` is always passed as a tuple, so a tuple value is not
/// unpacked by accident).
pub(super) fn percent(interp: &mut Interpreter, fmt: &str, args: Vec<Value>) -> Result<Value> {
    interp.eval_binary(Value::string(fmt), BinaryOp::Mod, Value::tuple(args))
}

/// `obj[key]`.
pub(super) fn getitem(interp: &mut Interpreter, obj: &Value, key: Value) -> Result<Value> {
    crate::builtin_modules::operator::getitem_value(interp, obj.clone(), key)
}

/// `obj[key] = value`.
pub(super) fn setitem(
    interp: &mut Interpreter,
    obj: &Value,
    key: Value,
    value: Value,
) -> Result<()> {
    crate::builtin_modules::operator::setitem_value(interp, obj.clone(), key, value).map(|_| ())
}

/// `del obj[key]`.
pub(super) fn delitem(interp: &mut Interpreter, obj: &Value, key: Value) -> Result<()> {
    crate::builtin_modules::operator::delitem_value(interp, obj.clone(), key)
}

/// `list(iterable)` as a Rust vector.
pub(super) fn collect(interp: &mut Interpreter, iterable: &Value) -> Result<Vec<Value>> {
    interp.collect_iterable(iterable)
}

/// `a is b`.
pub(super) fn is_same(a: &Value, b: &Value) -> bool {
    a.is_identical_to(b)
}

/// `isinstance(value, cls)` through the full protocol.
pub(super) fn isinstance(interp: &mut Interpreter, value: &Value, cls: Value) -> Result<bool> {
    let result = builtin(interp, "isinstance", &[value.clone(), cls])?;
    truthy(interp, &result)
}

/// `isinstance(value, <builtin type name>)`.
pub(super) fn isinstance_builtin(
    interp: &mut Interpreter,
    value: &Value,
    type_name: &str,
) -> Result<bool> {
    let class = primitive_class_by_name(type_name)
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime(format!("shutil: missing builtin type {type_name}")))?;
    isinstance(interp, value, class)
}

pub(super) fn is_bytes(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    match value.kind() {
        ValueKind::Bytes(_) => Ok(true),
        ValueKind::Str(_) | ValueKind::None => Ok(false),
        _ => isinstance_builtin(interp, value, "bytes"),
    }
}

/// `isinstance(fn, os.DirEntry)`.
pub(super) fn is_direntry(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if matches!(
        value.kind(),
        ValueKind::Str(_) | ValueKind::Bytes(_) | ValueKind::None
    ) {
        return Ok(false);
    }
    let class = os_attr(interp, "DirEntry")?;
    isinstance(interp, value, class)
}

/// `getattr(obj, name)` or `None` when it raises `AttributeError`.
pub(super) fn getattr_opt(
    interp: &mut Interpreter,
    obj: &Value,
    name: &str,
) -> Result<Option<Value>> {
    let attribute_error = builtin_exc("AttributeError")?;
    match try_except(interp, &[attribute_error], |interp| {
        interp.get_attr(obj, name)
    })? {
        Ok(value) => Ok(Some(value)),
        Err(_) => Ok(None),
    }
}

/// `hasattr(obj, name)`.
pub(super) fn hasattr(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<bool> {
    Ok(getattr_opt(interp, obj, name)?.is_some())
}

/// `stat.S_IMODE(st.st_mode)`.
pub(super) fn s_imode(interp: &mut Interpreter, st: &Value) -> Result<Value> {
    let mode = interp.get_attr(st, "st_mode")?;
    binary(interp, &mode, BinaryOp::BitAnd, &Value::int(0o7777))
}

/// `stat.S_IFMT(mode) == fmt`.
pub(super) fn s_isfmt(interp: &mut Interpreter, mode: &Value, fmt: i64) -> Result<bool> {
    let kind = binary(interp, mode, BinaryOp::BitAnd, &Value::int(0o170_000))?;
    equal(interp, &kind, &Value::int(fmt))
}

pub(super) const S_IFDIR: i64 = 0o040_000;
pub(super) const S_IFIFO: i64 = 0o010_000;
pub(super) const S_IFLNK: i64 = 0o120_000;

// ---------------------------------------------------------------------------
// Exceptions
// ---------------------------------------------------------------------------

/// A builtin exception class (`OSError`, `KeyError`, ...).
pub(super) fn builtin_exc(name: &str) -> Result<ClassRc> {
    lookup_exc_class(name)
        .ok_or_else(|| PyError::runtime(format!("shutil: missing exception class {name}")))
}

/// An exception the source's `except` clause caught.
pub(super) struct Caught {
    /// The error as it was raised, for a bare `raise`.
    pub(super) error: PyError,
    /// The exception object (`except ... as exc`).
    pub(super) exc: Value,
    /// Which of the clause's classes matched first.
    pub(super) index: usize,
}

fn first_match(exc: &Value, classes: &[ClassRc]) -> Option<usize> {
    let ValueKind::PyInstance(instance) = exc.kind() else {
        return None;
    };
    let class = Rc::clone(&instance.borrow().class);
    classes
        .iter()
        .position(|expected| class_is_subclass_of(&class, expected))
}

/// `try: f() except (classes...) as exc:` — `Ok(Err(caught))` when the
/// exception matches one of `classes` (checked in order, like consecutive
/// `except` clauses).  A handled exception's captured traceback frames are
/// dropped, as leaving an `except` clause does, so they cannot leak into a
/// later, unrelated exception.  Anything else propagates unchanged.
pub(super) fn try_except<T>(
    interp: &mut Interpreter,
    classes: &[ClassRc],
    f: impl FnOnce(&mut Interpreter) -> Result<T>,
) -> Result<std::result::Result<T, Caught>> {
    let saved = (pyrust_core::captured_error_frames_len() > 0)
        .then(pyrust_core::clone_captured_error_frames);
    let error = match f(interp) {
        Ok(value) => return Ok(Ok(value)),
        Err(error) => error,
    };
    let Ok(exc) = interp.materialize_pyerror(error.clone()) else {
        return Err(error);
    };
    let Some(index) = first_match(&exc, classes) else {
        return Err(error);
    };
    pyrust_core::reset_captured_error_frames();
    for frame in saved.into_iter().flatten().rev() {
        pyrust_core::record_traceback_frame(frame);
    }
    Ok(Err(Caught { error, exc, index }))
}

/// `try: f() except OSError as exc:`.
pub(super) fn try_os<T>(
    interp: &mut Interpreter,
    f: impl FnOnce(&mut Interpreter) -> Result<T>,
) -> Result<std::result::Result<T, Caught>> {
    let os_error = builtin_exc("OSError")?;
    try_except(interp, &[os_error], f)
}

/// Instantiate `class(*args)` and return it as a raised error.
pub(super) fn raise_class(interp: &mut Interpreter, class: ClassRc, args: &[Value]) -> PyError {
    match call(interp, Value::py_class(class), args) {
        Ok(exc) => PyError::raised(exc),
        Err(err) => err,
    }
}

/// `raise <module class>(*args)` (`Error`, `SameFileError`, `ReadError`, ...).
pub(super) fn raise_global(interp: &mut Interpreter, name: &str, args: &[Value]) -> PyError {
    match global_class(name) {
        Ok(class) => raise_class(interp, class, args),
        Err(err) => err,
    }
}

/// `raise <builtin exception>(*args)`.
pub(super) fn raise_builtin(interp: &mut Interpreter, name: &str, args: &[Value]) -> PyError {
    match builtin_exc(name) {
        Ok(class) => raise_class(interp, class, args),
        Err(err) => err,
    }
}

/// An exception instance of `<builtin exception>(*args)`.
pub(super) fn new_builtin_exc(
    interp: &mut Interpreter,
    name: &str,
    args: &[Value],
) -> Result<Value> {
    let class = builtin_exc(name)?;
    call(interp, Value::py_class(class), args)
}

/// `raise exc from cause` inside the `except` clause that caught `cause`.
pub(super) fn raise_from(interp: &mut Interpreter, exc: Value, cause: Value) -> PyError {
    let _ = interp.assign_attr(exc.clone(), "__context__", cause.clone());
    let _ = interp.assign_attr(exc.clone(), "__cause__", cause);
    let _ = interp.assign_attr(exc.clone(), "__suppress_context__", Value::bool_(true));
    PyError::raised(exc)
}

/// The exception object for an error (OSError subclasses keep their `errno`,
/// `strerror` and `filename`).
#[cfg(unix)]
pub(super) fn exception_value(interp: &mut Interpreter, error: PyError) -> Result<Value> {
    interp.materialize_pyerror(error)
}

// ---------------------------------------------------------------------------
// rmtree error hooks
// ---------------------------------------------------------------------------

/// The `onexc` callback `rmtree` builds from its arguments.
pub(super) enum OnExc {
    /// `ignore_errors=True`: `def onexc(*args): pass`.
    Ignore,
    /// No hook: `def onexc(*args): raise`.
    Raise,
    /// `onerror` given: called with `(func, path, sys.exc_info())`.
    OnError(Value),
    /// `onexc` given: called with `(func, path, exc)`.
    Call(Value),
}

impl OnExc {
    /// `onexc(func, path, exc)` for an exception the source just caught.
    pub(super) fn handle(
        &self,
        interp: &mut Interpreter,
        func: Value,
        path: Value,
        caught: Caught,
    ) -> Result<()> {
        match self {
            Self::Ignore => Ok(()),
            Self::Raise => Err(caught.error),
            Self::Call(hook) => call(interp, hook.clone(), &[func, path, caught.exc]).map(|_| ()),
            Self::OnError(hook) => {
                let exc = caught.exc;
                let exc_type = match exc.kind() {
                    ValueKind::PyInstance(instance) => {
                        Value::py_class(Rc::clone(&instance.borrow().class))
                    }
                    _ => builtin(interp, "type", std::slice::from_ref(&exc))?,
                };
                let traceback = interp.get_attr(&exc, "__traceback__")?;
                let exc_info = Value::tuple(vec![exc_type, exc, traceback]);
                call(interp, hook.clone(), &[func, path, exc_info]).map(|_| ())
            }
        }
    }

    /// `onexc(func, path, err)` for an error raised natively (an `OSError`
    /// built from an OS error code, or `OSError("Cannot call rmtree ...")`).
    #[cfg(unix)]
    pub(super) fn handle_error(
        &self,
        interp: &mut Interpreter,
        func: Value,
        path: Value,
        error: PyError,
    ) -> Result<()> {
        if matches!(self, Self::Ignore) {
            return Ok(());
        }
        let exc = exception_value(interp, error.clone())?;
        self.handle(
            interp,
            func,
            path,
            Caught {
                error,
                exc,
                index: 0,
            },
        )
    }
}

/// `OSError("Cannot call rmtree on a symbolic link")`.
pub(super) fn symlink_rmtree_error(interp: &mut Interpreter) -> PyError {
    raise_builtin(
        interp,
        "OSError",
        &[Value::string("Cannot call rmtree on a symbolic link")],
    )
}
