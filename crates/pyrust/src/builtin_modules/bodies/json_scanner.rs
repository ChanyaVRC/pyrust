// `json.scanner` submodule — 100% native Rust implementation of Python 3.12's
// `json.scanner` module (issues #3251, #3413).
//
// `make_scanner` / `c_make_scanner` are the `_json.Scanner` type itself, as in
// CPython: calling it with a decoder captures `strict`, `object_hook`,
// `object_pairs_hook`, `parse_float`, `parse_int` and `parse_constant` and
// returns a callable that decodes natively (`bodies/json/scan.rs`, a port of
// `_json.c`'s `scan_once_unicode`).  `py_make_scanner` reproduces the
// pure-Python scanner, which dispatches through the decoder's
// `parse_object` / `parse_array` / `parse_string` hooks.
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/json/scanner.py>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::json::args;
use crate::builtin_modules::json::scan::{self, Doc, PyScanGuard, ScanConfig, ScanError};
use crate::builtin_modules::json::{docs, support};
use crate::builtin_modules::macros::native_state::{has_native_state, with_native_state};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyClass, PyModule, Value, ValueKind};

const NUMBER_RE_PATTERN: &str = r"(-?(?:0|[1-9]\d*))(\.\d+)?([eE][-+]?\d+)?";

type ClassRc = Rc<RefCell<PyClass>>;

/// State of a `_json.Scanner` (`make_scanner(context)`).
pub(crate) struct CScannerState {
    cfg: ScanConfig,
}

/// State of a `py_make_scanner(context)` scanner: the context attributes
/// `py_make_scanner` binds as closure variables.
struct PyScannerState {
    parse_object: Value,
    parse_array: Value,
    parse_string: Value,
    strict: Value,
    parse_float: Value,
    parse_int: Value,
    parse_constant: Value,
    object_hook: Value,
    object_pairs_hook: Value,
    memo: Value,
    /// Active `scan_once` calls: only the outermost one runs the
    /// `finally: memo.clear()` (nested values re-enter through the same
    /// object, which stands in for the inner `_scan_once` closure).
    depth: std::cell::Cell<usize>,
}

/// The hidden scanner classes, kept out of the module namespace like
/// CPython (where `_json.Scanner` lives in `_json` and the pure-Python
/// scanner is a closure).
#[derive(Clone)]
struct ScannerClasses {
    py_scanner: ClassRc,
}

// (a) Why `thread_local!`-style storage: `PyClass` is `!Send`; the hidden
//     `py_make_scanner` class must be reachable after it is removed from the
//     module namespace.
// (b) Multi-interpreter / multi-threaded impact: interpreters on one GIL group
//     share the most recently imported generation; existing scanners keep the
//     class they were created with.
pyrust_core::gil_local! {
    static CLASSES: RefCell<Option<ScannerClasses>> = const { RefCell::new(None) };
}

fn module_class(module: &PyModule, name: &str) -> Result<ClassRc> {
    match module.attrs.get(name).map(Value::kind) {
        Some(ValueKind::PyClass(class)) => Ok(Rc::clone(class)),
        _ => Err(PyError::runtime(format!(
            "json.scanner: missing class {name}"
        ))),
    }
}

/// Whether `value` is a native `_json.Scanner`.
pub(crate) fn is_c_scanner(value: &Value) -> bool {
    has_native_state::<CScannerState>(value)
}

/// The context a native `_json.Scanner` captured.
pub(crate) fn c_scanner_config(value: &Value) -> Result<ScanConfig> {
    with_native_state::<CScannerState, _>(value, "_json.Scanner", |st| Ok(st.cfg.clone()))
}

