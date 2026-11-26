// `shlex` module — 100% native Rust implementation of CPython 3.12.3's
// `Lib/shlex.py` (issues #3147, #3356).
//
// A lexical analyzer for simple shell-like syntaxes: the `shlex` class plus the
// `split`, `join`, and `quote` helpers.  Like CPython's pure-Python class, all
// lexer configuration and state (`instream`, `infile`, `posix`, `eof`,
// `commenters`, `wordchars`, `whitespace`, `whitespace_split`, `quotes`,
// `escape`, `escapedquotes`, `state`, `pushback`, `lineno`, `debug`, `token`,
// `filestack`, `source`, `_punctuation_chars`, `_pushback_chars`) lives in the
// instance `__dict__`, and the methods dispatch through the instance
// (`self.read_token()`, `self.sourcehook()`, ...) so subclasses may override
// them exactly as they can in CPython.
//
// Reference: <https://docs.python.org/3/library/shlex.html>

use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr, value_type_name_str};
use crate::value::{Value, ValueKind};

const MODULE_DOC: &str = "A lexical analyzer class for simple shell-like syntaxes.";

/// `shlex.wordchars` for the non-POSIX lexer.
const WORDCHARS: &str = "abcdfeghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_";
/// Extra `wordchars` added in POSIX mode.
const POSIX_WORDCHARS: &str = "ßàáâãäåæçèéêëìíîïðñòóôõöøùúûüýþÿÀÁÂÃÄÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖØÙÚÛÜÝÞ";
/// `wordchars` additions when `punctuation_chars` is in use.
const PUNCT_WORDCHARS: &str = "~-./*?=";
/// `punctuation_chars=True` expands to this set.
const DEFAULT_PUNCTUATION: &str = "();<>|&";

// ── Generic interpreter helpers ──────────────────────────────────────────────

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
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
    args: &[Value],
) -> Result<Value> {
    let callee = interp.get_attr(target, name)?;
    call_pos(interp, callee, args)
}

fn type_error(msg: impl Into<String>) -> PyError {
    PyError::named("TypeError", msg.into())
}

fn value_error(msg: impl Into<String>) -> PyError {
    PyError::named("ValueError", msg.into())
}

/// The text of an exact `str` or a `str` subclass instance.
fn str_content(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_owned());
    }
    if let ValueKind::PyInstance(inst) = v.kind()
        && let Some(data) = inst.borrow().attrs.builtin_data()
    {
        return data.as_str().map(str::to_owned);
    }
    None
}

fn py_isinstance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> Result<bool> {
    let res = call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls.clone()],
    )?;
    interp.truthy_value(&res)
}

fn is_str_instance(interp: &mut Interpreter, obj: &Value) -> Result<bool> {
    if obj.is_str() {
        return Ok(true);
    }
    let builtins = interp.load_module("builtins")?;
    let str_cls = interp.get_attr(&builtins, "str")?;
    py_isinstance(interp, obj, &str_cls)
}

fn py_cmp(interp: &mut Interpreter, a: &Value, op: BinaryOp, b: &Value) -> Result<bool> {
    let res = interp.eval_binary(a.clone(), op, b.clone())?;
    interp.truthy_value(&res)
}

fn py_print(interp: &mut Interpreter, text: String) -> Result<()> {
    call_pos(
        interp,
        Value::builtin_function("print"),
        &[Value::string(text)],
    )?;
    Ok(())
}

/// `fmt % args` with CPython's `%` formatting semantics.
fn percent_format(interp: &mut Interpreter, fmt: &str, args: Vec<Value>) -> Result<String> {
    let res = interp.eval_binary(Value::string(fmt), BinaryOp::Mod, Value::tuple(args))?;
    Ok(str_content(&res).unwrap_or_default())
}

/// `a, b, ... = value` with CPython's unpacking errors.
fn unpack(interp: &mut Interpreter, value: &Value, expected: usize) -> Result<Vec<Value>> {
    let items = interp.collect_iterable(value)?;
    if items.len() < expected {
        return Err(value_error(format!(
            "not enough values to unpack (expected {expected}, got {})",
            items.len()
        )));
    }
    if items.len() > expected {
        return Err(value_error(format!(
            "too many values to unpack (expected {expected})"
        )));
    }
    Ok(items)
}

