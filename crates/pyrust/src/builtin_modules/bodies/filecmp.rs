// `filecmp` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/filecmp.py` (issues #3230, #3471).
//
// The module mirrors the CPython source structure: `cmp` / `_sig` /
// `_do_cmp` with the shared `_cache`, `cmpfiles` / `_cmp` / `_filter`,
// `demo`, and the lazily evaluated `dircmp` class (`phase0`..`phase4`,
// `phase4_closure`, `report*`, `methodmap`, `__getattr__`).  Like the Python
// source, every step reaches the file system, `os.path`, `open`, `print` and
// the sibling functions / methods through ordinary lookups (`os.stat`,
// `self.phase4()`, module globals such as `cmpfiles`, ...), so subclasses,
// monkeypatching of `filecmp.os` / `filecmp._cache` and redirected
// `sys.stdout` behave exactly as in CPython.
//
// CPython 3.12.3's `dircmp` has no `shallow` parameter (it was added in
// 3.13): `dircmp(a, b, ignore=None, hide=None)`.
//
// Reference: <https://docs.python.org/3.12/library/filecmp.html>

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

const MODULE_DOC: &str = "Utilities for comparing files and directories.\n\nClasses:\n    dircmp\n\nFunctions:\n    cmp(f1, f2, shallow=True) -> int\n    cmpfiles(a, b, common) -> ([], [], [])\n    clear_cache()\n\n";

const DIRCMP_DOC: &str = "A class that manages the comparison of 2 directories.\n\n    dircmp(a, b, ignore=None, hide=None)\n      A and B are directories.\n      IGNORE is a list of names to ignore,\n        defaults to DEFAULT_IGNORES.\n      HIDE is a list of names to hide,\n        defaults to [os.curdir, os.pardir].\n\n    High level usage:\n      x = dircmp(dir1, dir2)\n      x.report() -> prints a report on the differences between dir1 and dir2\n       or\n      x.report_partial_closure() -> prints report on differences between dir1\n            and dir2, and reports on common immediate subdirectories.\n      x.report_full_closure() -> like report_partial_closure,\n            but fully recursive.\n\n    Attributes:\n     left_list, right_list: The files in dir1 and dir2,\n        filtered by hide and ignore.\n     common: a list of names in both dir1 and dir2.\n     left_only, right_only: names only in dir1, dir2.\n     common_dirs: subdirectories in both dir1 and dir2.\n     common_files: files in both dir1 and dir2.\n     common_funny: names in both dir1 and dir2 where the type differs between\n        dir1 and dir2, or the name is not stat-able.\n     same_files: list of identical files.\n     diff_files: list of filenames which differ.\n     funny_files: list of files which could not be compared.\n     subdirs: a dictionary of dircmp instances (or MyDirCmp instances if this\n       object is of type MyDirCmp, a subclass of dircmp), keyed by names\n       in common_dirs.\n     ";

/// `stat.S_IFMT` mask and the file-type bits `filecmp` tests.
const S_IFMT: i64 = 0o170000;
const S_IFDIR: i64 = 0o040000;
const S_IFREG: i64 = 0o100000;

/// `BUFSIZE = 8*1024`.
const BUFSIZE: i64 = 8 * 1024;

/// The cache is cleared once it holds more than this many entries.
const CACHE_LIMIT: i64 = 100;