/// `scanner(string, idx)` for a native scanner, returning either the
/// `(value, end)` code point result or the `StopIteration` index.
pub(crate) fn c_scan(
    interp: &mut Interpreter,
    cfg: &ScanConfig,
    string: &Value,
    idx: i64,
) -> std::result::Result<(Value, i64), ScanError> {
    let Some(doc) = Doc::new(string) else {
        return Err(ScanError::Raised(args::type_error(format!(
            "first argument must be a string, not {}",
            support::type_name(string)
        ))));
    };
    if idx < 0 {
        return Err(ScanError::Raised(PyError::named(
            "ValueError",
            "idx cannot be negative".to_string(),
        )));
    }
    if idx as usize >= doc.char_len() {
        return Err(ScanError::Stop(idx as usize));
    }
    let start = doc.byte_of(idx as usize);
    match scan::scan_once(interp, cfg, &doc, start) {
        Ok((value, end)) => Ok((value, doc.char_of(end) as i64)),
        Err(ScanError::Stop(byte)) => Err(ScanError::Stop(doc.char_of(byte))),
        Err(err) => Err(err),
    }
}

fn c_scanner_init(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let Some(this) = call_args.first().map(|a| a.value.clone()) else {
        return Err(args::type_error("descriptor '__init__' needs an argument"));
    };
    let mut slots = args::parse_c("make_scanner", &["context"], 1, &call_args[1..])?;
    let context = slots[0].take().unwrap_or_else(Value::none);
    let cfg = ScanConfig::from_context(interp, &context)?;
    crate::builtin_modules::macros::native_state::set_native_state(&this, CScannerState { cfg })?;
    Ok(Value::none())
}

fn c_scanner_call(interp: &mut Interpreter, call_args: &[ExpandedCallArg]) -> Result<Value> {
    let Some(this) = call_args.first().map(|a| a.value.clone()) else {
        return Err(args::type_error("descriptor '__call__' needs an argument"));
    };
    let mut slots = args::parse_c("scan_once", &["string", "idx"], 2, &call_args[1..])?;
    let string = slots[0].take().unwrap_or_else(Value::none);
    let idx_value = slots[1].take().unwrap_or_else(Value::none);
    let idx = support::ssize_index(interp, &idx_value)?;
    let cfg = c_scanner_config(&this)?;
    match c_scan(interp, &cfg, &string, idx) {
        Ok((value, end)) => Ok(Value::tuple(vec![value, Value::int(end)])),
        Err(ScanError::Stop(pos)) => Err(support::stop_iteration(interp, Value::int(pos as i64))),
        Err(ScanError::Raised(err)) => Err(err),
    }
}

// ── `py_make_scanner` ────────────────────────────────────────────────────────

const SIG_PY_SCAN_ONCE: args::Sig = args::Sig {
    qualname: "py_make_scanner.<locals>.scan_once",
    params: &["string", "idx"],
    defaults: 0,
    kwonly: &[],
    var_kw: false,
};

/// `NUMBER_RE.match(string, idx)` at byte offset `start`: the byte offsets
/// ending the integer, fraction and exponent groups.
fn match_number(b: &[u8], start: usize) -> Option<(usize, usize, usize)> {
    let n = b.len();
    let mut i = start;
    if i < n && b[i] == b'-' {
        i += 1;
    }
    if i >= n {
        return None;
    }
    if b[i] == b'0' {
        i += 1;
    } else if (b'1'..=b'9').contains(&b[i]) {
        i += 1;
        while i < n && b[i].is_ascii_digit() {
            i += 1;
        }
    } else {
        return None;
    }
    let int_end = i;
    if i + 1 < n && b[i] == b'.' && b[i + 1].is_ascii_digit() {
        i += 2;
        while i < n && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    let frac_end = i;
    if i < n && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < n && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < n && b[j].is_ascii_digit() {
            i = j + 1;
            while i < n && b[i].is_ascii_digit() {
                i += 1;
            }
        }
    }
    Some((int_end, frac_end, i))
}

fn py_scanner_state<R>(this: &Value, f: impl FnOnce(&PyScannerState) -> Result<R>) -> Result<R> {
    with_native_state::<PyScannerState, _>(this, "scan_once", f)
}