fn get(interp: &mut Interpreter, this: &Value, name: &str) -> Result<Value> {
    interp.get_attr(this, name)
}

fn set(interp: &mut Interpreter, this: &Value, name: &str, value: Value) -> Result<()> {
    interp.assign_attr(this.clone(), name, value)
}

fn new_deque(interp: &mut Interpreter) -> Result<Value> {
    let collections = interp.load_module("collections")?;
    let deque = interp.get_attr(&collections, "deque")?;
    call_pos(interp, deque, &[])
}

fn new_string_io(interp: &mut Interpreter, text: Value) -> Result<Value> {
    let io = interp.load_module("io")?;
    let string_io = interp.get_attr(&io, "StringIO")?;
    // A `str` subclass yields the same stream content as its plain text.
    let text = match str_content(&text) {
        Some(plain) => Value::string(plain),
        None => text,
    };
    call_pos(interp, string_io, &[text])
}

/// `self.debug <op> n` for the lexer's `debug` attribute.
fn debug_cmp(interp: &mut Interpreter, debug: &Value, op: BinaryOp, n: i64) -> Result<bool> {
    if let Some(d) = debug.as_int() {
        return Ok(match op {
            BinaryOp::Ge => d >= n,
            BinaryOp::Gt => d > n,
            _ => false,
        });
    }
    py_cmp(interp, debug, op, &Value::int(n))
}

fn debug_at_least(interp: &mut Interpreter, this: &Value, n: i64) -> Result<bool> {
    let debug = get(interp, this, "debug")?;
    debug_cmp(interp, &debug, BinaryOp::Ge, n)
}

// ── Character-set attributes ─────────────────────────────────────────────────

/// A `str`-valued lexer attribute (`whitespace`, `quotes`, ...) whose `in` test
/// is a substring check; any other type falls back to the generic `in`.
enum Chars {
    Text(String),
    Other(Value),
}

impl Chars {
    fn new(v: Value) -> Self {
        match str_content(&v) {
            Some(s) => Chars::Text(s),
            None => Chars::Other(v),
        }
    }

    fn has(&self, interp: &mut Interpreter, needle: &str) -> Result<bool> {
        match self {
            Chars::Text(s) => Ok(s.contains(needle)),
            Chars::Other(v) => py_cmp(interp, &Value::string(needle), BinaryOp::In, v),
        }
    }

    fn truthy(&self, interp: &mut Interpreter) -> Result<bool> {
        match self {
            Chars::Text(s) => Ok(!s.is_empty()),
            Chars::Other(v) => interp.truthy_value(v),
        }
    }
}

/// Lexer configuration read at the start of `read_token`.
struct Config {
    posix: bool,
    whitespace_split: bool,
    whitespace: Chars,
    commenters: Chars,
    wordchars: Chars,
    punct: Chars,
    punct_truthy: bool,
    quotes: Chars,
    escape: Chars,
    escapedquotes: Chars,
    instream: Value,
    dbg2: bool,
    dbg3: bool,
    dbg_gt1: bool,
}

/// `self.state`, `self.token`, `self.lineno` as locals while scanning.
struct Scan {
    /// `None` is Python `None` (past end of file).
    state: Option<String>,
    token: String,
    lineno: Value,
}

impl Scan {
    fn bump_lineno(&mut self, interp: &mut Interpreter) -> Result<()> {
        self.lineno = match self.lineno.as_int() {
            Some(n) if !self.lineno.is_bool() => Value::int(n + 1),
            _ => interp.eval_binary(self.lineno.clone(), BinaryOp::Add, Value::int(1))?,
        };
        Ok(())
    }

    fn state_value(&self) -> Value {
        match &self.state {
            Some(s) => Value::string(s.as_str()),
            None => Value::none(),
        }
    }

    fn flush(&self, interp: &mut Interpreter, this: &Value) -> Result<()> {
        set(interp, this, "state", self.state_value())?;
        set(interp, this, "token", Value::string(self.token.as_str()))?;
        set(interp, this, "lineno", self.lineno.clone())
    }
}

