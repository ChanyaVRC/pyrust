// `fileinput` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/fileinput.py` (issues #3230, #3485).
//
// The module mirrors the CPython source: the module-level `_state` and the
// stateful helpers (`input`, `close`, `nextfile`, `filename`, `lineno`,
// `filelineno`, `fileno`, `isfirstline`, `isstdin`), the `FileInput` class,
// `hook_compressed`, `hook_encoded` and `_test`.
//
// Like the Python source, every step goes back through `self`: `FileInput`
// keeps its state in ordinary instance attributes (`_files`, `_file`,
// `_filename`, ... and the bound `_readline` that hides the class method
// while a file is open), and calls `self.nextfile()`, `self.lineno()`,
// `self._readline()` and `self.close()` through attribute lookups, so
// subclasses overriding any of them behave exactly as in CPython.  The
// module's dependencies (`os`, `sys`, `io`) are module attributes that are
// looked up at call time, so they can be patched like the source's globals.
//
// Reference: <https://docs.python.org/3.12/library/fileinput.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value, ValueKind};

#[path = "fileinput/docs.rs"]
mod docs;

type ClassRc = Rc<RefCell<PyClass>>;

const fn sig(
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

const KWONLY_OPEN: &[&str] = &["mode", "openhook", "encoding", "errors"];

// The `def`s of `Lib/fileinput.py`: CPython reports arity errors for these in
// the interpreter's `initialize_locals` wording, not the macro's.
const SIG_INPUT: Sig = sig("input", &["files", "inplace", "backup"], 3, KWONLY_OPEN);
const SIG_INIT: Sig = sig(
    "FileInput.__init__",
    &["self", "files", "inplace", "backup"],
    3,
    KWONLY_OPEN,
);
const SIG_FN0: [Sig; 8] = [
    sig("close", &[], 0, &[]),
    sig("nextfile", &[], 0, &[]),
    sig("filename", &[], 0, &[]),
    sig("lineno", &[], 0, &[]),
    sig("filelineno", &[], 0, &[]),
    sig("fileno", &[], 0, &[]),
    sig("isfirstline", &[], 0, &[]),
    sig("isstdin", &[], 0, &[]),
];
const SIG_METHOD0: [Sig; 13] = [
    sig("FileInput.__del__", &["self"], 0, &[]),
    sig("FileInput.close", &["self"], 0, &[]),
    sig("FileInput.__enter__", &["self"], 0, &[]),
    sig("FileInput.__iter__", &["self"], 0, &[]),
    sig("FileInput.__next__", &["self"], 0, &[]),
    sig("FileInput.nextfile", &["self"], 0, &[]),
    sig("FileInput.readline", &["self"], 0, &[]),
    sig("FileInput._readline", &["self"], 0, &[]),
    sig("FileInput.filename", &["self"], 0, &[]),
    sig("FileInput.lineno", &["self"], 0, &[]),
    sig("FileInput.filelineno", &["self"], 0, &[]),
    sig("FileInput.fileno", &["self"], 0, &[]),
    sig("FileInput.isfirstline", &["self"], 0, &[]),
];
const SIG_ISSTDIN: Sig = sig("FileInput.isstdin", &["self"], 0, &[]);
const SIG_EXIT: Sig = sig(
    "FileInput.__exit__",
    &["self", "type", "value", "traceback"],
    0,
    &[],
);
const SIG_HOOK_COMPRESSED: Sig = sig(
    "hook_compressed",
    &["filename", "mode"],
    0,
    &["encoding", "errors"],
);
const SIG_HOOK_ENCODED: Sig = sig("hook_encoded", &["encoding", "errors"], 1, &[]);
const SIG_OPENHOOK: Sig = sig(
    "hook_encoded.<locals>.openhook",
    &["filename", "mode"],
    0,
    &[],
);
const SIG_TEST: Sig = sig("_test", &[], 0, &[]);

// Indexes into `SIG_METHOD0`.
const M_DEL: usize = 0;
const M_CLOSE: usize = 1;
const M_ENTER: usize = 2;
const M_ITER: usize = 3;
const M_NEXT: usize = 4;
const M_NEXTFILE: usize = 5;
const M_READLINE: usize = 6;
const M_READLINE_IMPL: usize = 7;
const M_FILENAME: usize = 8;
const M_LINENO: usize = 9;
const M_FILELINENO: usize = 10;
const M_FILENO: usize = 11;
const M_ISFIRSTLINE: usize = 12;

// Indexes into `SIG_FN0`.
const F_CLOSE: usize = 0;
const F_NEXTFILE: usize = 1;
const F_FILENAME: usize = 2;
const F_LINENO: usize = 3;
const F_FILELINENO: usize = 4;
const F_FILENO: usize = 5;
const F_ISFIRSTLINE: usize = 6;
const F_ISSTDIN: usize = 7;

// Per-interpreter state (`gil_local!`):
// (a) Why: `hook_encoded` returns a callable that carries its `encoding` and
//     `errors`; the CPython source builds a closure, and native code has none,
//     so the module keeps a hidden `_OpenHook` class (detached from the module
//     namespace so `dir(fileinput)` matches CPython) and builds instances of
//     it.
// (b) Every import generation is kept; the newest one is used, like the
//     module-global lookup of a re-imported module.
pyrust_core::gil_local! {
    static OPEN_HOOK_CLASSES: RefCell<Vec<ClassRc>> = const { RefCell::new(Vec::new()) };
}

fn all_list() -> Value {
    Value::list(
        [
            "input",
            "close",
            "nextfile",
            "filename",
            "lineno",
            "filelineno",
            "fileno",
            "isfirstline",
            "isstdin",
            "FileInput",
            "hook_compressed",
            "hook_encoded",
        ]
        .into_iter()
        .map(Value::string)
        .collect(),
    )
}

// ---------------------------------------------------------------------------
// Interpreter plumbing
// ---------------------------------------------------------------------------

fn call_kw(
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

fn call_pos(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    call_kw(interp, callee, pos, &[])
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

/// A builtin function (`open`, `callable`, `isinstance`, ...), the way the
/// source reaches it through the builtins namespace.
fn builtin(interp: &mut Interpreter, name: &'static str, pos: &[Value]) -> Result<Value> {
    call_pos(interp, Value::builtin_function(name), pos)
}

/// A module-level dependency (`os`, `sys`, `io`): the module attribute when
/// present (CPython resolves `import os` as a module global), otherwise the
/// imported module itself.
fn dep(interp: &mut Interpreter, name: &str) -> Result<Value> {
    if let Ok(module) = interp.load_module("fileinput")
        && let Ok(value) = interp.get_attr(&module, name)
    {
        return Ok(value);
    }
    interp.load_module(name)
}

/// `import <name>` executed inside a function body.
fn import(interp: &mut Interpreter, name: &str) -> Result<Value> {
    interp.load_module(name)
}

fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    interp.truthy_value(value)
}

fn binary(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<Value> {
    interp.eval_binary(a.clone(), op, b.clone())
}

/// `a == b`.
fn values_equal(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    let result = binary(interp, a, BinaryOp::Eq, b)?;
    truthy(interp, &result)
}

/// `item in container`.
fn contains(interp: &mut Interpreter, container: &Value, item: &Value) -> Result<bool> {
    let result = interp.eval_in(container.clone(), item.clone())?;
    truthy(interp, &result)
}

/// `fmt % value` with CPython's `%` formatting semantics.
fn percent_format(interp: &mut Interpreter, fmt: &str, value: Value) -> Result<Value> {
    interp.eval_binary(Value::string(fmt), BinaryOp::Mod, value)
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

/// `io.UnsupportedOperation` derives from both `OSError` and `ValueError`; the
/// runtime reports it by name, so the generic class-chain test misses it.
fn is_unsupported_operation(err: &PyError) -> bool {
    err.class_name_is("io.UnsupportedOperation") || err.class_name_is("UnsupportedOperation")
}

/// `except OSError`.
fn is_os_error(err: &PyError) -> bool {
    err.class_name_is("OSError") || is_unsupported_operation(err)
}

/// `except ValueError`.
fn is_value_error(err: &PyError) -> bool {
    err.class_name_is("ValueError") || is_unsupported_operation(err)
}

fn is_attribute_error(err: &PyError) -> bool {
    err.class_name_is("AttributeError")
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

/// `isinstance(value, cls)`.
fn is_instance(interp: &mut Interpreter, value: &Value, cls: &Value) -> Result<bool> {
    let result = builtin(interp, "isinstance", &[value.clone(), cls.clone()])?;
    truthy(interp, &result)
}

fn is_str_instance(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    let class = crate::interpreter::primitive_class_by_name("str")
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime("missing str class".into()))?;
    is_instance(interp, value, &class)
}

/// `"b" in mode` (and `"b" not in mode` through negation).
fn mode_is_binary(interp: &mut Interpreter, mode: &Value) -> Result<bool> {
    contains(interp, mode, &Value::string("b"))
}

fn attr_is_true(interp: &mut Interpreter, this: &Value, name: &str) -> Result<bool> {
    let value = interp.get_attr(this, name)?;
    truthy(interp, &value)
}

fn set_attr(interp: &mut Interpreter, this: &Value, name: &str, value: Value) -> Result<()> {
    interp.assign_attr(this.clone(), name, value)
}

/// `bytes`/`str` empty line for the mode (`b''` for binary, `''` for text).
fn empty_line(binary_mode: bool) -> Value {
    if binary_mode {
        Value::bytes(Vec::new())
    } else {
        Value::string("")
    }
}

// ---------------------------------------------------------------------------
// Module-level state helpers
// ---------------------------------------------------------------------------

/// The module global `_state`.
fn state(interp: &mut Interpreter) -> Result<Value> {
    let module = interp.load_module("fileinput")?;
    interp.get_attr(&module, "_state")
}

/// `global _state; _state = value`.
fn set_state(interp: &mut Interpreter, value: Value) -> Result<()> {
    let module = interp.load_module("fileinput")?;
    interp.assign_attr(module, "_state", value)
}

/// `if not _state: raise RuntimeError("no active input()")`.
fn active_state(interp: &mut Interpreter) -> Result<Value> {
    let current = state(interp)?;
    if truthy(interp, &current)? {
        Ok(current)
    } else {
        Err(PyError::named("RuntimeError", "no active input()"))
    }
}

/// The module function `name()` that forwards to `_state.<name>()`.
fn forward_to_state(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let current = active_state(interp)?;
    call_method(interp, &current, name, &[])
}

/// `input(files=None, inplace=False, backup="", *, mode="r", openhook=None,
/// encoding=None, errors=None)`.
fn input_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_INPUT, args)?;
    let files = b.take_or_none(0);
    let inplace = b.take_or(1, || Value::bool_(false));
    let backup = b.take_or(2, || Value::string(""));
    let mode = b.take_or(3, || Value::string("r"));
    let openhook = b.take_or_none(4);
    let encoding = b.take_or_none(5);
    let errors = b.take_or_none(6);
    let current = state(interp)?;
    if truthy(interp, &current)? && attr_is_true(interp, &current, "_file")? {
        return Err(PyError::named("RuntimeError", "input() already active"));
    }
    let module = interp.load_module("fileinput")?;
    let class = interp.get_attr(&module, "FileInput")?;
    let created = call_kw(
        interp,
        class,
        &[files, inplace, backup],
        &[
            ("mode", mode),
            ("openhook", openhook),
            ("encoding", encoding),
            ("errors", errors),
        ],
    )?;
    set_state(interp, created.clone())?;
    Ok(created)
}

/// `close()`.
fn close_module(interp: &mut Interpreter) -> Result<Value> {
    let current = state(interp)?;
    set_state(interp, Value::none())?;
    if truthy(interp, &current)? {
        call_method(interp, &current, "close", &[])?;
    }
    Ok(Value::none())
}

// ---------------------------------------------------------------------------
// FileInput
// ---------------------------------------------------------------------------

/// `FileInput.__init__`.
fn file_input_init(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_INIT, args)?;
    let this = b.take_or_none(0);
    let files = b.take_or_none(1);
    let inplace = b.take_or(2, || Value::bool_(false));
    let backup = b.take_or(3, || Value::string(""));
    let mode = b.take_or(4, || Value::string("r"));
    let openhook = b.take_or_none(5);
    let encoding = b.take_or_none(6);
    let errors = b.take_or_none(7);

    // `__del__` closes the input; make sure the finalizer queue is running.
    interp.arm_finalizers();

    let files = normalize_files(interp, files)?;
    set_attr(interp, &this, "_files", files)?;
    set_attr(interp, &this, "_inplace", inplace.clone())?;
    set_attr(interp, &this, "_backup", backup)?;
    set_attr(interp, &this, "_savestdout", Value::none())?;
    set_attr(interp, &this, "_output", Value::none())?;
    set_attr(interp, &this, "_filename", Value::none())?;
    set_attr(interp, &this, "_startlineno", Value::int(0))?;
    set_attr(interp, &this, "_filelineno", Value::int(0))?;
    set_attr(interp, &this, "_file", Value::none())?;
    set_attr(interp, &this, "_isstdin", Value::bool_(false))?;
    set_attr(interp, &this, "_backupfilename", Value::none())?;
    set_attr(interp, &this, "_encoding", encoding.clone())?;
    set_attr(interp, &this, "_errors", errors)?;

    // We can not use io.text_encoding() here because old openhook doesn't
    // take encoding parameter.
    let sys = dep(interp, "sys")?;
    let flags = interp.get_attr(&sys, "flags")?;
    let warn_default = interp.get_attr(&flags, "warn_default_encoding")?;
    if truthy(interp, &warn_default)?
        && !mode_is_binary(interp, &mode)?
        && encoding.is_none()
        && openhook.is_none()
    {
        let warnings = import(interp, "warnings")?;
        let warn = interp.get_attr(&warnings, "warn")?;
        let category = crate::interpreter::lookup_exc_class("EncodingWarning")
            .map(Value::py_class)
            .unwrap_or_else(Value::none);
        call_pos(
            interp,
            warn,
            &[
                Value::string("'encoding' argument not specified."),
                category,
                Value::int(2),
            ],
        )?;
    }

    // restrict mode argument to reading modes
    let is_r = values_equal(interp, &mode, &Value::string("r"))?;
    let is_rb = is_r || values_equal(interp, &mode, &Value::string("rb"))?;
    if !is_rb {
        return Err(value_error("FileInput opening mode must be 'r' or 'rb'"));
    }
    set_attr(interp, &this, "_mode", mode.clone())?;
    let write_mode = call_method(
        interp,
        &mode,
        "replace",
        &[Value::string("r"), Value::string("w")],
    )?;
    set_attr(interp, &this, "_write_mode", write_mode)?;
    if truthy(interp, &openhook)? {
        if truthy(interp, &inplace)? {
            return Err(value_error(
                "FileInput cannot use an opening hook in inplace mode",
            ));
        }
        let callable = builtin(interp, "callable", std::slice::from_ref(&openhook))?;
        if !truthy(interp, &callable)? {
            return Err(value_error("FileInput openhook must be callable"));
        }
    }
    set_attr(interp, &this, "_openhook", openhook)?;
    Ok(Value::none())
}

/// The `files` normalisation of `FileInput.__init__`.
fn normalize_files(interp: &mut Interpreter, files: Value) -> Result<Value> {
    if is_str_instance(interp, &files)? {
        return Ok(Value::tuple(vec![files]));
    }
    let os = dep(interp, "os")?;
    let pathlike = interp.get_attr(&os, "PathLike")?;
    if is_instance(interp, &files, &pathlike)? {
        let path = call_method(interp, &os, "fspath", std::slice::from_ref(&files))?;
        return Ok(Value::tuple(vec![path]));
    }
    let files = if files.is_none() {
        let sys = dep(interp, "sys")?;
        let argv = interp.get_attr(&sys, "argv")?;
        let items = interp.collect_iterable(&argv)?;
        Value::list(items.into_iter().skip(1).collect())
    } else {
        files
    };
    if !truthy(interp, &files)? {
        return Ok(Value::tuple(vec![Value::string("-")]));
    }
    let items = interp.collect_iterable(&files)?;
    Ok(Value::tuple(items))
}

/// `FileInput.close`.
fn file_input_close(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let result = call_method(interp, this, "nextfile", &[]);
    set_attr(interp, this, "_files", Value::tuple(Vec::new()))?;
    result.map(|_| Value::none())
}

/// `FileInput.__next__` (`stop = true`) and `FileInput.readline`
/// (`stop = false`): the shared read loop.
fn read_loop(interp: &mut Interpreter, this: &Value, stop: bool) -> Result<Value> {
    loop {
        let line = call_method(interp, this, "_readline", &[])?;
        if truthy(interp, &line)? {
            let count = interp.get_attr(this, "_filelineno")?;
            let next = binary(interp, &count, BinaryOp::Add, &Value::int(1))?;
            set_attr(interp, this, "_filelineno", next)?;
            return Ok(line);
        }
        if !attr_is_true(interp, this, "_file")? {
            if stop {
                return Err(PyError::named("StopIteration", String::new()));
            }
            return Ok(line);
        }
        call_method(interp, this, "nextfile", &[])?;
        // repeat with next file
    }
}

/// `FileInput.nextfile`.
fn file_input_nextfile(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let savestdout = interp.get_attr(this, "_savestdout")?;
    set_attr(interp, this, "_savestdout", Value::none())?;
    if truthy(interp, &savestdout)? {
        let sys = dep(interp, "sys")?;
        interp.assign_attr(sys, "stdout", savestdout)?;
    }

    let output = interp.get_attr(this, "_output")?;
    set_attr(interp, this, "_output", Value::none())?;
    let first = (|| -> Result<()> {
        if truthy(interp, &output)? {
            call_method(interp, &output, "close", &[])?;
        }
        Ok(())
    })();
    // `finally:` — an error raised here replaces the one above.
    let second = nextfile_close_file(interp, this);
    second.and(first).map(|()| Value::none())
}

/// The first `finally:` block of `FileInput.nextfile`.
fn nextfile_close_file(interp: &mut Interpreter, this: &Value) -> Result<()> {
    let file = interp.get_attr(this, "_file")?;
    set_attr(interp, this, "_file", Value::none())?;
    // `del self._readline  # restore FileInput._readline`
    let _ = try_swallow(
        || interp.delete_attr(this.clone(), "_readline"),
        is_attribute_error,
    )?;
    let first = (|| -> Result<()> {
        if truthy(interp, &file)? && !attr_is_true(interp, this, "_isstdin")? {
            call_method(interp, &file, "close", &[])?;
        }
        Ok(())
    })();
    // `finally:` — remove the backup file, reset `_isstdin`.
    let second = nextfile_cleanup(interp, this);
    second.and(first)
}

/// The innermost `finally:` block of `FileInput.nextfile`.
fn nextfile_cleanup(interp: &mut Interpreter, this: &Value) -> Result<()> {
    let backupfilename = interp.get_attr(this, "_backupfilename")?;
    set_attr(interp, this, "_backupfilename", Value::none())?;
    if truthy(interp, &backupfilename)? && !attr_is_true(interp, this, "_backup")? {
        let os = dep(interp, "os")?;
        let _ = try_swallow(
            || call_method(interp, &os, "unlink", std::slice::from_ref(&backupfilename)),
            is_os_error,
        )?;
    }
    set_attr(interp, this, "_isstdin", Value::bool_(false))
}

/// `FileInput._readline`.
fn file_input_readline_impl(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let mode = interp.get_attr(this, "_mode")?;
    let files = interp.get_attr(this, "_files")?;
    if !truthy(interp, &files)? {
        let binary_mode = mode_is_binary(interp, &mode)?;
        return Ok(empty_line(binary_mode));
    }
    let mut items = interp.collect_iterable(&files)?;
    let first = items.remove(0);
    set_attr(interp, this, "_filename", first)?;
    set_attr(interp, this, "_files", Value::tuple(items))?;
    let lineno = call_method(interp, this, "lineno", &[])?;
    set_attr(interp, this, "_startlineno", lineno)?;
    set_attr(interp, this, "_filelineno", Value::int(0))?;
    set_attr(interp, this, "_file", Value::none())?;
    set_attr(interp, this, "_isstdin", Value::bool_(false))?;
    set_attr(interp, this, "_backupfilename", Value::int(0))?;

    // EncodingWarning is emitted in __init__() already
    let binary_mode = mode_is_binary(interp, &mode)?;
    let encoding = if binary_mode {
        Value::none()
    } else {
        let configured = interp.get_attr(this, "_encoding")?;
        if truthy(interp, &configured)? {
            configured
        } else {
            Value::string("locale")
        }
    };
    let errors = interp.get_attr(this, "_errors")?;
    let filename = interp.get_attr(this, "_filename")?;

    if values_equal(interp, &filename, &Value::string("-"))? {
        set_attr(interp, this, "_filename", Value::string("<stdin>"))?;
        let sys = dep(interp, "sys")?;
        let stdin = interp.get_attr(&sys, "stdin")?;
        let file = if binary_mode {
            // `getattr(sys.stdin, 'buffer', sys.stdin)`
            match try_swallow(|| interp.get_attr(&stdin, "buffer"), is_attribute_error)? {
                Ok(buffer) => buffer,
                Err(_) => stdin,
            }
        } else {
            stdin
        };
        set_attr(interp, this, "_file", file)?;
        set_attr(interp, this, "_isstdin", Value::bool_(true))?;
    } else if attr_is_true(interp, this, "_inplace")? {
        open_inplace(interp, this, &filename, &mode, &encoding, &errors)?;
    } else {
        // This may raise OSError
        let hook = interp.get_attr(this, "_openhook")?;
        let file = if truthy(interp, &hook)? {
            // Custom hooks made previous to Python 3.10 didn't have
            // encoding argument
            let configured = interp.get_attr(this, "_encoding")?;
            if configured.is_none() {
                call_pos(interp, hook, &[filename, mode])?
            } else {
                call_kw(
                    interp,
                    hook,
                    &[filename, mode],
                    &[("encoding", configured), ("errors", errors)],
                )?
            }
        } else {
            let open = Value::builtin_function("open");
            call_kw(
                interp,
                open,
                &[filename, mode],
                &[("encoding", encoding), ("errors", errors)],
            )?
        };
        set_attr(interp, this, "_file", file)?;
    }
    // `self._readline = self._file.readline  # hide FileInput._readline`
    let file = interp.get_attr(this, "_file")?;
    let readline = interp.get_attr(&file, "readline")?;
    set_attr(interp, this, "_readline", readline)?;
    call_method(interp, this, "_readline", &[])
}

/// The `if self._inplace:` branch of `FileInput._readline`.
fn open_inplace(
    interp: &mut Interpreter,
    this: &Value,
    filename: &Value,
    mode: &Value,
    encoding: &Value,
    errors: &Value,
) -> Result<()> {
    let os = dep(interp, "os")?;
    let path = call_method(interp, &os, "fspath", std::slice::from_ref(filename))?;
    let backup = interp.get_attr(this, "_backup")?;
    let suffix = if truthy(interp, &backup)? {
        backup
    } else {
        Value::string(".bak")
    };
    let backupfilename = binary(interp, &path, BinaryOp::Add, &suffix)?;
    set_attr(interp, this, "_backupfilename", backupfilename.clone())?;
    let _ = try_swallow(
        || call_method(interp, &os, "unlink", std::slice::from_ref(&backupfilename)),
        is_os_error,
    )?;
    // The next few lines may raise OSError
    call_method(
        interp,
        &os,
        "rename",
        &[filename.clone(), backupfilename.clone()],
    )?;
    let open = Value::builtin_function("open");
    let file = call_kw(
        interp,
        open.clone(),
        &[backupfilename, mode.clone()],
        &[("encoding", encoding.clone()), ("errors", errors.clone())],
    )?;
    set_attr(interp, this, "_file", file.clone())?;
    let write_mode = interp.get_attr(this, "_write_mode")?;
    let perm = try_swallow(
        || -> Result<Value> {
            let fileno = call_method(interp, &file, "fileno", &[])?;
            let stat = call_method(interp, &os, "fstat", &[fileno])?;
            interp.get_attr(&stat, "st_mode")
        },
        is_os_error,
    )?;
    let output = match perm {
        Err(_) => call_kw(
            interp,
            open,
            &[filename.clone(), write_mode],
            &[("encoding", encoding.clone()), ("errors", errors.clone())],
        )?,
        Ok(perm) => {
            let creat = interp.get_attr(&os, "O_CREAT")?;
            let wronly = interp.get_attr(&os, "O_WRONLY")?;
            let trunc = interp.get_attr(&os, "O_TRUNC")?;
            let mut flags = binary(interp, &creat, BinaryOp::BitOr, &wronly)?;
            flags = binary(interp, &flags, BinaryOp::BitOr, &trunc)?;
            if let Ok(o_binary) =
                try_swallow(|| interp.get_attr(&os, "O_BINARY"), is_attribute_error)?
            {
                flags = binary(interp, &flags, BinaryOp::BitOr, &o_binary)?;
            }
            let fd = call_method(
                interp,
                &os,
                "open",
                &[filename.clone(), flags, perm.clone()],
            )?;
            let fdopen = interp.get_attr(&os, "fdopen")?;
            let output = call_kw(
                interp,
                fdopen,
                &[fd, write_mode],
                &[("encoding", encoding.clone()), ("errors", errors.clone())],
            )?;
            let _ = try_swallow(
                || call_method(interp, &os, "chmod", &[filename.clone(), perm]),
                is_os_error,
            )?;
            output
        }
    };
    set_attr(interp, this, "_output", output.clone())?;
    let sys = dep(interp, "sys")?;
    let savestdout = interp.get_attr(&sys, "stdout")?;
    set_attr(interp, this, "_savestdout", savestdout)?;
    interp.assign_attr(sys, "stdout", output)?;
    Ok(())
}

/// `FileInput.fileno`.
fn file_input_fileno(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    if attr_is_true(interp, this, "_file")? {
        let file = interp.get_attr(this, "_file")?;
        let result = try_swallow(|| call_method(interp, &file, "fileno", &[]), is_value_error)?;
        Ok(result.unwrap_or_else(|_| Value::int(-1)))
    } else {
        Ok(Value::int(-1))
    }
}

/// `__class_getitem__ = classmethod(GenericAlias)`.
fn class_getitem(args: &[ExpandedCallArg]) -> Result<Value> {
    let (cls, rest) = args.split_first().ok_or_else(|| {
        type_error("FileInput.__class_getitem__() takes exactly one argument (0 given)")
    })?;
    if rest.iter().any(|arg| arg.name.is_some()) || rest.len() != 1 {
        return Err(type_error(format!(
            "FileInput.__class_getitem__() takes exactly one argument ({} given)",
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

// ---------------------------------------------------------------------------
// hook_compressed / hook_encoded / _test
// ---------------------------------------------------------------------------

/// `hook_compressed(filename, mode, *, encoding=None, errors=None)`.
fn hook_compressed_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_HOOK_COMPRESSED, args)?;
    let filename = b.take_or_none(0);
    let mode = b.take_or_none(1);
    let mut encoding = b.take_or_none(2);
    let errors = b.take_or_none(3);
    // EncodingWarning is emitted in FileInput() already.
    if encoding.is_none() && !mode_is_binary(interp, &mode)? {
        encoding = Value::string("locale");
    }
    let os = dep(interp, "os")?;
    let path = interp.get_attr(&os, "path")?;
    let split = call_method(interp, &path, "splitext", std::slice::from_ref(&filename))?;
    let parts = interp.collect_iterable(&split)?;
    let ext = parts
        .get(1)
        .cloned()
        .ok_or_else(|| value_error("not enough values to unpack"))?;
    let is_gz = values_equal(interp, &ext, &Value::string(".gz"))?;
    let is_bz2 = !is_gz && values_equal(interp, &ext, &Value::string(".bz2"))?;
    if !is_gz && !is_bz2 {
        return call_kw(
            interp,
            Value::builtin_function("open"),
            &[filename, mode],
            &[("encoding", encoding), ("errors", errors)],
        );
    }
    let binary_mode = mode_is_binary(interp, &mode)?;
    let io = dep(interp, "io")?;
    let wrapper = try_swallow(|| interp.get_attr(&io, "TextIOWrapper"), is_attribute_error)?.ok();
    if !binary_mode && wrapper.is_none() {
        // This runtime's `io` has no `TextIOWrapper` to wrap the binary
        // stream in; `gzip.open` / `bz2.open` build the same text stream.
        let text_mode = if contains(interp, &mode, &Value::string("t"))? {
            mode
        } else {
            binary(interp, &mode, BinaryOp::Add, &Value::string("t"))?
        };
        let (module, name) = if is_gz {
            ("gzip", "open")
        } else {
            ("bz2", "open")
        };
        let module = import(interp, module)?;
        let opener = interp.get_attr(&module, name)?;
        return call_kw(
            interp,
            opener,
            &[filename, text_mode],
            &[("encoding", encoding), ("errors", errors)],
        );
    }
    let stream = if is_gz {
        let gzip = import(interp, "gzip")?;
        call_method(interp, &gzip, "open", &[filename, mode])?
    } else {
        let bz2 = import(interp, "bz2")?;
        call_method(interp, &bz2, "BZ2File", &[filename, mode])?
    };
    // gzip and bz2 are binary mode by default.
    match wrapper {
        Some(wrapper) if !binary_mode => call_kw(
            interp,
            wrapper,
            &[stream],
            &[("encoding", encoding), ("errors", errors)],
        ),
        _ => Ok(stream),
    }
}

fn open_hook_class() -> Result<ClassRc> {
    OPEN_HOOK_CLASSES
        .with(|classes| classes.borrow().last().cloned())
        .ok_or_else(|| PyError::runtime("fileinput: hook class is not registered".into()))
}

/// `hook_encoded(encoding, errors=None)`.
fn hook_encoded_impl(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let mut b = args::bind(&SIG_HOOK_ENCODED, args)?;
    let encoding = b.take_or_none(0);
    let errors = b.take_or_none(1);
    let class = open_hook_class()?;
    let hook = Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })));
    set_attr(interp, &hook, "_encoding", encoding)?;
    set_attr(interp, &hook, "_errors", errors)?;
    Ok(hook)
}