/// `_scan_once(string, idx)` of `py_make_scanner`.
fn py_scan_once(interp: &mut Interpreter, this: &Value, string: &Value, idx: i64) -> Result<Value> {
    let stop = |interp: &mut Interpreter| support::stop_iteration(interp, Value::int(idx));
    let Some(doc) = Doc::indexed(string) else {
        return Err(args::type_error(
            "cannot use a string pattern on a bytes-like object",
        ));
    };
    if idx < 0 || idx as usize >= doc.char_len() {
        return Err(stop(interp));
    }
    let start = doc.byte_of(idx as usize);
    let b = doc.text().as_bytes();
    let rest = &b[start..];
    let next = Value::int(idx + 1);
    let get =
        |pick: fn(&PyScannerState) -> &Value| py_scanner_state(this, |st| Ok(pick(st).clone()));
    match b[start] {
        b'"' => {
            let parse_string = get(|st| &st.parse_string)?;
            let strict = get(|st| &st.strict)?;
            return support::call(interp, parse_string, &[string.clone(), next, strict]);
        }
        b'{' => {
            let parse_object = get(|st| &st.parse_object)?;
            let strict = get(|st| &st.strict)?;
            let object_hook = get(|st| &st.object_hook)?;
            let object_pairs_hook = get(|st| &st.object_pairs_hook)?;
            let memo = get(|st| &st.memo)?;
            let s_and_end = Value::tuple(vec![string.clone(), next]);
            return support::call(
                interp,
                parse_object,
                &[
                    s_and_end,
                    strict,
                    this.clone(),
                    object_hook,
                    object_pairs_hook,
                    memo,
                ],
            );
        }
        b'[' => {
            let parse_array = get(|st| &st.parse_array)?;
            let s_and_end = Value::tuple(vec![string.clone(), next]);
            return support::call(interp, parse_array, &[s_and_end, this.clone()]);
        }
        b'n' if rest.starts_with(b"null") => {
            return Ok(Value::tuple(vec![Value::none(), Value::int(idx + 4)]));
        }
        b't' if rest.starts_with(b"true") => {
            return Ok(Value::tuple(vec![Value::bool_(true), Value::int(idx + 4)]));
        }
        b'f' if rest.starts_with(b"false") => {
            return Ok(Value::tuple(vec![Value::bool_(false), Value::int(idx + 5)]));
        }
        _ => {}
    }
    if let Some((int_end, frac_end, exp_end)) = match_number(b, start) {
        let text = doc.text();
        let (parser, numstr) = if frac_end > int_end || exp_end > frac_end {
            (get(|st| &st.parse_float)?, &text[start..exp_end])
        } else {
            (get(|st| &st.parse_int)?, &text[start..int_end])
        };
        let res = support::call(interp, parser, &[Value::string(numstr)])?;
        return Ok(Value::tuple(vec![
            res,
            Value::int(idx + (exp_end - start) as i64),
        ]));
    }
    for (token, first) in [("NaN", b'N'), ("Infinity", b'I'), ("-Infinity", b'-')] {
        if b[start] == first && rest.starts_with(token.as_bytes()) {
            let parse_constant = get(|st| &st.parse_constant)?;
            let res = support::call(interp, parse_constant, &[Value::string(token)])?;
            return Ok(Value::tuple(vec![
                res,
                Value::int(idx + token.len() as i64),
            ]));
        }
    }
    Err(stop(interp))
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let re_mod = interp.load_module("re")?;
    let compile_fn = interp.get_attr(&re_mod, "compile")?;
    let flags = interp.get_attr(&re_mod, "VERBOSE")?.as_int().unwrap_or(0)
        | interp.get_attr(&re_mod, "MULTILINE")?.as_int().unwrap_or(0)
        | interp.get_attr(&re_mod, "DOTALL")?.as_int().unwrap_or(0);
    let number_re = support::call(
        interp,
        compile_fn,
        &[Value::string(NUMBER_RE_PATTERN), Value::int(flags)],
    )?;
    let mut m = module.borrow_mut();
    let c_scanner = module_class(&m, "Scanner")?;
    let py_scanner = module_class(&m, "_PyScanner")?;
    m.attrs.shift_remove("Scanner");
    m.attrs.shift_remove("_PyScanner");
    {
        let mut class = c_scanner.borrow_mut();
        class
            .attrs
            .insert("__module__".to_string(), Value::string("_json"));
        class.error_name = Some("_json.Scanner");
        class.bump_mutation_version();
    }
    CLASSES.with(|slot| *slot.borrow_mut() = Some(ScannerClasses { py_scanner }));
    let scanner_type = Value::py_class(c_scanner);
    m.insert_attr("re".to_string(), re_mod);
    m.insert_attr("c_make_scanner".to_string(), scanner_type.clone());
    m.insert_attr("make_scanner".to_string(), scanner_type);
    m.insert_attr("NUMBER_RE".to_string(), number_re);
    for name in [
        "re",
        "c_make_scanner",
        "__all__",
        "NUMBER_RE",
        "py_make_scanner",
        "make_scanner",
    ] {
        if let Some(value) = m.attrs.shift_remove(name) {
            m.attrs.insert(name.to_string(), value);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::JSON_SCANNER_MODULE_DOC),
        "__package__" => Value::string("json"),
        "__all__" => Value::list(vec![Value::string("make_scanner")]),
    }

    // `_json.Scanner` (re-homed and bound as `make_scanner` by `post_load_init`).
    #[native_state(CScannerState)]
    class Scanner {
        constants {
            "__doc__" => Value::string("JSON scanner object"),
        }

        fn __init__(args) -> Result<Value> {
            c_scanner_init(_interp, args)
        }

        fn __call__(args) -> Result<Value> {
            c_scanner_call(_interp, args)
        }
    }

    // The closure `py_make_scanner` returns (hidden by `post_load_init`).
    #[native_state(PyScannerState)]
    class _PyScanner {
        fn __call__(args) -> Result<Value> {
            let Some(this) = args.first().map(|a| a.value.clone()) else {
                return Err(args::type_error("descriptor '__call__' needs an argument"));
            };
            let mut b = args::bind(&SIG_PY_SCAN_ONCE, &args[1..])?;
            let string = b.take_or_none(0);
            let idx_value = b.take_or_none(1);
            let idx = support::ssize_index(_interp, &idx_value)?;
            py_scanner_state(&this, |st| {
                st.depth.set(st.depth.get() + 1);
                Ok(())
            })?;
            let session = PyScanGuard::enter();
            let result = py_scan_once(_interp, &this, &string, idx);
            drop(session);
            let (outermost, memo) = py_scanner_state(&this, |st| {
                st.depth.set(st.depth.get() - 1);
                Ok((st.depth.get() == 0, st.memo.clone()))
            })?;
            if outermost {
                // `finally: memo.clear()`
                support::call_method(_interp, &memo, "clear", &[])?;
            }
            result
        }
    }

    fn py_make_scanner(context: crate::interpreter::builtin_args::PyValue) -> Result<Value> {
        let context = context.0;
        let mut read = |name: &str| _interp.get_attr(&context, name);
        let state = PyScannerState {
            parse_object: read("parse_object")?,
            parse_array: read("parse_array")?,
            parse_string: read("parse_string")?,
            strict: read("strict")?,
            parse_float: read("parse_float")?,
            parse_int: read("parse_int")?,
            parse_constant: read("parse_constant")?,
            object_hook: read("object_hook")?,
            object_pairs_hook: read("object_pairs_hook")?,
            memo: read("memo")?,
            depth: std::cell::Cell::new(0),
        };
        let class = CLASSES
            .with(|slot| slot.borrow().as_ref().map(|c| Rc::clone(&c.py_scanner)))
            .ok_or_else(|| PyError::runtime("json.scanner is not initialised".to_string()))?;
        let instance = Value::py_instance(Rc::new(RefCell::new(crate::value::PyInstance {
            class,
            attrs: crate::value::InstanceAttrs::new(),
        })));
        crate::builtin_modules::macros::native_state::set_native_state(&instance, state)?;
        Ok(instance)
    }
}