/// `nextchar` as text; a non-`str` read result behaves like CPython's `in`
/// operator on a `str` container (falsy values are treated as end of input).
fn nextchar_text(interp: &mut Interpreter, v: &Value) -> Result<String> {
    if let Some(s) = str_content(v) {
        return Ok(s);
    }
    if !interp.truthy_value(v)? {
        return Ok(String::new());
    }
    Err(type_error(format!(
        "'in <string>' requires string as left operand, not {}",
        value_type_name_str(v)
    )))
}

fn pushback_chars(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    get(interp, this, "_pushback_chars")
}

fn read_token_scan(
    interp: &mut Interpreter,
    this: &Value,
    cfg: &Config,
    scan: &mut Scan,
) -> Result<bool> {
    let mut quoted = false;
    let mut escapedstate = String::from(" ");
    loop {
        let raw = if cfg.punct_truthy {
            let chars = pushback_chars(interp, this)?;
            if interp.truthy_value(&chars)? {
                call_method(interp, &chars, "pop", &[])?
            } else {
                call_method(interp, &cfg.instream, "read", &[Value::int(1)])?
            }
        } else {
            call_method(interp, &cfg.instream, "read", &[Value::int(1)])?
        };
        let nextchar = nextchar_text(interp, &raw)?;
        let nc = nextchar.as_str();
        if nc == "\n" {
            scan.bump_lineno(interp)?;
        }
        if cfg.dbg3 {
            let line = percent_format(
                interp,
                "shlex: in state %r I see character: %r",
                vec![scan.state_value(), Value::string(nc)],
            )?;
            py_print(interp, line)?;
        }
        let Some(state) = scan.state.clone() else {
            scan.token.clear(); // past end of file
            break;
        };
        // "emit current token" test shared by several branches.
        let emit = !scan.token.is_empty() || (cfg.posix && quoted);
        if state == " " {
            if nc.is_empty() {
                scan.state = None; // end of file
                break;
            } else if cfg.whitespace.has(interp, nc)? {
                if cfg.dbg2 {
                    py_print(
                        interp,
                        "shlex: I see whitespace in whitespace state".to_string(),
                    )?;
                }
                if emit {
                    break;
                } else {
                    continue;
                }
            } else if cfg.commenters.has(interp, nc)? {
                call_method(interp, &cfg.instream, "readline", &[])?;
                scan.bump_lineno(interp)?;
            } else if cfg.posix && cfg.escape.has(interp, nc)? {
                escapedstate = "a".to_string();
                scan.state = Some(nc.to_string());
            } else if cfg.wordchars.has(interp, nc)? {
                scan.token = nc.to_string();
                scan.state = Some("a".to_string());
            } else if cfg.punct.has(interp, nc)? {
                scan.token = nc.to_string();
                scan.state = Some("c".to_string());
            } else if cfg.quotes.has(interp, nc)? {
                if !cfg.posix {
                    scan.token = nc.to_string();
                }
                scan.state = Some(nc.to_string());
            } else if cfg.whitespace_split {
                scan.token = nc.to_string();
                scan.state = Some("a".to_string());
            } else {
                scan.token = nc.to_string();
                if !scan.token.is_empty() || (cfg.posix && quoted) {
                    break;
                } else {
                    continue;
                }
            }
        } else if cfg.quotes.has(interp, &state)? {
            quoted = true;
            if nc.is_empty() {
                // end of file
                if cfg.dbg2 {
                    py_print(interp, "shlex: I see EOF in quotes state".to_string())?;
                }
                return Err(value_error("No closing quotation"));
            }
            if nc == state {
                if !cfg.posix {
                    scan.token.push_str(nc);
                    scan.state = Some(" ".to_string());
                    break;
                } else {
                    scan.state = Some("a".to_string());
                }
            } else if cfg.posix
                && cfg.escape.has(interp, nc)?
                && cfg.escapedquotes.has(interp, &state)?
            {
                escapedstate = state;
                scan.state = Some(nc.to_string());
            } else {
                scan.token.push_str(nc);
            }
        } else if cfg.escape.has(interp, &state)? {
            if nc.is_empty() {
                // end of file
                if cfg.dbg2 {
                    py_print(interp, "shlex: I see EOF in escape state".to_string())?;
                }
                return Err(value_error("No escaped character"));
            }
            // In posix shells, only the quote itself or the escape
            // character may be escaped within quotes.
            if cfg.quotes.has(interp, &escapedstate)? && nc != state && nc != escapedstate {
                scan.token.push_str(&state);
            }
            scan.token.push_str(nc);
            scan.state = Some(escapedstate.clone());
        } else if state == "a" || state == "c" {
            if nc.is_empty() {
                scan.state = None; // end of file
                break;
            } else if cfg.whitespace.has(interp, nc)? {
                if cfg.dbg2 {
                    py_print(interp, "shlex: I see whitespace in word state".to_string())?;
                }
                scan.state = Some(" ".to_string());
                if emit {
                    break;
                } else {
                    continue;
                }
            } else if cfg.commenters.has(interp, nc)? {
                call_method(interp, &cfg.instream, "readline", &[])?;
                scan.bump_lineno(interp)?;
                if cfg.posix {
                    scan.state = Some(" ".to_string());
                    if emit {
                        break;
                    } else {
                        continue;
                    }
                }
            } else if state == "c" {
                if cfg.punct.has(interp, nc)? {
                    scan.token.push_str(nc);
                } else {
                    if !cfg.whitespace.has(interp, nc)? {
                        let chars = pushback_chars(interp, this)?;
                        call_method(interp, &chars, "append", &[Value::string(nc)])?;
                    }
                    scan.state = Some(" ".to_string());
                    break;
                }
            } else if cfg.posix && cfg.quotes.has(interp, nc)? {
                scan.state = Some(nc.to_string());
            } else if cfg.posix && cfg.escape.has(interp, nc)? {
                escapedstate = "a".to_string();
                scan.state = Some(nc.to_string());
            } else if cfg.wordchars.has(interp, nc)?
                || cfg.quotes.has(interp, nc)?
                || (cfg.whitespace_split && !cfg.punct.has(interp, nc)?)
            {
                scan.token.push_str(nc);
            } else {
                if cfg.punct_truthy {
                    let chars = pushback_chars(interp, this)?;
                    call_method(interp, &chars, "append", &[Value::string(nc)])?;
                } else {
                    let pushback = get(interp, this, "pushback")?;
                    call_method(interp, &pushback, "appendleft", &[Value::string(nc)])?;
                }
                if cfg.dbg2 {
                    py_print(interp, "shlex: I see punctuation in word state".to_string())?;
                }
                scan.state = Some(" ".to_string());
                if emit {
                    break;
                } else {
                    continue;
                }
            }
        }
    }
    Ok(quoted)
}

