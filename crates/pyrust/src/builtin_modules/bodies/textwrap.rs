// `textwrap` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/textwrap.py` (issues #2786, #3488).
//
// The module mirrors the CPython source: the `TextWrapper` class with its
// twelve instance attributes and its class-level `unicode_whitespace_trans`,
// `wordsep_re`, `wordsep_simple_re` and `sentence_end_re`, the `wrap` / `fill`
// / `shorten` / `dedent` / `indent` convenience functions and `__all__`.
//
// Like the Python source, every step goes back through `self`: `wrap` calls
// `_split_chunks`, `fill` calls `wrap`, `_split_chunks` calls
// `_munge_whitespace` and `_split`, `_wrap_chunks` calls `_handle_long_word`,
// and every option is an ordinary attribute read at the point the source reads
// it, so subclasses (`argparse.HelpFormatter`'s wrappers, ...) overriding any
// method or mutating any option behave exactly as in CPython.  The module
// level functions look `TextWrapper` up in the module namespace at call time.
//
// The compiled regexes CPython exposes (`TextWrapper.wordsep_re`, ...) are
// created with the real `re` module at import so they stay patchable; when
// they are still the stock objects the splitting and sentence-ending tests run
// as native scanners (`textwrap/split.rs`) instead of the pure-Python engine.
//
// Reference: <https://docs.python.org/3.12/library/textwrap.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::builtin_modules::json::args::{self, Sig};
use crate::builtin_modules::operator::{delitem_value, getitem_value, setitem_value};
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{FromValue, PyBytesLike};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, builtin_data_backing, make_slice_value, value_type_name_str,
};
use crate::value::{PyDict, PyKey, PyModule, Value};

#[path = "textwrap/docs.rs"]
mod docs;
#[path = "textwrap/patterns.rs"]
mod patterns;
#[path = "textwrap/split.rs"]
mod split;

const fn sig(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: false,
    }
}

const fn sig_kw(qualname: &'static str, params: &'static [&'static str], defaults: usize) -> Sig {
    Sig {
        qualname,
        params,
        defaults,
        kwonly: &[],
        var_kw: true,
    }
}

// The `def`s of `Lib/textwrap.py`: CPython reports arity errors for these in
// the interpreter's `initialize_locals` wording, not the macro's.
const SIG_INIT: Sig = Sig {
    qualname: "TextWrapper.__init__",
    params: &[
        "self",
        "width",
        "initial_indent",
        "subsequent_indent",
        "expand_tabs",
        "replace_whitespace",
        "fix_sentence_endings",
        "break_long_words",
        "drop_whitespace",
        "break_on_hyphens",
        "tabsize",
    ],
    defaults: 10,
    kwonly: &["max_lines", "placeholder"],
    var_kw: false,
};
const SIG_MUNGE: Sig = sig("TextWrapper._munge_whitespace", &["self", "text"], 0);
const SIG_SPLIT: Sig = sig("TextWrapper._split", &["self", "text"], 0);
const SIG_FIX_SENTENCE_ENDINGS: Sig =
    sig("TextWrapper._fix_sentence_endings", &["self", "chunks"], 0);
const SIG_HANDLE_LONG_WORD: Sig = sig(
    "TextWrapper._handle_long_word",
    &["self", "reversed_chunks", "cur_line", "cur_len", "width"],
    0,
);
const SIG_WRAP_CHUNKS: Sig = sig("TextWrapper._wrap_chunks", &["self", "chunks"], 0);
const SIG_SPLIT_CHUNKS: Sig = sig("TextWrapper._split_chunks", &["self", "text"], 0);
const SIG_WRAP_METHOD: Sig = sig("TextWrapper.wrap", &["self", "text"], 0);
const SIG_FILL_METHOD: Sig = sig("TextWrapper.fill", &["self", "text"], 0);
const SIG_WRAP: Sig = sig_kw("wrap", &["text", "width"], 1);
const SIG_FILL: Sig = sig_kw("fill", &["text", "width"], 1);
const SIG_SHORTEN: Sig = sig_kw("shorten", &["text", "width"], 0);
const SIG_DEDENT: Sig = sig("dedent", &["text"], 0);
const SIG_INDENT: Sig = sig("indent", &["text", "prefix", "predicate"], 1);

/// `_whitespace`.
const WHITESPACE: &str = "\t\n\x0b\x0c\r ";

/// Namespace order of the CPython module (`import re`, `__all__`, ...).
const NAMESPACE_ORDER: &[&str] = &[
    "__doc__",
    "re",
    "__all__",
    "_whitespace",
    "TextWrapper",
    "wrap",
    "fill",
    "shorten",
    "_whitespace_only_re",
    "_leading_whitespace_re",
    "dedent",
    "indent",
];

fn all_list() -> Value {
    Value::list(
        ["TextWrapper", "wrap", "fill", "dedent", "indent", "shorten"]
            .into_iter()
            .map(Value::string)
            .collect(),
    )
}

/// `dict.fromkeys(map(ord, _whitespace), ord(' '))`.
fn whitespace_translation() -> Value {
    let mut table = PyDict::default();
    for c in WHITESPACE.chars() {
        table.insert(PyKey::Int(c as i64), Value::int(' ' as i64));
    }
    Value::dict(table)
}

// Per-interpreter state (`gil_local!`):
// (a) Why: the stock compiled regexes are identified by object identity so
//     `_split` / `_fix_sentence_endings` can run the native scanners only
//     while `TextWrapper.wordsep_re` & co. still are CPython's own patterns.
// (b) Every generation is kept (a re-import after `del sys.modules[...]`
//     compiles new ones); a patched attribute is simply not in the list.
pyrust_core::gil_local! {
    static STOCK_REGEXES: RefCell<Vec<[Value; 3]>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy)]
