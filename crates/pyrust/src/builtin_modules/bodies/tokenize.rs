// `tokenize` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/tokenize.py` (issues #3204, #3479).
//
// The tokenizer proper is the stateful native lexer in `tokenize/lexer.rs`
// (shared with `ast` and `tabnanny`).  This file is the module surface around
// it, mirroring the CPython source: `TokenInfo` (a real subclass of a
// `collections.namedtuple`), `TokenError`, `StopTokenizing`, `Untokenizer` and
// `untokenize`, `detect_encoding`, `open`, and the lazy `tokenize()` /
// `generate_tokens()` generators.  The `token` constants and functions are
// re-exported the way `from token import *` does.
//
// Like the Python source, every callable looks module-level helpers up through
// the module namespace (`tok_name`, `EXACT_TOKEN_TYPES`, `lookup`,
// `TextIOWrapper`, `detect_encoding`, `Untokenizer`, ...), so patching them
// behaves as in CPython.
//
// Namespace differences that are deliberate: CPython's module also binds `re`,
// `functools`, `itertools`, the regular-expression building blocks of the old
// pure-Python tokenizer (`Whitespace`, `PseudoToken`, `endpats`, ...), `cookie_re`
// / `blank_re` (compiled with `re`), `_tokenize`, `main` and the private
// `_generate_tokens_from_c_tokenizer`.  Binding them would import the
// pure-Python `re` on every `import tokenize` (over 150 ms in a debug build);
// the PEP 263 cookie is matched by a small native scanner instead.
//
// Reference: <https://docs.python.org/3.12/library/tokenize.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::macros::link_class_bases;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValueRef;
use crate::interpreter::{
    ExpandedCallArg, Interpreter, class_is_builtin_exception_subclass, instantiate_exception,
    lookup_exc_class, render_value_repr,
};
use crate::value::{PyClass, PyModule, Value, ValueKind};

#[path = "tokenize/encoding.rs"]
mod encoding;
#[path = "tokenize/lexer.rs"]
pub(crate) mod lexer;
#[path = "tokenize/stream.rs"]
mod stream;
#[path = "tokenize/untokenizer.rs"]
mod untokenizer;

type ClassRc = Rc<RefCell<PyClass>>;

const MODULE_DOC: &str = "Tokenization help for Python programs.\n\n\
tokenize(readline) is a generator that breaks a stream of bytes into\n\
Python tokens.  It decodes the bytes according to PEP-0263 for\n\
determining source file encoding.\n\n\
It accepts a readline-like method which is called repeatedly to get the\n\
next line of input (or b\"\" for EOF).  It generates 5-tuples with these\n\
members:\n\n\
    the token type (see token.py)\n\
    the token (a string)\n\
    the starting (row, column) indices of the token (a 2-tuple of ints)\n\
    the ending (row, column) indices of the token (a 2-tuple of ints)\n\
    the original line (string)\n\n\
It is designed to match the working of the Python tokenizer exactly, except\n\
that it produces COMMENT tokens for comments and gives type OP for all\n\
operators.  Additionally, all token lists start with an ENCODING token\n\
which tells you which encoding was used to decode the bytes stream.\n";

const AUTHOR: &str = "Ka-Ping Yee <ping@lfw.org>";
const CREDITS: &str = "GvR, ESR, Tim Peters, Thomas Wouters, Fred Drake, \
Skip Montanaro, Raymond Hettinger, Trent Nelson, Michael Foord";

/// The names `__all__` adds to `token.__all__`.
const EXTRA_ALL: [&str; 5] = [
    "tokenize",
    "generate_tokens",
    "detect_encoding",
    "untokenize",
    "TokenInfo",
];

// Token types the native code compares against (`Lib/token.py` values).
const TOK_OP: i64 = lexer::TOK_OP;
const TOK_ENCODING: i64 = 67;

// Per-interpreter state (`gil_local!`):
// (a) Why: the lazy generators and `TokenInfo` constructors are native
//     helpers without a module reference, yet they build `TokenInfo`
//     instances and raise `TokenError`.
// (b) Every generation is kept (a re-import after `del sys.modules[...]`
//     creates new classes); the newest one is used, like CPython's module
//     global lookup after a reload.
pyrust_core::gil_local! {
    static TOKEN_INFO_CLASSES: RefCell<Vec<ClassRc>> = const { RefCell::new(Vec::new()) };
    static TOKEN_ERROR_CLASSES: RefCell<Vec<ClassRc>> = const { RefCell::new(Vec::new()) };
}

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