fn read_token_impl(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let posix_v = get(interp, this, "posix")?;
    let posix = interp.truthy_value(&posix_v)?;
    let ws_split_v = get(interp, this, "whitespace_split")?;
    let punct_v = get(interp, this, "punctuation_chars")?;
    let punct = Chars::new(punct_v);
    let debug = get(interp, this, "debug")?;
    let cfg = Config {
        posix,
        whitespace_split: interp.truthy_value(&ws_split_v)?,
        whitespace: Chars::new(get(interp, this, "whitespace")?),
        commenters: Chars::new(get(interp, this, "commenters")?),
        wordchars: Chars::new(get(interp, this, "wordchars")?),
        punct_truthy: punct.truthy(interp)?,
        punct,
        quotes: Chars::new(get(interp, this, "quotes")?),
        escape: Chars::new(get(interp, this, "escape")?),
        escapedquotes: Chars::new(get(interp, this, "escapedquotes")?),
        instream: get(interp, this, "instream")?,
        dbg2: debug_cmp(interp, &debug, BinaryOp::Ge, 2)?,
        dbg3: debug_cmp(interp, &debug, BinaryOp::Ge, 3)?,
        dbg_gt1: debug_cmp(interp, &debug, BinaryOp::Gt, 1)?,
    };
    let state_v = get(interp, this, "state")?;
    let token_v = get(interp, this, "token")?;
    let mut scan = Scan {
        state: if state_v.is_none() {
            None
        } else {
            Some(str_content(&state_v).ok_or_else(|| {
                type_error(format!(
                    "'in <string>' requires string as left operand, not {}",
                    value_type_name_str(&state_v)
                ))
            })?)
        },
        token: str_content(&token_v).ok_or_else(|| {
            type_error(format!(
                "can only concatenate str (not \"{}\") to str",
                value_type_name_str(&token_v)
            ))
        })?,
        lineno: get(interp, this, "lineno")?,
    };
    let outcome = read_token_scan(interp, this, &cfg, &mut scan);
    // The attributes mirror the scan locals even when scanning raised.
    let flushed = scan.flush(interp, this);
    let quoted = outcome?;
    flushed?;

    // `result = self.token; self.token = ''`
    let mut result = Value::string(scan.token.as_str());
    set(interp, this, "token", Value::string(""))?;
    if posix && !quoted && scan.token.is_empty() {
        result = Value::none();
    }
    if cfg.dbg_gt1 {
        if interp.truthy_value(&result)? {
            let repr = render_value_repr(interp, &result)?;
            py_print(interp, format!("shlex: raw token={repr}"))?;
        } else {
            py_print(interp, "shlex: raw token=EOF".to_string())?;
        }
    }
    Ok(result)
}

