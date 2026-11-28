//! The lazy `tokenize(readline)` and `generate_tokens(readline)` generators.
//!
//! CPython 3.12 drives its C tokenizer from `readline`; here the stateful
//! native lexer (`super::lexer`) is fed one physical line at a time, and the
//! tokens it produces are handed out one by one, so `readline` is only called
//! as the consumer iterates.

use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use super::lexer::{LexError, LexToken, StatefulLexer};
use super::{
    ExpandedCallArg, Interpreter, PyError, Result, TOK_ENCODING, Value, ValueKind, call_method,
    call_pos, instantiate_exception, lookup_exc_class, module_global, token_error,
    token_info_class, type_error,
};
use crate::interpreter::{ProviderIterator, builtin_data_backing, is_stop_iteration_error};

/// Where the physical lines come from.
enum Source {
    /// `generate_tokens`: `readline()` returns `str`.
    Text,
    /// `tokenize`: `readline()` returns `bytes`, decoded with the detected
    /// encoding.
    Bytes(BytesSource),
}

struct BytesSource {
    /// `detect_encoding` has run (and the `ENCODING` token was handed out).
    started: bool,
    /// The encoding `detect_encoding` reported, `utf-8-sig` mapped to `utf-8`
    /// (the BOM is already stripped from the lines it consumed).
    encoding: Option<Value>,
    /// The lines `detect_encoding` consumed, replayed ahead of `readline`.
    consumed: VecDeque<Value>,
}

struct Stream {
    readline: Value,
    source: Source,
    lexer: StatefulLexer,
    /// Tokens the lexer produced that have not been handed out yet.
    pending: VecDeque<LexToken>,
    /// An error found on the line just fed: raised once `pending` is drained.
    deferred: Option<LexError>,
    /// The lexer was told about EOF or failed: nothing more to produce.
    finished: bool,
}

/// `tokenize(readline)` (`bytes_mode`) / `generate_tokens(readline)`: nothing
/// runs until the first `next()`.
pub(super) fn make_stream(readline: Value, bytes_mode: bool) -> Value {
    let source = if bytes_mode {
        Source::Bytes(BytesSource {
            started: false,
            encoding: None,
            consumed: VecDeque::new(),
        })
    } else {
        Source::Text
    };
    let state = Stream {
        readline,
        source,
        lexer: StatefulLexer::for_tokenize(),
        pending: VecDeque::new(),
        deferred: None,
        finished: false,
    };
    Value::generator(Box::new(ProviderIterator::new(
        Box::new(Some(Some(state))),
        advance,
        "generator",
        "generator",
        None,
    )))
}

fn advance(
    interp: &mut Interpreter,
    state_rc: &Rc<RefCell<Box<dyn Any>>>,
) -> Result<Option<Value>> {
    let taken = state_rc
        .borrow_mut()
        .downcast_mut::<Option<Option<Stream>>>()
        .map(Option::take);
    let mut st = match taken {
        Some(Some(Some(st))) => st,
        // Exhausted, or closed by an earlier exception.
        Some(Some(None)) => {
            restore(state_rc, None);
            return Ok(None);
        }
        Some(None) => {
            return Err(PyError::named("ValueError", "generator already executing"));
        }
        None => {
            return Err(PyError::runtime(
                "internal: tokenize state corrupted".to_string(),
            ));
        }
    };
    let result = step(interp, &mut st);
    let keep = matches!(result, Ok(Some(_)));
    restore(state_rc, keep.then_some(st));
    result.map_err(|err| interp.pep479_wrap_stop_iteration(err))
}

fn restore(state_rc: &Rc<RefCell<Box<dyn Any>>>, st: Option<Stream>) {
    if let Some(slot) = state_rc
        .borrow_mut()
        .downcast_mut::<Option<Option<Stream>>>()
    {
        *slot = Some(st);
    }
}

/// Produce the next `TokenInfo`, or `None` at the end of the stream.
fn step(interp: &mut Interpreter, st: &mut Stream) -> Result<Option<Value>> {
    if let Source::Bytes(bytes) = &mut st.source
        && !bytes.started
    {
        bytes.started = true;
        return start_bytes(interp, &st.readline, bytes).map(Some);
    }
    loop {
        if let Some(token) = st.pending.pop_front() {
            return make_token_info(interp, &token).map(Some);
        }
        if let Some(err) = st.deferred.take() {
            st.finished = true;
            return Err(lex_error(&st.lexer, err));
        }
        if st.finished {
            return Ok(None);
        }
        let line = read_line(interp, &st.readline, &mut st.source)?;
        let produced = match line {
            Some(text) => st.lexer.feed_line(&text),
            None => {
                st.finished = true;
                st.lexer.finish()
            }
        };
        match produced {
            Ok(tokens) => {
                st.pending.extend(tokens);
                st.deferred = st.lexer.take_deferred_error();
            }
            Err(err) => {
                st.finished = true;
                return Err(lex_error(&st.lexer, err));
            }
        }
    }
}