// The `def`s of `Lib/tokenize.py`: CPython reports arity errors for these in
// the interpreter's `initialize_locals` wording, not the macro's.
const SIG_TOKENIZE: Sig = sig("tokenize", &["readline"], 0);
const SIG_GENERATE_TOKENS: Sig = sig("generate_tokens", &["readline"], 0);
const SIG_DETECT_ENCODING: Sig = sig("detect_encoding", &["readline"], 0);
const SIG_OPEN: Sig = sig("open", &["filename"], 0);
const SIG_UNTOKENIZE: Sig = sig("untokenize", &["iterable"], 0);
const SIG_GET_NORMAL_NAME: Sig = sig("_get_normal_name", &["orig_enc"], 0);
const SIG_TOKEN_INFO_REPR: Sig = sig("TokenInfo.__repr__", &["self"], 0);
const SIG_UT_INIT: Sig = sig("Untokenizer.__init__", &["self"], 0);
const SIG_UT_ADD_WHITESPACE: Sig = sig("Untokenizer.add_whitespace", &["self", "start"], 0);
const SIG_UT_ESCAPE_BRACKETS: Sig = sig("Untokenizer.escape_brackets", &["self", "token"], 0);
const SIG_UT_UNTOKENIZE: Sig = sig("Untokenizer.untokenize", &["self", "iterable"], 0);
const SIG_UT_COMPAT: Sig = sig("Untokenizer.compat", &["self", "token", "iterable"], 0);

// ---------------------------------------------------------------------------
// Interpreter plumbing shared by the submodules
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

/// A builtin function (`len`, `open`, ...), the way the source reaches it
/// through the builtins namespace.
fn builtin(interp: &mut Interpreter, name: &'static str, pos: &[Value]) -> Result<Value> {
    call_pos(interp, Value::builtin_function(name), pos)
}

/// A module-level global of `tokenize` (`tok_name`, `lookup`, ...), read from
/// the live module namespace so that patching it takes effect.
fn module_global(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("tokenize")?;
    interp.get_attr(&module, name)
}

fn truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if let Some(text) = value.as_str() {
        return Ok(!text.is_empty());
    }
    interp.truthy_value(value)
}