// ── get_token / sources ──────────────────────────────────────────────────────

fn get_token_impl(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let pushback = get(interp, this, "pushback")?;
    if interp.truthy_value(&pushback)? {
        let tok = call_method(interp, &pushback, "popleft", &[])?;
        if debug_at_least(interp, this, 1)? {
            let repr = render_value_repr(interp, &tok)?;
            py_print(interp, format!("shlex: popping token {repr}"))?;
        }
        return Ok(tok);
    }
    // No pushback.  Get a token.
    let mut raw = call_method(interp, this, "read_token", &[])?;
    // Handle inclusions
    if !get(interp, this, "source")?.is_none() {
        loop {
            let source = get(interp, this, "source")?;
            if !py_cmp(interp, &raw, BinaryOp::Eq, &source)? {
                break;
            }
            let name = call_method(interp, this, "read_token", &[])?;
            let spec = call_method(interp, this, "sourcehook", &[name])?;
            if interp.truthy_value(&spec)? {
                let pair = unpack(interp, &spec, 2)?;
                call_method(
                    interp,
                    this,
                    "push_source",
                    &[pair[1].clone(), pair[0].clone()],
                )?;
            }
            raw = call_method(interp, this, "get_token", &[])?;
        }
    }
    // Maybe we got EOF instead?
    loop {
        let eof = get(interp, this, "eof")?;
        if !py_cmp(interp, &raw, BinaryOp::Eq, &eof)? {
            break;
        }
        let filestack = get(interp, this, "filestack")?;
        if !interp.truthy_value(&filestack)? {
            return get(interp, this, "eof");
        }
        call_method(interp, this, "pop_source", &[])?;
        raw = call_method(interp, this, "get_token", &[])?;
    }
    // Neither inclusion nor EOF
    if debug_at_least(interp, this, 1)? {
        let eof = get(interp, this, "eof")?;
        if py_cmp(interp, &raw, BinaryOp::Ne, &eof)? {
            let repr = render_value_repr(interp, &raw)?;
            py_print(interp, format!("shlex: token={repr}"))?;
        } else {
            py_print(interp, "shlex: token=EOF".to_string())?;
        }
    }
    Ok(raw)
}

fn push_source_impl(
    interp: &mut Interpreter,
    this: &Value,
    newstream: Value,
    newfile: Value,
) -> Result<()> {
    let newstream = if is_str_instance(interp, &newstream)? {
        new_string_io(interp, newstream)?
    } else {
        newstream
    };
    let entry = Value::tuple(vec![
        get(interp, this, "infile")?,
        get(interp, this, "instream")?,
        get(interp, this, "lineno")?,
    ]);
    let filestack = get(interp, this, "filestack")?;
    call_method(interp, &filestack, "appendleft", &[entry])?;
    set(interp, this, "infile", newfile.clone())?;
    set(interp, this, "instream", newstream)?;
    set(interp, this, "lineno", Value::int(1))?;
    let debug = get(interp, this, "debug")?;
    if interp.truthy_value(&debug)? {
        let line = if !newfile.is_none() {
            let infile = get(interp, this, "infile")?;
            percent_format(interp, "shlex: pushing to file %s", vec![infile])?
        } else {
            let instream = get(interp, this, "instream")?;
            percent_format(interp, "shlex: pushing to stream %s", vec![instream])?
        };
        py_print(interp, line)?;
    }
    Ok(())
}