/// `hook_encoded.<locals>.openhook(filename, mode)`.
fn open_hook_call(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let (this, rest) = args
        .split_first()
        .ok_or_else(|| type_error("openhook() takes 2 positional arguments but 0 were given"))?;
    let this = this.value.clone();
    let mut b = args::bind(&SIG_OPENHOOK, rest)?;
    let filename = b.take_or_none(0);
    let mode = b.take_or_none(1);
    let encoding = interp.get_attr(&this, "_encoding")?;
    let errors = interp.get_attr(&this, "_errors")?;
    call_kw(
        interp,
        Value::builtin_function("open"),
        &[filename, mode],
        &[("encoding", encoding), ("errors", errors)],
    )
}

/// `_test()`.
fn run_test(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<Value> {
    let _ = args::bind(&SIG_TEST, args)?;
    let getopt = import(interp, "getopt")?;
    let mut inplace = Value::bool_(false);
    let mut backup = Value::bool_(false);
    let sys = dep(interp, "sys")?;
    let argv = interp.get_attr(&sys, "argv")?;
    let argv: Vec<Value> = interp
        .collect_iterable(&argv)?
        .into_iter()
        .skip(1)
        .collect();
    let parsed = call_method(
        interp,
        &getopt,
        "getopt",
        &[Value::list(argv), Value::string("ib:")],
    )?;
    let parsed = interp.collect_iterable(&parsed)?;
    let [opts, rest] = <[Value; 2]>::try_from(parsed)
        .map_err(|_| value_error("not enough values to unpack (expected 2)"))?;
    for pair in interp.collect_iterable(&opts)? {
        let pair = interp.collect_iterable(&pair)?;
        let [o, a] = <[Value; 2]>::try_from(pair)
            .map_err(|_| value_error("not enough values to unpack (expected 2)"))?;
        if values_equal(interp, &o, &Value::string("-i"))? {
            inplace = Value::bool_(true);
        }
        if values_equal(interp, &o, &Value::string("-b"))? {
            backup = a;
        }
    }
    let module = interp.load_module("fileinput")?;
    let input_fn = interp.get_attr(&module, "input")?;
    let stream = call_kw(
        interp,
        input_fn,
        &[rest],
        &[("inplace", inplace), ("backup", backup)],
    )?;
    let iterator = builtin(interp, "iter", &[stream])?;
    loop {
        let next = builtin(interp, "next", std::slice::from_ref(&iterator));
        let line = match next {
            Ok(line) => line,
            Err(err) if err.class_name_is("StopIteration") => break,
            Err(err) => return Err(err),
        };
        let mut text = interp_str(interp, line)?;
        if text.ends_with('\n') {
            text.pop();
        }
        if text.ends_with('\r') {
            text.pop();
        }
        let lineno = module_call(interp, &module, "lineno")?;
        let filename = module_call(interp, &module, "filename")?;
        let filelineno = module_call(interp, &module, "filelineno")?;
        let first = module_call(interp, &module, "isfirstline")?;
        let marker = if truthy(interp, &first)? { "*" } else { "" };
        let formatted = percent_format(
            interp,
            "%d: %s[%d]%s %s",
            Value::tuple(vec![
                lineno,
                filename,
                filelineno,
                Value::string(marker),
                Value::string(text),
            ]),
        )?;
        builtin(interp, "print", &[formatted])?;
    }
    let lineno = module_call(interp, &module, "lineno")?;
    let filename = module_call(interp, &module, "filename")?;
    let filelineno = module_call(interp, &module, "filelineno")?;
    let formatted = percent_format(
        interp,
        "%d: %s[%d]",
        Value::tuple(vec![lineno, filename, filelineno]),
    )?;
    builtin(interp, "print", &[formatted])?;
    Ok(Value::none())
}

fn module_call(interp: &mut Interpreter, module: &Value, name: &str) -> Result<Value> {
    let function = interp.get_attr(module, name)?;
    call_pos(interp, function, &[])
}

/// `str(value)`.
fn interp_str(interp: &mut Interpreter, value: Value) -> Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }
    let text = percent_format(interp, "%s", Value::tuple(vec![value]))?;
    Ok(text.as_str().unwrap_or_default().to_string())
}