fn binary(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<Value> {
    interp.eval_binary(a.clone(), op, b.clone())
}

fn compare(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<bool> {
    let result = binary(interp, a, op, b)?;
    truthy(interp, &result)
}

/// `str(value)`.
fn str_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    if let Some(text) = value.as_str() {
        return Ok(text.to_string());
    }
    interp.render_value_as_str(value)
}

/// `repr(value)`.
fn repr_of(interp: &mut Interpreter, value: &Value) -> Result<String> {
    render_value_repr(interp, value)
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn syntax_error(msg: impl Into<String>) -> PyError {
    PyError::named("SyntaxError", msg.into())
}

/// Whether `err` is an instance of the built-in exception `name` (or of a
/// subclass of it).
fn error_is(err: &PyError, name: &str) -> bool {
    use pyrust_core::PyErrorKind;
    match err.kind() {
        PyErrorKind::Named(class, _) => class.as_ref() == name,
        PyErrorKind::KeyError(_) => name == "KeyError",
        PyErrorKind::AttributeError { .. } => name == "AttributeError",
        PyErrorKind::Class(class, _) => class_is_builtin_exception_subclass(class, name),
        PyErrorKind::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => {
                class_is_builtin_exception_subclass(&inst.borrow().class, name)
            }
            ValueKind::PyClass(class) => class_is_builtin_exception_subclass(class, name),
            _ => false,
        },
        _ => false,
    }
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

/// `TokenError(msg, (line, col))`.
fn token_error(msg: String, line: u32, col: u32) -> PyError {
    let args = vec![
        Value::string(msg.as_str()),
        Value::tuple(vec![
            Value::int(i64::from(line)),
            Value::int(i64::from(col)),
        ]),
    ];
    match TOKEN_ERROR_CLASSES.with(|classes| classes.borrow().last().cloned()) {
        Some(class) => PyError::raised(instantiate_exception(class, args)),
        None => PyError::runtime("tokenize is not initialised".to_string()),
    }
}

/// The newest `TokenInfo` class.
fn token_info_class() -> Result<Value> {
    TOKEN_INFO_CLASSES
        .with(|classes| classes.borrow().last().cloned())
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime("tokenize is not initialised".to_string()))
}

// ---------------------------------------------------------------------------
// TokenInfo
// ---------------------------------------------------------------------------

/// `TokenInfo.__repr__`.
fn token_info_repr(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let fields = interp.collect_iterable(this)?;
    let mut items: Vec<Value> = fields;
    let ty = items.first().cloned().unwrap_or_else(Value::none);
    // annotated_type = '%d (%s)' % (self.type, tok_name[self.type])
    let tok_name = module_global(interp, "tok_name")?;
    let name = crate::builtin_modules::operator::getitem_value(interp, tok_name, ty.clone())?;
    let annotated = binary(
        interp,
        &Value::string("%d (%s)"),
        BinaryOp::Mod,
        &Value::tuple(vec![ty, name]),
    )?;
    if let Some(first) = items.first_mut() {
        *first = annotated;
    }
    // 'TokenInfo(type=%s, string=%r, start=%r, end=%r, line=%r)' % self._replace(type=annotated_type)
    binary(
        interp,
        &Value::string("TokenInfo(type=%s, string=%r, start=%r, end=%r, line=%r)"),
        BinaryOp::Mod,
        &Value::tuple(items),
    )
}

/// `TokenInfo.exact_type`.
fn token_info_exact_type(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let ty = interp.get_attr(this, "type")?;
    let is_op = match ty.as_int() {
        Some(n) => n == TOK_OP,
        None => compare(interp, &ty, BinaryOp::Eq, &Value::int(TOK_OP))?,
    };
    if is_op {
        let string = interp.get_attr(this, "string")?;
        let exact = module_global(interp, "EXACT_TOKEN_TYPES")?;
        let contains = binary(interp, &string, BinaryOp::In, &exact)?;
        if truthy(interp, &contains)? {
            return crate::builtin_modules::operator::getitem_value(interp, exact, string);
        }
    }
    Ok(ty)
}

// ---------------------------------------------------------------------------
// Module wiring
// ---------------------------------------------------------------------------

/// Wire what the CPython source gets from its imports and class statements:
/// the `from token import *` names, `__all__`, the `collections.namedtuple`
/// base of `TokenInfo`, and the `codecs` / `io` / `sys` / `collections` /
/// `builtins.open` module globals.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // from token import *; from token import EXACT_TOKEN_TYPES
    let token = interp.load_module("token")?;
    let token_all = interp.get_attr(&token, "__all__")?;
    let names = interp.collect_iterable(&token_all)?;
    let mut exported = Vec::with_capacity(names.len() + EXTRA_ALL.len());
    for name in &names {
        let Some(text) = name.as_str() else { continue };
        let value = interp.get_attr(&token, text)?;
        module.borrow_mut().insert_attr(text.to_string(), value);
        exported.push(name.clone());
    }
    let exact = interp.get_attr(&token, "EXACT_TOKEN_TYPES")?;
    module
        .borrow_mut()
        .insert_attr("EXACT_TOKEN_TYPES".to_string(), exact);
    // __all__ = token.__all__ + ["tokenize", ...]
    exported.extend(EXTRA_ALL.iter().map(Value::string));
    module
        .borrow_mut()
        .insert_attr("__all__".to_string(), Value::list(exported));

    // from builtins import open as _builtin_open; from codecs import lookup,
    // BOM_UTF8; from io import TextIOWrapper; import collections, sys
    let codecs = interp.load_module("codecs")?;
    let io = interp.load_module("io")?;
    let collections = interp.load_module("collections")?;
    let sys = interp.load_module("sys")?;
    let lookup = interp.get_attr(&codecs, "lookup")?;
    let bom = interp.get_attr(&codecs, "BOM_UTF8")?;
    for (name, value) in [
        ("_builtin_open", Value::builtin_function("open")),
        ("lookup", lookup),
        ("BOM_UTF8", bom),
        ("collections", collections.clone()),
        ("sys", sys),
    ] {
        module.borrow_mut().insert_attr(name.to_string(), value);
    }
    // This runtime's `io` has no `TextIOWrapper` class (text files come from
    // the builtin `open`), so the name is only bound when it exists;
    // `open()` falls back to reopening the file in text mode.
    if let Ok(text_io_wrapper) = interp.get_attr(&io, "TextIOWrapper") {
        module
            .borrow_mut()
            .insert_attr("TextIOWrapper".to_string(), text_io_wrapper);
    }

    // class TokenInfo(collections.namedtuple('TokenInfo', 'type string start end line')):
    let namedtuple = interp.get_attr(&collections, "namedtuple")?;
    let base = call_pos(
        interp,
        namedtuple,
        &[
            Value::string("TokenInfo"),
            Value::string("type string start end line"),
        ],
    )?;
    interp.assign_attr(base.clone(), "__module__", Value::string("tokenize"))?;
    let ValueKind::PyClass(base_class) = base.kind() else {
        return Err(PyError::runtime(
            "tokenize: namedtuple() did not return a class".to_string(),
        ));
    };
    let class_value = module.borrow().get_attr_value("TokenInfo");
    let Some(class_value) = class_value else {
        return Err(PyError::runtime(
            "tokenize: TokenInfo is missing".to_string(),
        ));
    };
    let ValueKind::PyClass(class) = class_value.kind() else {
        return Err(PyError::runtime(
            "tokenize: TokenInfo is not a class".to_string(),
        ));
    };
    link_class_bases(class, vec![Rc::clone(base_class)]);
    {
        let mut class = class.borrow_mut();
        // `class TokenInfo(...)` has no docstring.
        class.attrs.insert("__doc__".to_string(), Value::none());
        class.bump_mutation_version();
    }
    TOKEN_INFO_CLASSES.with(|classes| classes.borrow_mut().push(Rc::clone(class)));

    // `class TokenError(Exception): pass` has no docstring either.
    for name in ["TokenError", "StopTokenizing"] {
        let error = module.borrow().get_attr_value(name);
        if let Some(error) = error
            && let ValueKind::PyClass(error) = error.kind()
        {
            error
                .borrow_mut()
                .attrs
                .insert("__doc__".to_string(), Value::none());
            if name == "TokenError" {
                TOKEN_ERROR_CLASSES.with(|classes| classes.borrow_mut().push(Rc::clone(error)));
            }
        }
    }
    Ok(())
}