/// The first step of `tokenize()`: `detect_encoding`, then the `ENCODING`
/// token.
fn start_bytes(
    interp: &mut Interpreter,
    readline: &Value,
    bytes: &mut BytesSource,
) -> Result<Value> {
    let detect = module_global(interp, "detect_encoding")?;
    let detected = call_pos(interp, detect, std::slice::from_ref(readline))?;
    let pair = interp.unpack_exact_values(&detected, 2)?;
    let consumed = interp.collect_iterable(&pair[1])?;
    bytes.consumed = consumed.into();
    let mut encoding = pair[0].clone();
    if encoding.as_str() == Some("utf-8-sig") {
        // BOM will already have been stripped.
        encoding = Value::string("utf-8");
    }
    bytes.encoding = Some(encoding.clone());
    let class = token_info_class()?;
    let zero = Value::tuple(vec![Value::int(0), Value::int(0)]);
    call_pos(
        interp,
        class,
        &[
            Value::int(TOK_ENCODING),
            encoding,
            zero.clone(),
            zero,
            Value::string(""),
        ],
    )
}

/// The `str` text of an exact `str` or a `str` subclass instance.
fn str_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    builtin_data_backing(value)?.as_str().map(str::to_string)
}

/// One physical line, decoded; `None` at EOF (`''` / `b''` or `StopIteration`).
fn read_line(
    interp: &mut Interpreter,
    readline: &Value,
    source: &mut Source,
) -> Result<Option<String>> {
    let value = match source {
        Source::Bytes(bytes) if !bytes.consumed.is_empty() => {
            bytes.consumed.pop_front().unwrap_or_else(Value::none)
        }
        _ => match call_pos(interp, readline.clone(), &[]) {
            Ok(value) => value,
            Err(err) if is_stop_iteration_error(&err) => return Ok(None),
            Err(err) => return Err(err),
        },
    };
    match source {
        Source::Text => match str_text(&value) {
            Some(text) if text.is_empty() => Ok(None),
            Some(text) => Ok(Some(text)),
            None => Err(type_error("readline() returned a non-string object")),
        },
        Source::Bytes(bytes) => {
            let ValueKind::Bytes(raw) = value.kind() else {
                return Err(type_error("readline() returned a non-bytes object"));
            };
            if raw.is_empty() {
                return Ok(None);
            }
            let encoding = bytes.encoding.clone().unwrap_or_else(Value::none);
            if encoding.as_str() == Some("utf-8") {
                return Ok(Some(String::from_utf8_lossy(raw).into_owned()));
            }
            let decoded = call_method(
                interp,
                &value,
                "decode",
                &[encoding, Value::string("replace")],
            )?;
            Ok(str_text(&decoded))
        }
    }
}

/// `TokenInfo(type, string, (srow, scol), (erow, ecol), line)`.
fn make_token_info(interp: &mut Interpreter, token: &LexToken) -> Result<Value> {
    let class = token_info_class()?;
    let args = [
        ExpandedCallArg {
            name: None,
            value: Value::int(token.tok_type),
        },
        ExpandedCallArg {
            name: None,
            value: Value::string(token.string.as_str()),
        },
        ExpandedCallArg {
            name: None,
            value: Value::tuple(vec![
                Value::int(i64::from(token.start_line)),
                Value::int(i64::from(token.start_col)),
            ]),
        },
        ExpandedCallArg {
            name: None,
            value: Value::tuple(vec![
                Value::int(i64::from(token.end_line)),
                Value::int(i64::from(token.end_col)),
            ]),
        },
        ExpandedCallArg {
            name: None,
            value: Value::string(token.line.as_str()),
        },
    ];
    interp.call_function_expanded(class, &args)
}

/// `_transform_msg`: massage the C tokenizer's messages for backwards
/// compatibility with the old Python tokenizer.
fn transform_msg(msg: &str) -> String {
    if msg.contains("unterminated triple-quoted string literal") {
        "EOF in multi-line string".to_string()
    } else {
        msg.to_string()
    }
}

/// A lexer failure as CPython's `tokenize` raises it: `TokenError(msg,
/// (lineno, offset))` for a plain `SyntaxError`, the `IndentationError` /
/// `TabError` itself otherwise.
fn lex_error(lexer: &StatefulLexer, err: LexError) -> PyError {
    let (class_name, msg, line, _col, text) = match err {
        LexError::TokenError { msg, line, col } => {
            if msg == "unterminated f-string literal" {
                // Reported at the start of the f-string, naming the line the
                // tokenizer was on.
                let msg = format!("{msg} (detected at line {})", lexer.line_number());
                return match lexer.open_origin() {
                    Some((line, col)) => token_error(msg, line, col + 1),
                    None => token_error(msg, line, col),
                };
            }
            return token_error(transform_msg(&msg), line, col);
        }
        LexError::IndentationError {
            msg,
            line,
            col,
            text,
        } => ("IndentationError", msg, line, col, text),
        LexError::TabError {
            msg,
            line,
            col,
            text,
        } => ("TabError", msg, line, col, text),
    };
    // CPython reports the source line without its newline and the offset just
    // past its last character.
    let text = text.strip_suffix('\n').unwrap_or(&text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let offset = text.chars().count() as i64 + 1;
    let details = Value::tuple(vec![
        Value::string("<string>"),
        Value::int(i64::from(line)),
        Value::int(offset),
        Value::string(text),
        Value::none(),
        Value::none(),
    ]);
    match lookup_exc_class(class_name) {
        Some(class) => PyError::raised(instantiate_exception(
            class,
            vec![Value::string(msg), details],
        )),
        None => PyError::named(class_name, msg),
    }
}