// ---------------------------------------------------------------------------
// Module wiring
// ---------------------------------------------------------------------------

/// Wire what the CPython source gets from `import io`, `import sys, os` and
/// `from types import GenericAlias`, and detach the hidden `_OpenHook` class
/// that stands in for the closure `hook_encoded` returns.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    for name in ["io", "sys", "os"] {
        if let Ok(dep) = interp.load_module(name) {
            module_rc.borrow_mut().insert_attr(name.to_string(), dep);
        }
    }
    if let Ok(types) = interp.load_module("types")
        && let Ok(alias) = interp.get_attr(&types, "GenericAlias")
    {
        module_rc
            .borrow_mut()
            .insert_attr("GenericAlias".to_string(), alias);
    }
    let hook = module_rc.borrow_mut().attrs.shift_remove("_OpenHook");
    if let Some(hook) = hook
        && let ValueKind::PyClass(class) = hook.kind()
    {
        OPEN_HOOK_CLASSES.with(|classes| classes.borrow_mut().push(class.clone()));
    }
    // The module namespace of the source: `__doc__`, the imports, `__all__`,
    // `_state`, then the functions and the class in definition order.
    let mut module = module_rc.borrow_mut();
    for name in [
        "__doc__",
        "io",
        "sys",
        "os",
        "GenericAlias",
        "__all__",
        "_state",
        "input",
        "close",
        "nextfile",
        "filename",
        "lineno",
        "filelineno",
        "fileno",
        "isfirstline",
        "isstdin",
        "FileInput",
        "hook_compressed",
        "hook_encoded",
        "_test",
    ] {
        if let Some(value) = module.attrs.shift_remove(name) {
            module.attrs.insert(name.to_string(), value);
        }
    }
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "__all__" => all_list(),
        "_state" => Value::none(),
    }

    /// Return an instance of the FileInput class, which can be iterated.
    fn input(args) -> Result<Value> {
        input_impl(_interp, args)
    }

    /// Close the sequence.
    fn close(args) -> Result<Value> {
        let _ = args::bind(&SIG_FN0[F_CLOSE], args)?;
        close_module(_interp)
    }

    fn nextfile(args) -> Result<Value> {
        let _ = args::bind(&SIG_FN0[F_NEXTFILE], args)?;
        forward_to_state(_interp, "nextfile")
    }

    fn filename(args) -> Result<Value> {
        let _ = args::bind(&SIG_FN0[F_FILENAME], args)?;
        forward_to_state(_interp, "filename")
    }

    fn lineno(args) -> Result<Value> {
        let _ = args::bind(&SIG_FN0[F_LINENO], args)?;
        forward_to_state(_interp, "lineno")
    }

    fn filelineno(args) -> Result<Value> {
        let _ = args::bind(&SIG_FN0[F_FILELINENO], args)?;
        forward_to_state(_interp, "filelineno")
    }

    fn fileno(args) -> Result<Value> {
        let _ = args::bind(&SIG_FN0[F_FILENO], args)?;
        forward_to_state(_interp, "fileno")
    }

    fn isfirstline(args) -> Result<Value> {
        let _ = args::bind(&SIG_FN0[F_ISFIRSTLINE], args)?;
        forward_to_state(_interp, "isfirstline")
    }

    fn isstdin(args) -> Result<Value> {
        let _ = args::bind(&SIG_FN0[F_ISSTDIN], args)?;
        forward_to_state(_interp, "isstdin")
    }

    fn hook_compressed(args) -> Result<Value> {
        hook_compressed_impl(_interp, args)
    }

    fn hook_encoded(args) -> Result<Value> {
        hook_encoded_impl(_interp, args)
    }

    fn _test(args) -> Result<Value> {
        run_test(_interp, args)
    }

    class FileInput {
        constants {
            "__doc__" => Value::string(docs::CLASS_DOC),
        }

        fn __init__(args) -> Result<Value> {
            file_input_init(_interp, args)
        }

        fn __del__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_DEL], args)?;
            let this = b.take_or_none(0);
            call_method(_interp, &this, "close", &[])?;
            Ok(Value::none())
        }

        fn close(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_CLOSE], args)?;
            let this = b.take_or_none(0);
            file_input_close(_interp, &this)
        }

        fn __enter__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_ENTER], args)?;
            Ok(b.take_or_none(0))
        }

        fn __exit__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_EXIT, args)?;
            let this = b.take_or_none(0);
            call_method(_interp, &this, "close", &[])?;
            Ok(Value::none())
        }

        fn __iter__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_ITER], args)?;
            Ok(b.take_or_none(0))
        }

        fn __next__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_NEXT], args)?;
            let this = b.take_or_none(0);
            read_loop(_interp, &this, true)
        }

        fn nextfile(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_NEXTFILE], args)?;
            let this = b.take_or_none(0);
            file_input_nextfile(_interp, &this)
        }

        fn readline(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_READLINE], args)?;
            let this = b.take_or_none(0);
            read_loop(_interp, &this, false)
        }

        fn _readline(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_READLINE_IMPL], args)?;
            let this = b.take_or_none(0);
            file_input_readline_impl(_interp, &this)
        }

        fn filename(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_FILENAME], args)?;
            let this = b.take_or_none(0);
            _interp.get_attr(&this, "_filename")
        }

        fn lineno(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_LINENO], args)?;
            let this = b.take_or_none(0);
            let start = _interp.get_attr(&this, "_startlineno")?;
            let current = _interp.get_attr(&this, "_filelineno")?;
            binary(_interp, &start, BinaryOp::Add, &current)
        }

        fn filelineno(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_FILELINENO], args)?;
            let this = b.take_or_none(0);
            _interp.get_attr(&this, "_filelineno")
        }

        fn fileno(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_FILENO], args)?;
            let this = b.take_or_none(0);
            file_input_fileno(_interp, &this)
        }

        fn isfirstline(args) -> Result<Value> {
            let mut b = args::bind(&SIG_METHOD0[M_ISFIRSTLINE], args)?;
            let this = b.take_or_none(0);
            let current = _interp.get_attr(&this, "_filelineno")?;
            let first = values_equal(_interp, &current, &Value::int(1))?;
            Ok(Value::bool_(first))
        }

        fn isstdin(args) -> Result<Value> {
            let mut b = args::bind(&SIG_ISSTDIN, args)?;
            let this = b.take_or_none(0);
            _interp.get_attr(&this, "_isstdin")
        }

        #[classmethod]
        fn __class_getitem__(args) -> Result<Value> {
            let _ = _interp;
            class_getitem(args)
        }
    }

    /// The callable `hook_encoded` returns (the `openhook` closure of the
    /// source); hidden from the module namespace in `prepare_module`.
    class _OpenHook {
        fn __call__(args) -> Result<Value> {
            open_hook_call(_interp, args)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::interpreter::Interpreter;

    #[test]
    fn iterates_files_and_tracks_line_numbers() {
        let mut interp = Interpreter::default();
        let src = r#"
import fileinput, os, tempfile

with tempfile.TemporaryDirectory() as tmp:
    a = os.path.join(tmp, "a.txt")
    b = os.path.join(tmp, "b.txt")
    with open(a, "w", encoding="utf-8", newline="\n") as fp:
        fp.write("x\ny\n")
    with open(b, "w", encoding="utf-8", newline="\n") as fp:
        fp.write("z")
    rows = []
    with fileinput.FileInput([a, b], encoding="utf-8") as fi:
        for line in fi:
            rows.append((fi.lineno(), fi.filelineno(), fi.isfirstline(), line))
    assert rows == [(1, 1, True, "x\n"), (2, 2, False, "y\n"), (3, 1, True, "z")], rows
    assert fileinput._state is None
"#;
        interp
            .exec_source(src, None, None)
            .expect("FileInput should iterate over several files");
    }

    #[test]
    fn inplace_restores_stdout_and_removes_backup() {
        let mut interp = Interpreter::default();
        let src = r#"
import fileinput, os, sys, tempfile

with tempfile.TemporaryDirectory() as tmp:
    p = os.path.join(tmp, "f.txt")
    with open(p, "w", encoding="utf-8", newline="\n") as fp:
        fp.write("a\nb\n")
    real = sys.stdout
    with fileinput.FileInput([p], inplace=True, encoding="utf-8") as fi:
        for line in fi:
            sys.stdout.write(line.upper())
    assert sys.stdout is real
    with open(p, encoding="utf-8", newline="") as fp:
        assert fp.read().replace("\r\n", "\n") == "A\nB\n"
    assert not os.path.exists(p + ".bak")
"#;
        interp
            .exec_source(src, None, None)
            .expect("inplace filtering should rewrite the file and restore stdout");
    }

    #[test]
    fn module_helpers_require_an_active_input() {
        let mut interp = Interpreter::default();
        let src = r#"
import fileinput

try:
    fileinput.lineno()
except RuntimeError as e:
    assert str(e) == "no active input()", str(e)
else:
    raise AssertionError("expected RuntimeError")
assert fileinput._state is None
"#;
        interp
            .exec_source(src, None, None)
            .expect("helpers should refuse to run without input()");
    }
}