pyrust_module! {
    exception TokenError;
    exception StopTokenizing;

    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__author__" => Value::string(AUTHOR),
        "__credits__" => Value::string(CREDITS),
        "tabsize" => Value::int(8),
    }

    fn _get_normal_name(args) -> Result<Value> {
        let mut b = args::bind(&SIG_GET_NORMAL_NAME, args)?;
        let orig_enc = b.take_or_none(0);
        encoding::get_normal_name(_interp, &orig_enc)
    }

    fn detect_encoding(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DETECT_ENCODING, args)?;
        let readline = b.take_or_none(0);
        encoding::detect_encoding(_interp, &readline)
    }

    #[py_name = "open"]
    fn open_file(args) -> Result<Value> {
        let mut b = args::bind(&SIG_OPEN, args)?;
        let filename = b.take_or_none(0);
        encoding::open_file(_interp, &filename)
    }

    fn tokenize(args) -> Result<Value> {
        let mut b = args::bind(&SIG_TOKENIZE, args)?;
        let readline = b.take_or_none(0);
        Ok(stream::make_stream(readline, true))
    }

    fn generate_tokens(args) -> Result<Value> {
        let mut b = args::bind(&SIG_GENERATE_TOKENS, args)?;
        let readline = b.take_or_none(0);
        Ok(stream::make_stream(readline, false))
    }

    fn untokenize(args) -> Result<Value> {
        let mut b = args::bind(&SIG_UNTOKENIZE, args)?;
        let iterable = b.take_or_none(0);
        untokenizer::untokenize(_interp, iterable)
    }

    class TokenInfo {
        fn __repr__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_TOKEN_INFO_REPR, args)?;
            let this = b.take_or_none(0);
            token_info_repr(_interp, &this)
        }

        #[getter]
        fn exact_type(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            token_info_exact_type(_interp, self_val.0)
        }
    }

    class Untokenizer {
        fn __init__(args) -> Result<Value> {
            let mut b = args::bind(&SIG_UT_INIT, args)?;
            let this = b.take_or_none(0);
            untokenizer::init(_interp, &this)?;
            Ok(Value::none())
        }

        fn add_whitespace(args) -> Result<Value> {
            let mut b = args::bind(&SIG_UT_ADD_WHITESPACE, args)?;
            let this = b.take_or_none(0);
            let start = b.take_or_none(1);
            untokenizer::add_whitespace(_interp, &this, &start)?;
            Ok(Value::none())
        }

        fn escape_brackets(args) -> Result<Value> {
            let mut b = args::bind(&SIG_UT_ESCAPE_BRACKETS, args)?;
            let _this = b.take_or_none(0);
            let token = b.take_or_none(1);
            untokenizer::escape_brackets(_interp, &token)
        }

        fn untokenize(args) -> Result<Value> {
            let mut b = args::bind(&SIG_UT_UNTOKENIZE, args)?;
            let this = b.take_or_none(0);
            let iterable = b.take_or_none(1);
            untokenizer::untokenize_method(_interp, &this, &iterable)
        }

        fn compat(args) -> Result<Value> {
            let mut b = args::bind(&SIG_UT_COMPAT, args)?;
            let this = b.take_or_none(0);
            let token = b.take_or_none(1);
            let iterable = b.take_or_none(2);
            untokenizer::compat(_interp, &this, &token, &iterable)?;
            Ok(Value::none())
        }
    }
}