enum Stock {
    WordSep = 0,
    WordSepSimple = 1,
    SentenceEnd = 2,
}

fn is_stock(value: &Value, which: Stock) -> bool {
    STOCK_REGEXES.with(|all| {
        all.borrow()
            .iter()
            .any(|set| set[which as usize].is_identical_to(value))
    })
}

// ---------------------------------------------------------------------------
// Interpreter plumbing
// ---------------------------------------------------------------------------

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

fn call_with(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(String, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    expanded.extend(kw.iter().map(|(name, value)| ExpandedCallArg {
        name: Some(name.clone()),
        value: value.clone(),
    }));
    interp.call_function_expanded(callee, &expanded)
}

fn call_pos(interp: &mut Interpreter, callee: Value, pos: &[Value]) -> Result<Value> {
    call_with(interp, callee, pos, &[])
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

fn attr(interp: &mut Interpreter, this: &Value, name: &str) -> Result<Value> {
    interp.get_attr(this, name)
}

fn truthy_attr(interp: &mut Interpreter, this: &Value, name: &str) -> Result<bool> {
    let value = interp.get_attr(this, name)?;
    interp.truthy_value(&value)
}

/// A module global of `textwrap`, resolved at call time so reassigning it on
/// the module is observed like in CPython.
fn module_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("textwrap")?;
    interp.get_attr(&module, name)
}

/// The `str` value behind a `str` or `str` subclass instance.
fn plain_str(value: &Value) -> Option<Value> {
    if value.is_str() {
        return Some(value.clone());
    }
    builtin_data_backing(value).filter(Value::is_str)
}

/// `len(value)`.
fn len_of(interp: &mut Interpreter, value: &Value) -> Result<i64> {
    if value.is_str() {
        return Ok(value.as_str().map_or(0, |s| s.chars().count()) as i64);
    }
    if let Some(n) = value.list_len() {
        return Ok(n as i64);
    }
    let n = call_pos(
        interp,
        Value::builtin_function("len"),
        std::slice::from_ref(value),
    )?;
    Ok(n.as_int().unwrap_or(0))
}

/// `a <op> b` as a boolean.
fn cmp(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<bool> {
    if let (Some(x), Some(y)) = (a.as_int(), b.as_int()) {
        let fast = match op {
            BinaryOp::Lt => Some(x < y),
            BinaryOp::Le => Some(x <= y),
            BinaryOp::Gt => Some(x > y),
            BinaryOp::Ge => Some(x >= y),
            BinaryOp::Eq => Some(x == y),
            _ => None,
        };
        if let Some(result) = fast {
            return Ok(result);
        }
    }
    let result = interp.eval_binary(a.clone(), op, b.clone())?;
    interp.truthy_value(&result)
}

fn cmp_int(interp: &mut Interpreter, a: i64, op: BinaryOp, b: &Value) -> Result<bool> {
    cmp(interp, &Value::int(a), op, b)
}

fn arith(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<Value> {
    if let (Some(x), Some(y)) = (a.as_int(), b.as_int()) {
        let fast = match op {
            BinaryOp::Add => x.checked_add(y),
            BinaryOp::Sub => x.checked_sub(y),
            _ => None,
        };
        if let Some(result) = fast {
            return Ok(Value::int(result));
        }
    }
    interp.eval_binary(a.clone(), op, b.clone())
}

/// `value.strip()` has no characters left (`value.strip() == ''`).
fn strip_is_empty(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if let Some(text) = plain_str(value) {
        return Ok(text.as_str().is_some_and(|s| {
            s.chars()
                .all(|c| pyrust_builtins::string::is_python_space_cp(c as u32))
        }));
    }
    let stripped = call_method(interp, value, "strip", &[])?;
    cmp(interp, &stripped, BinaryOp::Eq, &Value::string(""))
}

/// `bool(value.strip())`.
fn strip_truthy(interp: &mut Interpreter, value: &Value) -> Result<bool> {
    if let Some(text) = plain_str(value) {
        return Ok(!text.as_str().is_some_and(|s| {
            s.chars()
                .all(|c| pyrust_builtins::string::is_python_space_cp(c as u32))
        }));
    }
    let stripped = call_method(interp, value, "strip", &[])?;
    interp.truthy_value(&stripped)
}

/// `''.join(items)`.
fn join_values(interp: &mut Interpreter, items: &[Value]) -> Result<Value> {
    if items.iter().all(Value::is_str) {
        let mut out = String::new();
        for item in items {
            out.push_str(item.as_str().unwrap_or_default());
        }
        return Ok(Value::string(out));
    }
    call_method(
        interp,
        &Value::string(""),
        "join",
        &[Value::list(items.to_vec())],
    )
}

/// `sep.join(iterable)` for an arbitrary iterable argument.
fn join_iterable(interp: &mut Interpreter, sep: &str, iterable: Value) -> Result<Value> {
    call_method(interp, &Value::string(sep), "join", &[iterable])
}

// ---------------------------------------------------------------------------
// Sequence helpers: the chunk stacks are Python lists in `_wrap_chunks`, but a
// `list` subclass or other mutable sequence works through the generic paths.
// ---------------------------------------------------------------------------

fn seq_len(interp: &mut Interpreter, seq: &Value) -> Result<i64> {
    len_of(interp, seq)
}

fn seq_getitem(interp: &mut Interpreter, seq: &Value, index: i64) -> Result<Value> {
    if let Some(found) = seq.list_with(|items| {
        let idx = if index < 0 {
            index + items.len() as i64
        } else {
            index
        };
        usize::try_from(idx)
            .ok()
            .and_then(|i| items.get(i).cloned())
    }) && let Some(item) = found
    {
        return Ok(item);
    }
    getitem_value(interp, seq.clone(), Value::int(index))
}

fn seq_last(interp: &mut Interpreter, seq: &Value) -> Result<Value> {
    seq_getitem(interp, seq, -1)
}

fn seq_del_last(interp: &mut Interpreter, seq: &Value) -> Result<()> {
    if seq.list_with(|items| !items.is_empty()) == Some(true) {
        seq.list_with_mut(|items| items.pop());
        return Ok(());
    }
    delitem_value(interp, seq.clone(), Value::int(-1))
}

fn seq_set_last(interp: &mut Interpreter, seq: &Value, item: Value) -> Result<()> {
    if seq.list_with(|items| !items.is_empty()) == Some(true) {
        seq.list_with_mut(|items| {
            if let Some(last) = items.last_mut() {
                *last = item;
            }
        });
        return Ok(());
    }
    setitem_value(interp, seq.clone(), Value::int(-1), item)
}

fn seq_push(interp: &mut Interpreter, seq: &Value, item: Value) -> Result<()> {
    if seq.is_list() {
        return seq.list_push(item);
    }
    call_method(interp, seq, "append", &[item])?;
    Ok(())
}

/// `list(seq)`.
fn seq_items(interp: &mut Interpreter, seq: &Value) -> Result<Vec<Value>> {
    if let Some(items) = seq.list_with(Vec::clone) {
        return Ok(items);
    }
    interp.collect_iterable(seq)
}

// ---------------------------------------------------------------------------
// TextWrapper
// ---------------------------------------------------------------------------

/// `TextWrapper.__init__`.
fn text_wrapper_init(interp: &mut Interpreter, args: &[ExpandedCallArg]) -> Result<()> {
    let mut b = args::bind(&SIG_INIT, args)?;
    let this = b.take_or_none(0);
    let values = [
        ("width", b.take_or(1, || Value::int(70))),
        ("initial_indent", b.take_or(2, || Value::string(""))),
        ("subsequent_indent", b.take_or(3, || Value::string(""))),
        ("expand_tabs", b.take_or(4, || Value::bool_(true))),
        ("replace_whitespace", b.take_or(5, || Value::bool_(true))),
        ("fix_sentence_endings", b.take_or(6, || Value::bool_(false))),
        ("break_long_words", b.take_or(7, || Value::bool_(true))),
        ("drop_whitespace", b.take_or(8, || Value::bool_(true))),
        ("break_on_hyphens", b.take_or(9, || Value::bool_(true))),
        ("tabsize", b.take_or(10, || Value::int(8))),
        ("max_lines", b.take_or_none(11)),
        ("placeholder", b.take_or(12, || Value::string(" [...]"))),
    ];
    for (name, value) in values {
        interp.assign_attr(this.clone(), name, value)?;
    }
    Ok(())
}

/// `_munge_whitespace(self, text)`.
fn munge_whitespace(interp: &mut Interpreter, this: &Value, text: Value) -> Result<Value> {
    let mut text = text;
    if truthy_attr(interp, this, "expand_tabs")? {
        let tabsize = attr(interp, this, "tabsize")?;
        text = call_method(interp, &text, "expandtabs", &[tabsize])?;
    }
    if truthy_attr(interp, this, "replace_whitespace")? {
        let table = attr(interp, this, "unicode_whitespace_trans")?;
        text = call_method(interp, &text, "translate", &[table])?;
    }
    Ok(text)
}

/// `_split(self, text)`.
fn split_text(interp: &mut Interpreter, this: &Value, text: Value) -> Result<Value> {
    let hyphens = attr(interp, this, "break_on_hyphens")?;
    let on_hyphens = hyphens.is_bool() && hyphens.as_bool();
    let (name, which) = if on_hyphens {
        ("wordsep_re", Stock::WordSep)
    } else {
        ("wordsep_simple_re", Stock::WordSepSimple)
    };
    let pattern = attr(interp, this, name)?;
    if is_stock(&pattern, which)
        && let Some(text) = plain_str(&text)
    {
        let chars: Vec<char> = text.as_str().unwrap_or_default().chars().collect();
        let spans = if on_hyphens {
            split::wordsep_spans(&chars)
        } else {
            split::simple_spans(&chars)
        };
        let chunks = spans
            .into_iter()
            .map(|(a, b)| Value::string(chars[a..b].iter().collect::<String>()))
            .collect();
        return Ok(Value::list(chunks));
    }
    let pieces = call_method(interp, &pattern, "split", &[text])?;
    let mut chunks = Vec::new();
    for piece in interp.collect_iterable(&pieces)? {
        if interp.truthy_value(&piece)? {
            chunks.push(piece);
        }
    }
    Ok(Value::list(chunks))
}

/// `_fix_sentence_endings(self, chunks)`.
fn fix_sentence_endings(interp: &mut Interpreter, this: &Value, chunks: &Value) -> Result<()> {
    let regex = attr(interp, this, "sentence_end_re")?;
    let search = interp.get_attr(&regex, "search")?;
    let stock = is_stock(&regex, Stock::SentenceEnd);
    let mut i: i64 = 0;
    while i < seq_len(interp, chunks)? - 1 {
        let next = seq_getitem(interp, chunks, i + 1)?;
        let is_space = if next.is_str() {
            next.as_str() == Some(" ")
        } else {
            cmp(interp, &next, BinaryOp::Eq, &Value::string(" "))?
        };
        let hit = if is_space {
            let chunk = seq_getitem(interp, chunks, i)?;
            match plain_str(&chunk) {
                Some(text) if stock => split::sentence_end(text.as_str().unwrap_or_default()),
                _ => {
                    let found = call_pos(interp, search.clone(), &[chunk])?;
                    interp.truthy_value(&found)?
                }
            }
        } else {
            false
        };
        if hit {
            setitem_value(
                interp,
                chunks.clone(),
                Value::int(i + 1),
                Value::string("  "),
            )?;
            i += 2;
        } else {
            i += 1;
        }
    }
    Ok(())
}

/// `chunk[start:stop]`.
fn slice_of(
    interp: &mut Interpreter,
    chunk: &Value,
    start: Option<Value>,
    stop: Option<Value>,
) -> Result<Value> {
    getitem_value(interp, chunk.clone(), make_slice_value(start, stop, None))
}

/// `_handle_long_word(self, reversed_chunks, cur_line, cur_len, width)`.
fn handle_long_word(
    interp: &mut Interpreter,
    this: &Value,
    reversed_chunks: &Value,
    cur_line: &Value,
    cur_len: &Value,
    width: &Value,
) -> Result<()> {
    // Figure out when indent is larger than the specified width, and make
    // sure at least one character is stripped off on every pass.
    let space_left = if cmp(interp, width, BinaryOp::Lt, &Value::int(1))? {
        Value::int(1)
    } else {
        arith(interp, width, BinaryOp::Sub, cur_len)?
    };

    if truthy_attr(interp, this, "break_long_words")? {
        let mut end = space_left.clone();
        let chunk = seq_last(interp, reversed_chunks)?;
        if truthy_attr(interp, this, "break_on_hyphens")? {
            let chunk_len = len_of(interp, &chunk)?;
            if cmp_int(interp, chunk_len, BinaryOp::Gt, &space_left)? {
                // Break after the last hyphen, but only if there are
                // non-hyphens before it.
                let hyphen = call_method(
                    interp,
                    &chunk,
                    "rfind",
                    &[Value::string("-"), Value::int(0), space_left.clone()],
                )?;
                if cmp(interp, &hyphen, BinaryOp::Gt, &Value::int(0))? {
                    let head = slice_of(interp, &chunk, None, Some(hyphen.clone()))?;
                    let mut any_other = false;
                    for c in interp.collect_iterable(&head)? {
                        if cmp(interp, &c, BinaryOp::Ne, &Value::string("-"))? {
                            any_other = true;
                            break;
                        }
                    }
                    if any_other {
                        end = arith(interp, &hyphen, BinaryOp::Add, &Value::int(1))?;
                    }
                }
            }
        }
        let head = slice_of(interp, &chunk, None, Some(end.clone()))?;
        seq_push(interp, cur_line, head)?;
        let tail = slice_of(interp, &chunk, Some(end), None)?;
        seq_set_last(interp, reversed_chunks, tail)?;
    } else if seq_len(interp, cur_line)? == 0 {
        // Otherwise, we have to preserve the long word intact.  Only add it
        // to the current line if there's nothing already there -- that
        // minimizes how much we violate the width constraint.
        let chunk = seq_last(interp, reversed_chunks)?;
        seq_del_last(interp, reversed_chunks)?;
        seq_push(interp, cur_line, chunk)?;
    }
    // If we're not allowed to break long words, and there's already text on
    // the current line, do nothing: the next pass starts a fresh line.
    Ok(())
}

/// `cur_len + length <= width` with `width` possibly not an `int`.
fn fits(interp: &mut Interpreter, len: i64, width: &Value) -> Result<bool> {
    cmp_int(interp, len, BinaryOp::Le, width)
}

/// `_wrap_chunks(self, chunks)`.
fn wrap_chunks(interp: &mut Interpreter, this: &Value, chunks: &Value) -> Result<Value> {
    let mut lines: Vec<Value> = Vec::new();
    let total_width = attr(interp, this, "width")?;
    if cmp(interp, &total_width, BinaryOp::Le, &Value::int(0))? {
        let width = attr(interp, this, "width")?;
        let message = interp.eval_binary(
            Value::string("invalid width %r (must be > 0)"),
            BinaryOp::Mod,
            width,
        )?;
        return Err(value_error(
            message.as_str().unwrap_or_default().to_string(),
        ));
    }
    let max_lines = attr(interp, this, "max_lines")?;
    if !max_lines.is_none() {
        let max_lines = attr(interp, this, "max_lines")?;
        let indent = if cmp(interp, &max_lines, BinaryOp::Gt, &Value::int(1))? {
            attr(interp, this, "subsequent_indent")?
        } else {
            attr(interp, this, "initial_indent")?
        };
        let placeholder = attr(interp, this, "placeholder")?;
        let stripped = call_method(interp, &placeholder, "lstrip", &[])?;
        let needed = len_of(interp, &indent)? + len_of(interp, &stripped)?;
        let width = attr(interp, this, "width")?;
        if cmp_int(interp, needed, BinaryOp::Gt, &width)? {
            return Err(value_error("placeholder too large for max width"));
        }
    }

    // Arrange in reverse order so items can be efficiently popped from a
    // stack of chunks.
    call_method(interp, chunks, "reverse", &[])?;

    while seq_len(interp, chunks)? > 0 {
        // Start the list of chunks that will make up the current line.
        // `cur_len` is just the length of all the chunks in `cur_line`.
        let cur_line = Value::list(Vec::new());
        let mut cur_len: i64 = 0;

        // Figure out which static string will prefix this line.
        let indent = if lines.is_empty() {
            attr(interp, this, "initial_indent")?
        } else {
            attr(interp, this, "subsequent_indent")?
        };

        // Maximum width for this line.
        let total_width = attr(interp, this, "width")?;
        let indent_len = Value::int(len_of(interp, &indent)?);
        let width = arith(interp, &total_width, BinaryOp::Sub, &indent_len)?;

        // First chunk on line is whitespace -- drop it, unless this is the
        // very beginning of the text (ie. no lines started yet).
        if truthy_attr(interp, this, "drop_whitespace")? {
            let last = seq_last(interp, chunks)?;
            if strip_is_empty(interp, &last)? && !lines.is_empty() {
                seq_del_last(interp, chunks)?;
            }
        }

        while seq_len(interp, chunks)? > 0 {
            let last = seq_last(interp, chunks)?;
            let length = len_of(interp, &last)?;

            // Can at least squeeze this chunk onto the current line.
            if fits(interp, cur_len + length, &width)? {
                seq_del_last(interp, chunks)?;
                seq_push(interp, &cur_line, last)?;
                cur_len += length;
            } else {
                // Nope, this line is full.
                break;
            }
        }

        // The current line is full, and the next chunk is too big to fit on
        // *any* line (not just this one).
        if seq_len(interp, chunks)? > 0 {
            let last = seq_last(interp, chunks)?;
            let length = len_of(interp, &last)?;
            if cmp_int(interp, length, BinaryOp::Gt, &width)? {
                let handler = attr(interp, this, "_handle_long_word")?;
                call_pos(
                    interp,
                    handler,
                    &[
                        chunks.clone(),
                        cur_line.clone(),
                        Value::int(cur_len),
                        width.clone(),
                    ],
                )?;
                cur_len = 0;
                for item in seq_items(interp, &cur_line)? {
                    cur_len += len_of(interp, &item)?;
                }
            }
        }

        // If the last chunk on this line is all whitespace, drop it.
        if truthy_attr(interp, this, "drop_whitespace")? && seq_len(interp, &cur_line)? > 0 {
            let last = seq_last(interp, &cur_line)?;
            if strip_is_empty(interp, &last)? {
                cur_len -= len_of(interp, &last)?;
                seq_del_last(interp, &cur_line)?;
            }
        }

        if seq_len(interp, &cur_line)? > 0 {
            let max_lines = attr(interp, this, "max_lines")?;
            let mut store = max_lines.is_none();
            if !store {
                let next_count = lines.len() as i64 + 1;
                store = cmp_int(interp, next_count, BinaryOp::Lt, &max_lines)?;
            }
            if !store {
                let mut rest_is_done = seq_len(interp, chunks)? == 0;
                if !rest_is_done && truthy_attr(interp, this, "drop_whitespace")? {
                    rest_is_done = seq_len(interp, chunks)? == 1 && {
                        let first = seq_getitem(interp, chunks, 0)?;
                        !strip_truthy(interp, &first)?
                    };
                }
                store = rest_is_done && fits(interp, cur_len, &width)?;
            }
            if store {
                // Convert current line back to a string and store it in the
                // list of all lines (return value).
                let items = seq_items(interp, &cur_line)?;
                let joined = join_values(interp, &items)?;
                lines.push(arith(interp, &indent, BinaryOp::Add, &joined)?);
            } else {
                let mut items = seq_items(interp, &cur_line)?;
                let mut placed = false;
                while let Some(last) = items.last().cloned() {
                    if strip_truthy(interp, &last)? {
                        let placeholder = attr(interp, this, "placeholder")?;
                        let placeholder_len = len_of(interp, &placeholder)?;
                        if fits(interp, cur_len + placeholder_len, &width)? {
                            items.push(placeholder);
                            let joined = join_values(interp, &items)?;
                            lines.push(arith(interp, &indent, BinaryOp::Add, &joined)?);
                            placed = true;
                            break;
                        }
                    }
                    cur_len -= len_of(interp, &last)?;
                    items.pop();
                }
                if !placed {
                    let mut replaced = false;
                    if let Some(last_line) = lines.last().cloned() {
                        let prev_line = call_method(interp, &last_line, "rstrip", &[])?;
                        let placeholder = attr(interp, this, "placeholder")?;
                        let needed = len_of(interp, &prev_line)? + len_of(interp, &placeholder)?;
                        let total_width = attr(interp, this, "width")?;
                        if cmp_int(interp, needed, BinaryOp::Le, &total_width)? {
                            let placeholder = attr(interp, this, "placeholder")?;
                            let merged = arith(interp, &prev_line, BinaryOp::Add, &placeholder)?;
                            if let Some(slot) = lines.last_mut() {
                                *slot = merged;
                            }
                            replaced = true;
                        }
                    }
                    if !replaced {
                        let placeholder = attr(interp, this, "placeholder")?;
                        let stripped = call_method(interp, &placeholder, "lstrip", &[])?;
                        lines.push(arith(interp, &indent, BinaryOp::Add, &stripped)?);
                    }
                }
                break;
            }
        }
    }

    Ok(Value::list(lines))
}

/// `wrap(self, text)`.
fn wrap_text(interp: &mut Interpreter, this: &Value, text: Value) -> Result<Value> {
    let split_chunks = attr(interp, this, "_split_chunks")?;
    let chunks = call_pos(interp, split_chunks, &[text])?;
    if truthy_attr(interp, this, "fix_sentence_endings")? {
        let fixer = attr(interp, this, "_fix_sentence_endings")?;
        call_pos(interp, fixer, std::slice::from_ref(&chunks))?;
    }
    let wrapper = attr(interp, this, "_wrap_chunks")?;
    call_pos(interp, wrapper, &[chunks])
}

// ---------------------------------------------------------------------------
// Convenience interface
// ---------------------------------------------------------------------------

/// `TextWrapper(width=width, **kwargs)` through the module's `TextWrapper`.
fn new_wrapper(
    interp: &mut Interpreter,
    width: Value,
    extra: &[(String, Value)],
    var_kw: Vec<(String, Value)>,
) -> Result<Value> {
    let class = module_attr(interp, "TextWrapper")?;
    let mut kw = vec![("width".to_string(), width)];
    kw.extend(extra.iter().cloned());
    for (name, value) in var_kw {
        if kw.iter().any(|(existing, _)| *existing == name) {
            return Err(type_error(format!(
                "textwrap.TextWrapper() got multiple values for keyword argument '{name}'"
            )));
        }
        kw.push((name, value));
    }
    call_with(interp, class, &[], &kw)
}

/// `' '.join(text.strip().split())`.
fn collapse_whitespace(interp: &mut Interpreter, text: &Value) -> Result<Value> {
    let stripped = call_method(interp, text, "strip", &[])?;
    let words = call_method(interp, &stripped, "split", &[])?;
    join_iterable(interp, " ", words)
}

/// `textwrap.dedent(text)`.
fn dedent_text(text: &Value) -> Result<Value> {
    let Some(text) = plain_str(text) else {
        if PyBytesLike::matches(text) {
            return Err(type_error(
                "cannot use a string pattern on a bytes-like object",
            ));
        }
        return Err(type_error(format!(
            "expected string or bytes-like object, got '{}'",
            value_type_name_str(text)
        )));
    };
    Ok(Value::string(split::dedent(
        text.as_str().unwrap_or_default(),
    )))
}

/// `textwrap.indent(text, prefix, predicate=None)`.
fn indent_text(
    interp: &mut Interpreter,
    text: &Value,
    prefix: &Value,
    predicate: &Value,
) -> Result<Value> {
    let lines = call_method(interp, text, "splitlines", &[Value::bool_(true)])?;
    let mut out = Vec::new();
    for line in interp.collect_iterable(&lines)? {
        let selected = if predicate.is_none() {
            strip_truthy(interp, &line)?
        } else {
            let verdict = call_pos(interp, predicate.clone(), std::slice::from_ref(&line))?;
            interp.truthy_value(&verdict)?
        };
        out.push(if selected {
            arith(interp, prefix, BinaryOp::Add, &line)?
        } else {
            line
        });
    }
    join_values(interp, &out)
}

// ---------------------------------------------------------------------------
// Docs / import wiring
// ---------------------------------------------------------------------------

/// The CPython docstring of a `textwrap` callable, by registry name
/// (`textwrap.wrap`, `textwrap.TextWrapper._split`).  `None`: not a `textwrap`
/// callable; `Some(None)`: a callable that has no docstring.
pub(crate) fn native_doc(registry_key: &str) -> Option<Option<&'static str>> {
    let name = registry_key.strip_prefix("textwrap.")?;
    Some(match name {
        "wrap" => Some(docs::DOC_WRAP),
        "fill" => Some(docs::DOC_FILL),
        "shorten" => Some(docs::DOC_SHORTEN),
        "dedent" => Some(docs::DOC_DEDENT),
        "indent" => Some(docs::DOC_INDENT),
        "TextWrapper._munge_whitespace" => Some(docs::DOC_MUNGE_WHITESPACE),
        "TextWrapper._split" => Some(docs::DOC_SPLIT),
        "TextWrapper._fix_sentence_endings" => Some(docs::DOC_FIX_SENTENCE_ENDINGS),
        "TextWrapper._handle_long_word" => Some(docs::DOC_HANDLE_LONG_WORD),
        "TextWrapper._wrap_chunks" => Some(docs::DOC_WRAP_CHUNKS),
        "TextWrapper.wrap" => Some(docs::DOC_WRAP_METHOD),
        "TextWrapper.fill" => Some(docs::DOC_FILL_METHOD),
        "TextWrapper.__init__" | "TextWrapper._split_chunks" => None,
        _ => return None,
    })
}

/// Post-import wiring: `import re`, the compiled class-level regexes (and the
/// module-level `dedent` ones) and the source's namespace order.
pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let re = interp.load_module("re")?;
    let verbose = interp.get_attr(&re, "VERBOSE")?;
    let multiline = interp.get_attr(&re, "MULTILINE")?;
    let compile = interp.get_attr(&re, "compile")?;
    let compile_with = |interp: &mut Interpreter, pattern: &str, flags: Option<&Value>| {
        let mut pos = vec![Value::string(pattern)];
        pos.extend(flags.cloned());
        call_pos(interp, compile.clone(), &pos)
    };
    let wordsep = compile_with(interp, patterns::WORDSEP_PATTERN, Some(&verbose))?;
    let wordsep_simple = compile_with(interp, patterns::WORDSEP_SIMPLE_PATTERN, None)?;
    let sentence_end = compile_with(interp, patterns::SENTENCE_END_PATTERN, None)?;
    let whitespace_only =
        compile_with(interp, patterns::WHITESPACE_ONLY_PATTERN, Some(&multiline))?;
    let leading_whitespace = compile_with(
        interp,
        patterns::LEADING_WHITESPACE_PATTERN,
        Some(&multiline),
    )?;

    let class = module.borrow().get_attr_value("TextWrapper");
    if let Some(class) = class {
        for (name, value) in [
            ("wordsep_re", &wordsep),
            ("wordsep_simple_re", &wordsep_simple),
            ("sentence_end_re", &sentence_end),
        ] {
            interp.assign_attr(class.clone(), name, value.clone())?;
        }
    }
    STOCK_REGEXES.with(|all| {
        all.borrow_mut()
            .push([wordsep, wordsep_simple, sentence_end]);
    });

    let mut module = module.borrow_mut();
    module.insert_attr("re".to_string(), re);
    module.insert_attr("_whitespace".to_string(), Value::string(WHITESPACE));
    module.insert_attr("_whitespace_only_re".to_string(), whitespace_only);
    module.insert_attr("_leading_whitespace_re".to_string(), leading_whitespace);
    for name in NAMESPACE_ORDER {
        if let Some(value) = module.attrs.shift_remove(*name) {
            module.attrs.insert((*name).to_string(), value);
        }
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(docs::MODULE_DOC),
        "__all__" => all_list(),
    }

    class TextWrapper {
        constants {
            "__doc__" => Value::string(docs::CLASS_DOC),
            "unicode_whitespace_trans" => whitespace_translation(),
        }

        fn __init__(args) -> Result<Value> {
            text_wrapper_init(_interp, args)?;
            Ok(Value::none())
        }

        fn _munge_whitespace(args) -> Result<Value> {
            let mut b = args::bind(&SIG_MUNGE, args)?;
            let this = b.take_or_none(0);
            let text = b.take_or_none(1);
            munge_whitespace(_interp, &this, text)
        }

        fn _split(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SPLIT, args)?;
            let this = b.take_or_none(0);
            let text = b.take_or_none(1);
            split_text(_interp, &this, text)
        }

        fn _fix_sentence_endings(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FIX_SENTENCE_ENDINGS, args)?;
            let this = b.take_or_none(0);
            let chunks = b.take_or_none(1);
            fix_sentence_endings(_interp, &this, &chunks)?;
            Ok(Value::none())
        }

        fn _handle_long_word(args) -> Result<Value> {
            let mut b = args::bind(&SIG_HANDLE_LONG_WORD, args)?;
            let this = b.take_or_none(0);
            let reversed_chunks = b.take_or_none(1);
            let cur_line = b.take_or_none(2);
            let cur_len = b.take_or_none(3);
            let width = b.take_or_none(4);
            handle_long_word(_interp, &this, &reversed_chunks, &cur_line, &cur_len, &width)?;
            Ok(Value::none())
        }

        fn _wrap_chunks(args) -> Result<Value> {
            let mut b = args::bind(&SIG_WRAP_CHUNKS, args)?;
            let this = b.take_or_none(0);
            let chunks = b.take_or_none(1);
            wrap_chunks(_interp, &this, &chunks)
        }

        fn _split_chunks(args) -> Result<Value> {
            let mut b = args::bind(&SIG_SPLIT_CHUNKS, args)?;
            let this = b.take_or_none(0);
            let text = b.take_or_none(1);
            let munge = attr(_interp, &this, "_munge_whitespace")?;
            let text = call_pos(_interp, munge, &[text])?;
            let splitter = attr(_interp, &this, "_split")?;
            call_pos(_interp, splitter, &[text])
        }

        fn wrap(args) -> Result<Value> {
            let mut b = args::bind(&SIG_WRAP_METHOD, args)?;
            let this = b.take_or_none(0);
            let text = b.take_or_none(1);
            wrap_text(_interp, &this, text)
        }

        fn fill(args) -> Result<Value> {
            let mut b = args::bind(&SIG_FILL_METHOD, args)?;
            let this = b.take_or_none(0);
            let text = b.take_or_none(1);
            let wrapper = attr(_interp, &this, "wrap")?;
            let lines = call_pos(_interp, wrapper, &[text])?;
            join_iterable(_interp, "\n", lines)
        }
    }

    fn wrap(args) -> Result<Value> {
        let mut b = args::bind(&SIG_WRAP, args)?;
        let text = b.take_or_none(0);
        let width = b.take_or(1, || Value::int(70));
        let wrapper = new_wrapper(_interp, width, &[], std::mem::take(&mut b.var_kw))?;
        call_method(_interp, &wrapper, "wrap", &[text])
    }

    fn fill(args) -> Result<Value> {
        let mut b = args::bind(&SIG_FILL, args)?;
        let text = b.take_or_none(0);
        let width = b.take_or(1, || Value::int(70));
        let wrapper = new_wrapper(_interp, width, &[], std::mem::take(&mut b.var_kw))?;
        call_method(_interp, &wrapper, "fill", &[text])
    }

    fn shorten(args) -> Result<Value> {
        let mut b = args::bind(&SIG_SHORTEN, args)?;
        let text = b.take_or_none(0);
        let width = b.take_or_none(1);
        let wrapper = new_wrapper(
            _interp,
            width,
            &[("max_lines".to_string(), Value::int(1))],
            std::mem::take(&mut b.var_kw),
        )?;
        let collapsed = collapse_whitespace(_interp, &text)?;
        call_method(_interp, &wrapper, "fill", &[collapsed])
    }

    fn dedent(args) -> Result<Value> {
        let mut b = args::bind(&SIG_DEDENT, args)?;
        let text = b.take_or_none(0);
        dedent_text(&text)
    }

    fn indent(args) -> Result<Value> {
        let mut b = args::bind(&SIG_INDENT, args)?;
        let text = b.take_or_none(0);
        let prefix = b.take_or_none(1);
        let predicate = b.take_or_none(2);
        indent_text(_interp, &text, &prefix, &predicate)
    }
}