// The `def`s of `Lib/filecmp.py`: CPython reports arity errors for these in
// the interpreter's `initialize_locals` wording, not the macro's.
const SIG_CLEAR_CACHE: Sig = Sig {
    qualname: "clear_cache",
    params: &[],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_CMP: Sig = Sig {
    qualname: "cmp",
    params: &["f1", "f2", "shallow"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_SIG: Sig = Sig {
    qualname: "_sig",
    params: &["st"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_DO_CMP: Sig = Sig {
    qualname: "_do_cmp",
    params: &["f1", "f2"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_INIT: Sig = Sig {
    qualname: "dircmp.__init__",
    params: &["self", "a", "b", "ignore", "hide"],
    defaults: 2,
    kwonly: &[],
    var_kw: false,
};
const SIG_SELF: [Sig; 8] = [
    self_sig("dircmp.phase0"),
    self_sig("dircmp.phase1"),
    self_sig("dircmp.phase2"),
    self_sig("dircmp.phase3"),
    self_sig("dircmp.phase4"),
    self_sig("dircmp.phase4_closure"),
    self_sig("dircmp.report"),
    self_sig("dircmp.report_partial_closure"),
];
const SIG_REPORT_FULL_CLOSURE: Sig = self_sig("dircmp.report_full_closure");
const SIG_GETATTR: Sig = Sig {
    qualname: "dircmp.__getattr__",
    params: &["self", "attr"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_CMPFILES: Sig = Sig {
    qualname: "cmpfiles",
    params: &["a", "b", "common", "shallow"],
    defaults: 1,
    kwonly: &[],
    var_kw: false,
};
const SIG_PRIVATE_CMP: Sig = Sig {
    qualname: "_cmp",
    params: &["a", "b", "sh", "abs", "cmp"],
    defaults: 2,
    kwonly: &[],
    var_kw: false,
};
const SIG_FILTER: Sig = Sig {
    qualname: "_filter",
    params: &["flist", "skip"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};
const SIG_DEMO: Sig = Sig {
    qualname: "demo",
    params: &[],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

const fn self_sig(qualname: &'static str) -> Sig {
    Sig {
        qualname,
        params: &["self"],
        defaults: 0,
        kwonly: &[],
        var_kw: false,
    }
}

// Indices into `SIG_SELF`.
const PHASE0: usize = 0;
const PHASE1: usize = 1;
const PHASE2: usize = 2;
const PHASE3: usize = 3;
const PHASE4: usize = 4;
const PHASE4_CLOSURE: usize = 5;
const REPORT: usize = 6;
const REPORT_PARTIAL_CLOSURE: usize = 7;

fn all_list() -> Value {
    Value::list(
        [
            "clear_cache",
            "cmp",
            "dircmp",
            "cmpfiles",
            "DEFAULT_IGNORES",
        ]
        .into_iter()
        .map(Value::string)
        .collect(),
    )
}

fn default_ignores() -> Value {
    Value::list(
        [
            "RCS",
            "CVS",
            "tags",
            ".git",
            ".hg",
            ".bzr",
            "_darcs",
            "__pycache__",
        ]
        .into_iter()
        .map(Value::string)
        .collect(),
    )
}

// ---------------------------------------------------------------------------
// Interpreter plumbing
// ---------------------------------------------------------------------------

fn call_pos(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    target: &Value,
    name: &str,
    pos: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, pos)
}

/// A module global of `filecmp` (`_cache`, `_sig`, `cmpfiles`, ...), looked up
/// at call time like a Python global.
fn global(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("filecmp")?;
    interp.get_attr(&module, name)
}

/// A module-level dependency (`os`): the module attribute when present
/// (CPython resolves `import os` as a module global), otherwise the imported
/// module itself.
fn dep(interp: &mut Interpreter, name: &str) -> Result<Value> {
    if let Ok(module) = interp.load_module("filecmp")
        && let Ok(value) = interp.get_attr(&module, name)
    {
        return Ok(value);
    }
    interp.load_module(name)
}

/// `os.<name>`.
fn os_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let os = dep(interp, "os")?;
    interp.get_attr(&os, name)
}

/// `os.path.<name>(*args)`.
fn os_path_call(interp: &mut Interpreter, name: &str, pos: &[Value]) -> Result<Value> {
    let path = os_attr(interp, "path")?;
    call_method(interp, &path, name, pos)
}

fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

/// `a == b`.
fn values_equal(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    let result = interp.eval_binary(a.clone(), BinaryOp::Eq, b.clone())?;
    truthy(interp, &result)
}

/// `item in container`.
fn contains(interp: &mut Interpreter, item: &Value, container: &Value) -> Result<bool> {
    let result = interp.eval_binary(item.clone(), BinaryOp::In, container.clone())?;
    truthy(interp, &result)
}

/// `print(*values)`.
fn print_values(interp: &mut Interpreter, values: &[Value]) -> Result<()> {
    call_pos(interp, Value::builtin_function("print"), values)?;
    Ok(())
}

/// Run `f`.  An error accepted by `swallow` is handed back as `Ok(Err(err))`
/// after the traceback frames recorded while it unwound are dropped, so the
/// swallowed exception (CPython's `except` clause) does not leak frames into a
/// later, unrelated exception.  Any other error propagates unchanged.
fn try_swallow<T>(
    f: impl FnOnce() -> Result<T>,
    swallow: impl FnOnce(&PyError) -> bool,
) -> Result<std::result::Result<T, PyError>> {
    let saved = pyrust_core::clone_captured_error_frames();
    match f() {
        Ok(value) => Ok(Ok(value)),
        Err(err) if swallow(&err) => {
            pyrust_core::reset_captured_error_frames();
            for frame in saved.into_iter().rev() {
                pyrust_core::record_traceback_frame(frame);
            }
            Ok(Err(err))
        }
        Err(err) => Err(err),
    }
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn value_int(value: &Value, what: &str) -> Result<i64> {
    value
        .as_int()
        .ok_or_else(|| type_error(format!("{what} must be an int")))
}

/// `with cm as fp: body(fp)`.
fn with_context<T>(
    interp: &mut Interpreter,
    cm: &Value,
    body: impl FnOnce(&mut Interpreter, &Value) -> Result<T>,
) -> Result<T> {
    let fp = call_method(interp, cm, "__enter__", &[])?;
    let result = body(interp, &fp);
    let exit_args = [Value::none(), Value::none(), Value::none()];
    match result {
        Ok(value) => {
            call_method(interp, cm, "__exit__", &exit_args)?;
            Ok(value)
        }
        Err(err) => {
            let _ = call_method(interp, cm, "__exit__", &exit_args);
            Err(err)
        }
    }
}

/// `open(path, "rb")`.
fn open_rb(interp: &mut Interpreter, path: &Value) -> Result<Value> {
    let builtins = interp.load_module("builtins")?;
    let open = interp.get_attr(&builtins, "open")?;
    call_pos(interp, open, &[path.clone(), Value::string("rb")])
}

// ---------------------------------------------------------------------------
// Module functions
// ---------------------------------------------------------------------------

/// `_cache.clear()`.
fn clear_cache_impl(interp: &mut Interpreter) -> Result<Value> {
    let cache = global(interp, "_cache")?;
    call_method(interp, &cache, "clear", &[])?;
    Ok(Value::none())
}

/// `_sig(st)`.
fn sig_impl(interp: &mut Interpreter, st: &Value) -> Result<Value> {
    let mode = interp.get_attr(st, "st_mode")?;
    let fmt = value_int(&mode, "st_mode")? & S_IFMT;
    let size = interp.get_attr(st, "st_size")?;
    let mtime = interp.get_attr(st, "st_mtime")?;
    Ok(Value::tuple(vec![Value::int(fmt), size, mtime]))
}

fn sig_item(interp: &mut Interpreter, sig: &Value, index: i64) -> Result<Value> {
    call_method(interp, sig, "__getitem__", &[Value::int(index)])
}

/// `_do_cmp(f1, f2)`.
fn do_cmp_impl(interp: &mut Interpreter, f1: &Value, f2: &Value) -> Result<Value> {
    let bufsize = global(interp, "BUFSIZE")?;
    let cm1 = open_rb(interp, f1)?;
    with_context(interp, &cm1, |interp, fp1| {
        let cm2 = open_rb(interp, f2)?;
        with_context(interp, &cm2, |interp, fp2| {
            loop {
                let b1 = call_method(interp, fp1, "read", std::slice::from_ref(&bufsize))?;
                let b2 = call_method(interp, fp2, "read", std::slice::from_ref(&bufsize))?;
                if !values_equal(interp, &b1, &b2)? {
                    return Ok(Value::bool_(false));
                }
                if !truthy(interp, &b1)? {
                    return Ok(Value::bool_(true));
                }
            }
        })
    })
}

/// `cmp(f1, f2, shallow=True)`.
fn cmp_impl(interp: &mut Interpreter, f1: Value, f2: Value, shallow: &Value) -> Result<Value> {
    let os_stat = os_attr(interp, "stat")?;
    let sig_fn = global(interp, "_sig")?;
    let st1 = call_pos(interp, os_stat.clone(), std::slice::from_ref(&f1))?;
    let s1 = call_pos(interp, sig_fn.clone(), &[st1])?;
    let st2 = call_pos(interp, os_stat, std::slice::from_ref(&f2))?;
    let s2 = call_pos(interp, sig_fn, &[st2])?;

    let regular = Value::int(S_IFREG);
    let t1 = sig_item(interp, &s1, 0)?;
    let t2 = sig_item(interp, &s2, 0)?;
    if !values_equal(interp, &t1, &regular)? || !values_equal(interp, &t2, &regular)? {
        return Ok(Value::bool_(false));
    }
    if truthy(interp, shallow)? && values_equal(interp, &s1, &s2)? {
        return Ok(Value::bool_(true));
    }
    let n1 = sig_item(interp, &s1, 1)?;
    let n2 = sig_item(interp, &s2, 1)?;
    if !values_equal(interp, &n1, &n2)? {
        return Ok(Value::bool_(false));
    }

    let cache = global(interp, "_cache")?;
    let key = Value::tuple(vec![f1.clone(), f2.clone(), s1, s2]);
    let outcome = call_method(interp, &cache, "get", std::slice::from_ref(&key))?;
    if !outcome.is_none() {
        return Ok(outcome);
    }
    let do_cmp = global(interp, "_do_cmp")?;
    let outcome = call_pos(interp, do_cmp, &[f1, f2])?;
    // limit the maximum size of the cache
    let len = call_method(interp, &cache, "__len__", &[])?;
    if value_int(&len, "len(_cache)")? > CACHE_LIMIT {
        let clear = global(interp, "clear_cache")?;
        call_pos(interp, clear, &[])?;
    }
    let cache = global(interp, "_cache")?;
    call_method(interp, &cache, "__setitem__", &[key, outcome.clone()])?;
    Ok(outcome)
}

/// `_cmp(a, b, sh, abs=abs, cmp=cmp)`: 0 for equal, 1 for different, 2 for
/// funny cases (can't stat, etc.).
fn private_cmp_impl(
    interp: &mut Interpreter,
    a: Value,
    b: Value,
    sh: Value,
    abs_fn: Option<Value>,
    cmp_fn: Option<Value>,
) -> Result<Value> {
    let outcome = try_swallow(
        || -> Result<bool> {
            let equal = match cmp_fn {
                Some(cmp_fn) => call_pos(interp, cmp_fn, &[a, b, sh])?,
                None => cmp_impl(interp, a, b, &sh)?,
            };
            let abs_fn = abs_fn.unwrap_or_else(|| Value::builtin_function("abs"));
            let magnitude = call_pos(interp, abs_fn, &[equal])?;
            Ok(!truthy(interp, &magnitude)?)
        },
        |err| err.class_name_is("OSError"),
    )?;
    match outcome {
        Ok(different) => Ok(Value::bool_(different)),
        Err(_) => Ok(Value::int(2)),
    }
}

/// `cmpfiles(a, b, common, shallow=True)`.
fn cmpfiles_impl(
    interp: &mut Interpreter,
    a: &Value,
    b: &Value,
    common: &Value,
    shallow: &Value,
) -> Result<Value> {
    let res = Value::tuple(vec![
        Value::list(Vec::new()),
        Value::list(Vec::new()),
        Value::list(Vec::new()),
    ]);
    for x in interp.collect_iterable(common)? {
        let ax = os_path_call(interp, "join", &[a.clone(), x.clone()])?;
        let bx = os_path_call(interp, "join", &[b.clone(), x.clone()])?;
        let private_cmp = global(interp, "_cmp")?;
        let verdict = call_pos(interp, private_cmp, &[ax, bx, shallow.clone()])?;
        let bucket = call_method(interp, &res, "__getitem__", &[verdict])?;
        call_method(interp, &bucket, "append", &[x])?;
    }
    Ok(res)
}

/// `_filter(flist, skip)`: a copy with items that occur in skip removed.
fn filter_impl(interp: &mut Interpreter, flist: &Value, skip: &Value) -> Result<Value> {
    let contains_fn = interp.get_attr(skip, "__contains__")?;
    let mut kept = Vec::new();
    for item in interp.collect_iterable(flist)? {
        let found = call_pos(interp, contains_fn.clone(), std::slice::from_ref(&item))?;
        if !truthy(interp, &found)? {
            kept.push(item);
        }
    }
    Ok(Value::list(kept))
}

/// `demo()`: the command-line demonstration of `Lib/filecmp.py`.
fn demo_impl(interp: &mut Interpreter) -> Result<Value> {
    let getopt = interp.load_module("getopt")?;
    let sys = interp.load_module("sys")?;
    let argv = interp.get_attr(&sys, "argv")?;
    let slice = Value::list(
        interp
            .collect_iterable(&argv)?
            .into_iter()
            .skip(1)
            .collect(),
    );
    let parsed = call_method(interp, &getopt, "getopt", &[slice, Value::string("r")])?;
    let options = call_method(interp, &parsed, "__getitem__", &[Value::int(0)])?;
    let rest = call_method(interp, &parsed, "__getitem__", &[Value::int(1)])?;
    let nargs = call_pos(
        interp,
        Value::builtin_function("len"),
        std::slice::from_ref(&rest),
    )?;
    if value_int(&nargs, "len(args)")? != 2 {
        let error = interp.get_attr(&getopt, "GetoptError")?;
        let exc = call_pos(
            interp,
            error,
            &[Value::string("need exactly two args"), Value::none()],
        )?;
        return Err(PyError::raised(exc));
    }
    let first = call_method(interp, &rest, "__getitem__", &[Value::int(0)])?;
    let second = call_method(interp, &rest, "__getitem__", &[Value::int(1)])?;
    let dircmp = global(interp, "dircmp")?;
    let dd = call_pos(interp, dircmp, &[first, second])?;
    let flag = Value::tuple(vec![Value::string("-r"), Value::string("")]);
    if contains(interp, &flag, &options)? {
        call_method(interp, &dd, "report_full_closure", &[])?;
    } else {
        call_method(interp, &dd, "report", &[])?;
    }
    Ok(Value::none())
}

// ---------------------------------------------------------------------------
// `dircmp`
// ---------------------------------------------------------------------------

fn get(interp: &mut Interpreter, this: &Value, name: &str) -> Result<Value> {
    interp.get_attr(this, name)
}

fn set(interp: &mut Interpreter, this: &Value, name: &str, value: Value) -> Result<()> {
    interp.assign_attr(this.clone(), name, value)
}

fn dircmp_init(
    interp: &mut Interpreter,
    this: &Value,
    a: Value,
    b: Value,
    ignore: Value,
    hide: Value,
) -> Result<()> {
    set(interp, this, "left", a)?;
    set(interp, this, "right", b)?;
    if hide.is_none() {
        // Names never to be shown
        let curdir = os_attr(interp, "curdir")?;
        let pardir = os_attr(interp, "pardir")?;
        set(interp, this, "hide", Value::list(vec![curdir, pardir]))?;
    } else {
        set(interp, this, "hide", hide)?;
    }
    if ignore.is_none() {
        let ignores = global(interp, "DEFAULT_IGNORES")?;
        set(interp, this, "ignore", ignores)?;
    } else {
        set(interp, this, "ignore", ignore)?;
    }
    Ok(())
}

/// Compare everything except common subdirectories.
fn phase0(interp: &mut Interpreter, this: &Value) -> Result<()> {
    for (dir_attr, list_attr) in [("left", "left_list"), ("right", "right_list")] {
        let dir = get(interp, this, dir_attr)?;
        let listdir = os_attr(interp, "listdir")?;
        let names = call_pos(interp, listdir, &[dir])?;
        let hide = get(interp, this, "hide")?;
        let ignore = get(interp, this, "ignore")?;
        let skip = interp.eval_binary(hide, BinaryOp::Add, ignore)?;
        let filter = global(interp, "_filter")?;
        let filtered = call_pos(interp, filter, &[names, skip])?;
        set(interp, this, list_attr, filtered)?;
    }
    for list_attr in ["left_list", "right_list"] {
        let list = get(interp, this, list_attr)?;
        call_method(interp, &list, "sort", &[])?;
    }
    Ok(())
}

/// `dict(zip(map(os.path.normcase, names), names))`.
fn normcase_map(interp: &mut Interpreter, names: &Value) -> Result<Value> {
    let map = Value::dict(PyDict::default());
    for name in interp.collect_iterable(names)? {
        let key = os_path_call(interp, "normcase", std::slice::from_ref(&name))?;
        call_method(interp, &map, "__setitem__", &[key, name])?;
    }
    Ok(map)
}

/// `list(map(src.__getitem__, [k for k in keys if (k in other) == wanted]))`
fn select_names(
    interp: &mut Interpreter,
    src: &Value,
    other: &Value,
    wanted: bool,
) -> Result<Value> {
    let mut out = Vec::new();
    for key in interp.collect_iterable(src)? {
        if contains(interp, &key, other)? == wanted {
            out.push(call_method(interp, src, "__getitem__", &[key])?);
        }
    }
    Ok(Value::list(out))
}

/// Compute common names.
fn phase1(interp: &mut Interpreter, this: &Value) -> Result<()> {
    let left_list = get(interp, this, "left_list")?;
    let a = normcase_map(interp, &left_list)?;
    let right_list = get(interp, this, "right_list")?;
    let b = normcase_map(interp, &right_list)?;
    let common = select_names(interp, &a, &b, true)?;
    set(interp, this, "common", common)?;
    let left_only = select_names(interp, &a, &b, false)?;
    set(interp, this, "left_only", left_only)?;
    let right_only = select_names(interp, &b, &a, false)?;
    set(interp, this, "right_only", right_only)?;
    Ok(())
}

/// `try: os.stat(path) except OSError: None`.
fn stat_or_none(interp: &mut Interpreter, path: Value) -> Result<Option<Value>> {
    let outcome = try_swallow(
        || -> Result<Value> {
            let os_stat = os_attr(interp, "stat")?;
            call_pos(interp, os_stat, &[path])
        },
        |err| err.class_name_is("OSError"),
    )?;
    Ok(outcome.ok())
}

fn st_mode_type(interp: &mut Interpreter, st: &Value) -> Result<i64> {
    let mode = interp.get_attr(st, "st_mode")?;
    Ok(value_int(&mode, "st_mode")? & S_IFMT)
}

/// Distinguish files, directories, funnies.
fn phase2(interp: &mut Interpreter, this: &Value) -> Result<()> {
    let common_dirs = Value::list(Vec::new());
    let common_files = Value::list(Vec::new());
    let common_funny = Value::list(Vec::new());
    set(interp, this, "common_dirs", common_dirs.clone())?;
    set(interp, this, "common_files", common_files.clone())?;
    set(interp, this, "common_funny", common_funny.clone())?;

    let common = get(interp, this, "common")?;
    for x in interp.collect_iterable(&common)? {
        let left = get(interp, this, "left")?;
        let a_path = os_path_call(interp, "join", &[left, x.clone()])?;
        let right = get(interp, this, "right")?;
        let b_path = os_path_call(interp, "join", &[right, x.clone()])?;

        let a_stat = stat_or_none(interp, a_path)?;
        let b_stat = stat_or_none(interp, b_path)?;
        let bucket = match (a_stat, b_stat) {
            (Some(a_stat), Some(b_stat)) => {
                let a_type = st_mode_type(interp, &a_stat)?;
                let b_type = st_mode_type(interp, &b_stat)?;
                if a_type != b_type {
                    &common_funny
                } else if a_type == S_IFDIR {
                    &common_dirs
                } else if a_type == S_IFREG {
                    &common_files
                } else {
                    &common_funny
                }
            }
            _ => &common_funny,
        };
        bucket.list_push(x)?;
    }
    Ok(())
}

/// Find out differences between common files.
fn phase3(interp: &mut Interpreter, this: &Value) -> Result<()> {
    let left = get(interp, this, "left")?;
    let right = get(interp, this, "right")?;
    let common_files = get(interp, this, "common_files")?;
    let cmpfiles = global(interp, "cmpfiles")?;
    let xx = call_pos(interp, cmpfiles, &[left, right, common_files])?;
    let items = interp.collect_iterable(&xx)?;
    if items.len() < 3 {
        return Err(PyError::named(
            "ValueError",
            format!(
                "not enough values to unpack (expected 3, got {})",
                items.len()
            ),
        ));
    }
    if items.len() > 3 {
        return Err(PyError::named(
            "ValueError",
            "too many values to unpack (expected 3)".to_string(),
        ));
    }
    let mut items = items.into_iter();
    for name in ["same_files", "diff_files", "funny_files"] {
        let value = items.next().unwrap_or_else(Value::none);
        set(interp, this, name, value)?;
    }
    Ok(())
}

/// Find out differences between common subdirectories.
///
/// A new dircmp (or MyDirCmp if dircmp was subclassed) object is created for
/// each common subdirectory, these are stored in a dictionary indexed by
/// filename.  The hide and ignore properties are inherited from the parent.
fn phase4(interp: &mut Interpreter, this: &Value) -> Result<()> {
    set(interp, this, "subdirs", Value::dict(PyDict::default()))?;
    let common_dirs = get(interp, this, "common_dirs")?;
    for x in interp.collect_iterable(&common_dirs)? {
        let left = get(interp, this, "left")?;
        let a_x = os_path_call(interp, "join", &[left, x.clone()])?;
        let right = get(interp, this, "right")?;
        let b_x = os_path_call(interp, "join", &[right, x.clone()])?;
        let class = get(interp, this, "__class__")?;
        let ignore = get(interp, this, "ignore")?;
        let hide = get(interp, this, "hide")?;
        let sub = call_pos(interp, class, &[a_x, b_x, ignore, hide])?;
        let subdirs = get(interp, this, "subdirs")?;
        call_method(interp, &subdirs, "__setitem__", &[x, sub])?;
    }
    Ok(())
}

/// `list(self.subdirs.values())`, iterated lazily by the callers' loops.
fn subdir_values(interp: &mut Interpreter, this: &Value) -> Result<Vec<Value>> {
    let subdirs = get(interp, this, "subdirs")?;
    let values = call_method(interp, &subdirs, "values", &[])?;
    interp.collect_iterable(&values)
}

/// Recursively call phase4() on subdirectories.
fn phase4_closure(interp: &mut Interpreter, this: &Value) -> Result<()> {
    call_method(interp, this, "phase4", &[])?;
    for sd in subdir_values(interp, this)? {
        call_method(interp, &sd, "phase4_closure", &[])?;
    }
    Ok(())
}

/// `if self.<attr>: self.<attr>.sort(); print(label..., self.<attr>)`.
fn report_list(
    interp: &mut Interpreter,
    this: &Value,
    attr: &str,
    label: &[Value],
    dir_attr: Option<&str>,
) -> Result<()> {
    let list = get(interp, this, attr)?;
    if !truthy(interp, &list)? {
        return Ok(());
    }
    let list = get(interp, this, attr)?;
    call_method(interp, &list, "sort", &[])?;
    let mut printed: Vec<Value> = label.to_vec();
    if let Some(dir_attr) = dir_attr {
        printed.push(get(interp, this, dir_attr)?);
        printed.push(Value::string(":"));
    }
    printed.push(get(interp, this, attr)?);
    print_values(interp, &printed)
}

/// Print a report on the differences between a and b.
/// Output format is purposely lousy.
fn report(interp: &mut Interpreter, this: &Value) -> Result<()> {
    let left = get(interp, this, "left")?;
    let right = get(interp, this, "right")?;
    print_values(interp, &[Value::string("diff"), left, right])?;
    let only_in = [Value::string("Only in")];
    report_list(interp, this, "left_only", &only_in, Some("left"))?;
    report_list(interp, this, "right_only", &only_in, Some("right"))?;
    report_list(
        interp,
        this,
        "same_files",
        &[Value::string("Identical files :")],
        None,
    )?;
    report_list(
        interp,
        this,
        "diff_files",
        &[Value::string("Differing files :")],
        None,
    )?;
    report_list(
        interp,
        this,
        "funny_files",
        &[Value::string("Trouble with common files :")],
        None,
    )?;
    report_list(
        interp,
        this,
        "common_dirs",
        &[Value::string("Common subdirectories :")],
        None,
    )?;
    report_list(
        interp,
        this,
        "common_funny",
        &[Value::string("Common funny cases :")],
        None,
    )
}

/// Print reports on self and on subdirs.
fn report_partial_closure(interp: &mut Interpreter, this: &Value) -> Result<()> {
    call_method(interp, this, "report", &[])?;
    for sd in subdir_values(interp, this)? {
        print_values(interp, &[])?;
        call_method(interp, &sd, "report", &[])?;
    }
    Ok(())
}

/// Report on self and subdirs recursively.
fn report_full_closure(interp: &mut Interpreter, this: &Value) -> Result<()> {
    call_method(interp, this, "report", &[])?;
    for sd in subdir_values(interp, this)? {
        print_values(interp, &[])?;
        call_method(interp, &sd, "report_full_closure", &[])?;
    }
    Ok(())
}

/// `dircmp.__getattr__`: compute the attribute lazily through `methodmap`.
fn dircmp_getattr(interp: &mut Interpreter, this: &Value, attr: Value) -> Result<Value> {
    let methodmap = get(interp, this, "methodmap")?;
    if !contains(interp, &attr, &methodmap)? {
        let name = attr.as_str().unwrap_or_default().to_string();
        return Err(PyError::named("AttributeError", name));
    }
    let methodmap = get(interp, this, "methodmap")?;
    let phase = call_method(
        interp,
        &methodmap,
        "__getitem__",
        std::slice::from_ref(&attr),
    )?;
    call_pos(interp, phase, std::slice::from_ref(this))?;
    let name = attr.as_str().unwrap_or_default().to_string();
    interp.get_attr(this, &name)
}

/// `dircmp.__class_getitem__ = classmethod(GenericAlias)`.
fn class_getitem(args: &[ExpandedCallArg]) -> Result<Value> {
    let (cls, rest) = args.split_first().ok_or_else(|| {
        type_error("dircmp.__class_getitem__() takes exactly one argument (0 given)")
    })?;
    if rest.iter().any(|arg| arg.name.is_some()) || rest.len() != 1 {
        return Err(type_error(format!(
            "dircmp.__class_getitem__() takes exactly one argument ({} given)",
            rest.len()
        )));
    }
    let item = &rest[0].value;
    let type_arguments = if matches!(item.kind(), ValueKind::Tuple(_)) {
        item.clone()
    } else {
        Value::tuple(vec![item.clone()])
    };
    Ok(pyrust_builtins::generic_alias::generic_alias(
        cls.value.clone(),
        type_arguments,
    ))
}

/// Wire what the CPython source gets from its imports and class body: the
/// `os`, `stat`, `filterfalse` and `GenericAlias` module globals, and
/// `dircmp.methodmap`, which maps the lazily computed attributes to the
/// `dircmp` phase functions themselves (not to the subclass's overrides).
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    for name in ["os", "stat"] {
        if let Ok(dep) = interp.load_module(name) {
            module_rc.borrow_mut().insert_attr(name.to_string(), dep);
        }
    }
    if let Ok(itertools) = interp.load_module("itertools")
        && let Ok(filterfalse) = interp.get_attr(&itertools, "filterfalse")
    {
        module_rc
            .borrow_mut()
            .insert_attr("filterfalse".to_string(), filterfalse);
    }
    if let Ok(types) = interp.load_module("types")
        && let Ok(alias) = interp.get_attr(&types, "GenericAlias")
    {
        module_rc
            .borrow_mut()
            .insert_attr("GenericAlias".to_string(), alias);
    }
    let class = module_rc.borrow().attrs.get("dircmp").cloned();
    let Some(class) = class else { return };
    let ValueKind::PyClass(class_rc) = class.kind() else {
        return;
    };
    let mut methodmap = PyDict::default();
    for (attr, phase) in [
        ("subdirs", "phase4"),
        ("same_files", "phase3"),
        ("diff_files", "phase3"),
        ("funny_files", "phase3"),
        ("common_dirs", "phase2"),
        ("common_files", "phase2"),
        ("common_funny", "phase2"),
        ("common", "phase1"),
        ("left_only", "phase1"),
        ("right_only", "phase1"),
        ("left_list", "phase0"),
        ("right_list", "phase0"),
    ] {
        let function = class_rc.borrow().attrs.get(phase).cloned();
        if let Some(function) = function {
            methodmap.insert(PyKey::str_from(attr), function);
        }
    }
    class_rc
        .borrow_mut()
        .attrs
        .insert("methodmap".to_string(), Value::dict(methodmap));
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => all_list(),
        "_cache" => Value::dict(PyDict::default()),
        "BUFSIZE" => Value::int(BUFSIZE),
        "DEFAULT_IGNORES" => default_ignores(),
    }

    /// Clear the filecmp cache.
    fn clear_cache(args) -> Result<Value> {
        let _ = args::bind(&SIG_CLEAR_CACHE, args)?;
        clear_cache_impl(_interp)
    }

    /// Compare two files.
    fn cmp(args) -> Result<Value> {
        let mut b = args::bind(&SIG_CMP, args)?;
        let f1 = b.take_or_none(0);
        let f2 = b.take_or_none(1);
        let shallow = b.take_or(2, || Value::bool_(true));
        cmp_impl(_interp, f1, f2, &shallow)
    }

    fn _sig(args) -> Result<Value> {
        let mut b = args::bind(&SIG_SIG, args)?;
        let st = b.take_or_none(0);
        sig_impl(_interp, &st)
    }

    fn _do_cmp(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DO_CMP, args)?;
        let f1 = b.take_or_none(0);
        let f2 = b.take_or_none(1);
        do_cmp_impl(_interp, &f1, &f2)
    }

    /// Compare common files in two directories.
    fn cmpfiles(args) -> Result<Value> {
        let mut b = args::bind(&SIG_CMPFILES, args)?;
        let a = b.take_or_none(0);
        let bb = b.take_or_none(1);
        let common = b.take_or_none(2);
        let shallow = b.take_or(3, || Value::bool_(true));
        cmpfiles_impl(_interp, &a, &bb, &common, &shallow)
    }

    fn _cmp(args) -> Result<Value> {
        let mut b = args::bind(&SIG_PRIVATE_CMP, args)?;
        let a = b.take_or_none(0);
        let bb = b.take_or_none(1);
        let sh = b.take_or_none(2);
        let abs_fn = b.take(3);
        let cmp_fn = b.take(4);
        private_cmp_impl(_interp, a, bb, sh, abs_fn, cmp_fn)
    }

    fn _filter(args) -> Result<Value> {
        let mut b = args::bind(&SIG_FILTER, args)?;
        let flist = b.take_or_none(0);
        let skip = b.take_or_none(1);
        filter_impl(_interp, &flist, &skip)
    }

    fn demo(args) -> Result<Value> {
        let _ = args::bind(&SIG_DEMO, args)?;
        demo_impl(_interp)
    }

    class dircmp {
        constants {
            "__doc__" => Value::string(DIRCMP_DOC),
        }

        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_INIT, args)?;
            let this = b.take_or_none(0);
            let a = b.take_or_none(1);
            let bb = b.take_or_none(2);
            let ignore = b.take_or_none(3);
            let hide = b.take_or_none(4);
            dircmp_init(_interp, &this, a, bb, ignore, hide)?;
            Ok(Value::none())
        }

        fn phase0(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[PHASE0], args)?;
            let this = b.take_or_none(0);
            phase0(_interp, &this)?;
            Ok(Value::none())
        }

        fn phase1(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[PHASE1], args)?;
            let this = b.take_or_none(0);
            phase1(_interp, &this)?;
            Ok(Value::none())
        }

        fn phase2(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[PHASE2], args)?;
            let this = b.take_or_none(0);
            phase2(_interp, &this)?;
            Ok(Value::none())
        }

        fn phase3(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[PHASE3], args)?;
            let this = b.take_or_none(0);
            phase3(_interp, &this)?;
            Ok(Value::none())
        }

        fn phase4(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[PHASE4], args)?;
            let this = b.take_or_none(0);
            phase4(_interp, &this)?;
            Ok(Value::none())
        }

        fn phase4_closure(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[PHASE4_CLOSURE], args)?;
            let this = b.take_or_none(0);
            phase4_closure(_interp, &this)?;
            Ok(Value::none())
        }

        fn report(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[REPORT], args)?;
            let this = b.take_or_none(0);
            report(_interp, &this)?;
            Ok(Value::none())
        }

        fn report_partial_closure(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SELF[REPORT_PARTIAL_CLOSURE], args)?;
            let this = b.take_or_none(0);
            report_partial_closure(_interp, &this)?;
            Ok(Value::none())
        }

        fn report_full_closure(args) -> Result<Value> {
            let mut b = args::bind(&SIG_REPORT_FULL_CLOSURE, args)?;
            let this = b.take_or_none(0);
            report_full_closure(_interp, &this)?;
            Ok(Value::none())
        }

        fn __getattr__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_GETATTR, args)?;
            let this = b.take_or_none(0);
            let attr = b.take_or_none(1);
            dircmp_getattr(_interp, &this, attr)
        }

        #[classmethod]
        fn __class_getitem__(args) -> Result<Value> {
            class_getitem(args)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::interpreter::Interpreter;

    #[test]
    fn dircmp_rejects_shallow_kwarg_like_cpython_3_12() {
        let mut interp = Interpreter::default();
        let src = r#"
import filecmp
try:
    filecmp.dircmp("a", "b", shallow=False)
except TypeError as exc:
    assert "unexpected keyword argument 'shallow'" in str(exc), str(exc)
else:
    raise AssertionError("dircmp() must not accept shallow in 3.12")
"#;
        interp
            .exec_source(src, None, None)
            .expect("dircmp must reject shallow");
    }

    #[test]
    fn dircmp_compares_directories_lazily() {
        let mut interp = Interpreter::default();
        let src = r#"
import filecmp, os, tempfile

with tempfile.TemporaryDirectory() as tmp:
    d1 = os.path.join(tmp, "a")
    d2 = os.path.join(tmp, "b")
    os.mkdir(d1)
    os.mkdir(d2)
    os.mkdir(os.path.join(d1, "sub"))
    os.mkdir(os.path.join(d2, "sub"))
    with open(os.path.join(d1, "f.txt"), "wb") as f:
        f.write(b"hello")
    with open(os.path.join(d2, "f.txt"), "wb") as f:
        f.write(b"world")
    # Same size: only a differing mtime keeps the shallow comparison honest.
    os.utime(os.path.join(d1, "f.txt"), (1_700_000_000, 1_700_000_000))
    os.utime(os.path.join(d2, "f.txt"), (1_700_000_100, 1_700_000_100))
    with open(os.path.join(d1, "sub", "g.txt"), "wb") as f:
        f.write(b"same")
    with open(os.path.join(d2, "sub", "g.txt"), "wb") as f:
        f.write(b"same")
    dc = filecmp.dircmp(d1, d2)
    assert "diff_files" not in vars(dc)
    assert dc.diff_files == ["f.txt"], dc.diff_files
    assert "diff_files" in vars(dc)
    assert dc.subdirs["sub"].same_files == ["g.txt"]
    assert filecmp.cmp(os.path.join(d1, "sub", "g.txt"), os.path.join(d2, "sub", "g.txt"), shallow=False)
"#;
        interp
            .exec_source(src, None, None)
            .expect("dircmp lazy comparison should succeed");
    }
}