fn pop_source_impl(interp: &mut Interpreter, this: &Value) -> Result<()> {
    let instream = get(interp, this, "instream")?;
    call_method(interp, &instream, "close", &[])?;
    let filestack = get(interp, this, "filestack")?;
    let entry = call_method(interp, &filestack, "popleft", &[])?;
    let parts = unpack(interp, &entry, 3)?;
    set(interp, this, "infile", parts[0].clone())?;
    set(interp, this, "instream", parts[1].clone())?;
    set(interp, this, "lineno", parts[2].clone())?;
    let debug = get(interp, this, "debug")?;
    if interp.truthy_value(&debug)? {
        let instream = get(interp, this, "instream")?;
        let lineno = get(interp, this, "lineno")?;
        let line = percent_format(
            interp,
            "shlex: popping to %s, line %d",
            vec![instream, lineno],
        )?;
        py_print(interp, line)?;
    }
    set(interp, this, "state", Value::string(" "))?;
    Ok(())
}

fn sourcehook_impl(interp: &mut Interpreter, this: &Value, newfile: Value) -> Result<Value> {
    let first = interp.eval_index(&newfile, Value::int(0))?;
    let mut newfile = newfile;
    if py_cmp(interp, &first, BinaryOp::Eq, &Value::string("\""))? {
        // newfile[1:-1]
        newfile = match str_content(&newfile) {
            Some(text) => {
                let chars: Vec<char> = text.chars().collect();
                let end = chars.len().saturating_sub(1);
                let inner: String = if end > 1 {
                    chars[1..end].iter().collect()
                } else {
                    String::new()
                };
                Value::string(inner)
            }
            None => {
                let slice = call_pos(
                    interp,
                    Value::builtin_function("slice"),
                    &[Value::int(1), Value::int(-1)],
                )?;
                interp.eval_index(&newfile, slice)?
            }
        };
    }
    let infile = get(interp, this, "infile")?;
    if is_str_instance(interp, &infile)? {
        let os_path = interp.load_module("os.path")?;
        let isabs = interp.get_attr(&os_path, "isabs")?;
        let absolute = call_pos(interp, isabs, &[newfile.clone()])?;
        if !interp.truthy_value(&absolute)? {
            let dirname = interp.get_attr(&os_path, "dirname")?;
            let dir = call_pos(interp, dirname, &[infile])?;
            let join = interp.get_attr(&os_path, "join")?;
            newfile = call_pos(interp, join, &[dir, newfile])?;
        }
    }
    let stream = call_pos(
        interp,
        Value::builtin_function("open"),
        &[newfile.clone(), Value::string("r")],
    )?;
    Ok(Value::tuple(vec![newfile, stream]))
}

// ── Module-level helpers ─────────────────────────────────────────────────────

/// `shlex.quote` safe set: `[\w@%+=:,./-]` with `re.ASCII`.
fn is_safe_quote_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-')
}

fn quote_value(interp: &mut Interpreter, s: &Value) -> Result<Value> {
    if !interp.truthy_value(s)? {
        return Ok(Value::string("''"));
    }
    let Some(text) = str_content(s) else {
        // `_find_unsafe(s)` is a `str` regex: it rejects every non-str.
        let builtins = interp.load_module("builtins")?;
        for name in ["bytes", "bytearray", "memoryview"] {
            let cls = interp.get_attr(&builtins, name)?;
            if py_isinstance(interp, s, &cls)? {
                return Err(type_error(
                    "cannot use a string pattern on a bytes-like object",
                ));
            }
        }
        return Err(type_error(format!(
            "expected string or bytes-like object, got '{}'",
            value_type_name_str(s)
        )));
    };
    if text.chars().all(is_safe_quote_char) {
        return Ok(s.clone());
    }
    // use single quotes, and put single quotes into double quotes
    // the string $'b is then quoted as '$'"'"'b'
    Ok(Value::string(format!(
        "'{}'",
        text.replace('\'', "'\"'\"'")
    )))
}

fn module_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let module = interp.load_module("shlex")?;
    interp.get_attr(&module, name)
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => Value::list(
            ["shlex", "split", "quote", "join"]
                .into_iter()
                .map(Value::string)
                .collect(),
        ),
    }

    class shlex {
        constants {
            "__doc__" => Value::string(MODULE_DOC),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(PyValue(Value::none()))] instream: PyValue,
            #[default(PyValue(Value::none()))] infile: PyValue,
            #[default(PyValue(Value::bool_(false)))] posix: PyValue,
            #[default(PyValue(Value::bool_(false)))] punctuation_chars: PyValue,
        ) -> Result<Value> {
            let this = self_val.0;
            let mut instream = instream.0;
            if is_str_instance(_interp, &instream)? {
                instream = new_string_io(_interp, instream)?;
            }
            if !instream.is_none() {
                set(_interp, this, "instream", instream)?;
                set(_interp, this, "infile", infile.0)?;
            } else {
                let sys = _interp.load_module("sys")?;
                let stdin = _interp.get_attr(&sys, "stdin")?;
                set(_interp, this, "instream", stdin)?;
                set(_interp, this, "infile", Value::none())?;
            }
            let posix_truthy = _interp.truthy_value(&posix.0)?;
            set(_interp, this, "posix", posix.0)?;
            set(
                _interp,
                this,
                "eof",
                if posix_truthy { Value::none() } else { Value::string("") },
            )?;
            set(_interp, this, "commenters", Value::string("#"))?;
            let mut wordchars = WORDCHARS.to_string();
            if posix_truthy {
                wordchars.push_str(POSIX_WORDCHARS);
            }
            set(_interp, this, "wordchars", Value::string(wordchars.as_str()))?;
            // (reassigned below when punctuation chars are in use; the dict
            // slot keeps its position like CPython's instance `__dict__`)
            set(_interp, this, "whitespace", Value::string(" \t\r\n"))?;
            set(_interp, this, "whitespace_split", Value::bool_(false))?;
            set(_interp, this, "quotes", Value::string("'\""))?;
            set(_interp, this, "escape", Value::string("\\"))?;
            set(_interp, this, "escapedquotes", Value::string("\""))?;
            set(_interp, this, "state", Value::string(" "))?;
            let pushback = new_deque(_interp)?;
            set(_interp, this, "pushback", pushback)?;
            set(_interp, this, "lineno", Value::int(1))?;
            set(_interp, this, "debug", Value::int(0))?;
            set(_interp, this, "token", Value::string(""))?;
            let filestack = new_deque(_interp)?;
            set(_interp, this, "filestack", filestack)?;
            set(_interp, this, "source", Value::none())?;
            let mut punctuation = punctuation_chars.0;
            if !_interp.truthy_value(&punctuation)? {
                punctuation = Value::string("");
            } else if punctuation.is_bool() {
                punctuation = Value::string(DEFAULT_PUNCTUATION);
            }
            set(_interp, this, "_punctuation_chars", punctuation.clone())?;
            if _interp.truthy_value(&punctuation)? {
                // _pushback_chars is a push back queue used by lookahead logic
                let pushback_chars = new_deque(_interp)?;
                set(_interp, this, "_pushback_chars", pushback_chars)?;
                // these chars added because allowed in file names, args, wildcards
                wordchars.push_str(PUNCT_WORDCHARS);
                let wordchars = Value::string(wordchars.as_str());
                // remove any punctuation chars from wordchars
                let builtins = _interp.load_module("builtins")?;
                let dict_cls = _interp.get_attr(&builtins, "dict")?;
                let fromkeys = _interp.get_attr(&dict_cls, "fromkeys")?;
                let keys = call_pos(_interp, fromkeys, &[punctuation])?;
                let table = call_method(_interp, &wordchars, "maketrans", &[keys])?;
                let translated = call_method(_interp, &wordchars, "translate", &[table])?;
                set(_interp, this, "wordchars", translated)?;
            }
            Ok(Value::none())
        }

        #[getter]
        fn punctuation_chars(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            get(_interp, self_val.0, "_punctuation_chars")
        }

        /// Push a token onto the stack popped by the get_token method
        fn push_token(
            #[positional_only] self_val: PyValueRef<'_>,
            tok: PyValue,
        ) -> Result<Value> {
            if debug_at_least(_interp, self_val.0, 1)? {
                let repr = render_value_repr(_interp, &tok.0)?;
                py_print(_interp, format!("shlex: pushing token {repr}"))?;
            }
            let pushback = get(_interp, self_val.0, "pushback")?;
            call_method(_interp, &pushback, "appendleft", &[tok.0])?;
            Ok(Value::none())
        }

        /// Push an input source onto the lexer's input source stack.
        fn push_source(
            #[positional_only] self_val: PyValueRef<'_>,
            newstream: PyValue,
            #[default(PyValue(Value::none()))] newfile: PyValue,
        ) -> Result<Value> {
            push_source_impl(_interp, self_val.0, newstream.0, newfile.0)?;
            Ok(Value::none())
        }

        /// Pop the input source stack.
        fn pop_source(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            pop_source_impl(_interp, self_val.0)?;
            Ok(Value::none())
        }

        /// Get a token from the input stream (or from stack if it's nonempty)
        fn get_token(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            get_token_impl(_interp, self_val.0)
        }

        fn read_token(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            read_token_impl(_interp, self_val.0)
        }

        /// Hook called on a filename to be sourced.
        fn sourcehook(
            #[positional_only] self_val: PyValueRef<'_>,
            newfile: PyValue,
        ) -> Result<Value> {
            sourcehook_impl(_interp, self_val.0, newfile.0)
        }

        /// Emit a C-compiler-like, Emacs-friendly error-message leader.
        fn error_leader(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(PyValue(Value::none()))] infile: PyValue,
            #[default(PyValue(Value::none()))] lineno: PyValue,
        ) -> Result<Value> {
            let infile = if infile.0.is_none() {
                get(_interp, self_val.0, "infile")?
            } else {
                infile.0
            };
            let lineno = if lineno.0.is_none() {
                get(_interp, self_val.0, "lineno")?
            } else {
                lineno.0
            };
            let text = percent_format(_interp, "\"%s\", line %d: ", vec![infile, lineno])?;
            Ok(Value::string(text))
        }

        fn __iter__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            Ok(self_val.0.clone())
        }

        fn __next__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let token = call_method(_interp, self_val.0, "get_token", &[])?;
            let eof = get(_interp, self_val.0, "eof")?;
            if py_cmp(_interp, &token, BinaryOp::Eq, &eof)? {
                return Err(PyError::named("StopIteration", String::new()));
            }
            Ok(token)
        }
    }

    /// Split the string *s* using shell-like syntax.
    fn split(
        s: PyValue,
        #[default(PyValue(Value::bool_(false)))] comments: PyValue,
        #[default(PyValue(Value::bool_(true)))] posix: PyValue,
    ) -> Result<Value> {
        if s.0.is_none() {
            return Err(value_error("s argument must not be None"));
        }
        let cls = module_attr(_interp, "shlex")?;
        let lex = _interp.call_function_expanded(
            cls,
            &[
                ExpandedCallArg { name: None, value: s.0 },
                ExpandedCallArg { name: Some("posix".to_string()), value: posix.0 },
            ],
        )?;
        _interp.assign_attr(lex.clone(), "whitespace_split", Value::bool_(true))?;
        if !_interp.truthy_value(&comments.0)? {
            _interp.assign_attr(lex.clone(), "commenters", Value::string(""))?;
        }
        Ok(Value::list(_interp.collect_iterable(&lex)?))
    }

    /// Return a shell-escaped string from *split_command*.
    fn join(split_command: PyValue) -> Result<Value> {
        let quote = module_attr(_interp, "quote")?;
        let items = _interp.collect_iterable(&split_command.0)?;
        let mut quoted = Vec::with_capacity(items.len());
        for item in items {
            quoted.push(call_pos(_interp, quote.clone(), &[item])?);
        }
        call_method(_interp, &Value::string(" "), "join", &[Value::list(quoted)])
    }

    /// Return a shell-escaped version of the string *s*.
    fn quote(s: PyValue) -> Result<Value> {
        quote_value(_interp, &s.0)
    }

    fn _print_tokens(lexer: PyValue) -> Result<Value> {
        loop {
            let token = call_method(_interp, &lexer.0, "get_token", &[])?;
            if !_interp.truthy_value(&token)? {
                break;
            }
            let repr = render_value_repr(_interp, &token)?;
            py_print(_interp, format!("Token: {repr}"))?;
        }
        Ok(Value::none())
    }
}
