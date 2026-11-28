//! `Untokenizer` and `untokenize`, transcribed from CPython 3.12.3's
//! `Lib/tokenize.py`.
//!
//! Like the Python class, the state lives in instance attributes (`tokens`,
//! `prev_row`, `prev_col`, `prev_type`, `encoding`), and the steps go back
//! through `self` (`add_whitespace`, `escape_brackets`, `compat`), so a
//! subclass overriding any of them behaves as in CPython.

use super::lexer::{
    TOK_DEDENT, TOK_ENDMARKER, TOK_FSTRING_END, TOK_FSTRING_MIDDLE, TOK_FSTRING_START, TOK_INDENT,
    TOK_NAME, TOK_NEWLINE, TOK_NL, TOK_NUMBER, TOK_STRING,
};
use super::{
    BinaryOp, Interpreter, PyError, Result, TOK_ENCODING, Value, binary, builtin, call_method,
    compare, module_global, str_of, truthy,
};
use crate::interpreter::{is_stop_iteration_error, make_iterator};

// ---------------------------------------------------------------------------
// Small generic helpers
// ---------------------------------------------------------------------------

/// `tok_type == kind`.
fn is_type(interp: &mut Interpreter, tok_type: &Value, kind: i64) -> Result<bool> {
    match tok_type.as_int() {
        Some(n) => Ok(n == kind),
        None => compare(interp, tok_type, BinaryOp::Eq, &Value::int(kind)),
    }
}

/// `tok_type in (a, b)`.
fn is_type_in(interp: &mut Interpreter, tok_type: &Value, kinds: &[i64]) -> Result<bool> {
    for &kind in kinds {
        if is_type(interp, tok_type, kind)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `len(value)`.
fn py_len(interp: &mut Interpreter, value: &Value) -> Result<i64> {
    let len = builtin(interp, "len", std::slice::from_ref(value))?;
    Ok(len.as_int().unwrap_or(0))
}

/// The next item of `iterator`, or `None` once it is exhausted.
fn next_item(interp: &mut Interpreter, iterator: &Value) -> Result<Option<Value>> {
    match interp.call_next(iterator, None) {
        Ok(item) => Ok(Some(item)),
        Err(err) if is_stop_iteration_error(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

/// `obj[index]`.
fn item(interp: &mut Interpreter, obj: &Value, index: i64) -> Result<Value> {
    crate::builtin_modules::operator::getitem_value(interp, obj.clone(), Value::int(index))
}

/// `obj[:stop]`.
fn head(interp: &mut Interpreter, obj: &Value, stop: i64) -> Result<Value> {
    let bounds = crate::interpreter::make_slice_value(None, Some(Value::int(stop)), None);
    crate::builtin_modules::operator::getitem_value(interp, obj.clone(), bounds)
}

/// `self.<name>`.
fn attr(interp: &mut Interpreter, this: &Value, name: &str) -> Result<Value> {
    interp.get_attr(this, name)
}

/// `self.<name> = value`.
fn set_attr(interp: &mut Interpreter, this: &Value, name: &str, value: Value) -> Result<()> {
    interp.assign_attr(this.clone(), name, value)
}

/// `tokens.append(value)`.
fn append(interp: &mut Interpreter, tokens: &Value, value: Value) -> Result<()> {
    let is_list = tokens.as_list().is_some();
    if is_list {
        return tokens.list_push(value);
    }
    call_method(interp, tokens, "append", &[value])?;
    Ok(())
}

/// `"".join(parts)`.
fn join(interp: &mut Interpreter, parts: &Value) -> Result<Value> {
    call_method(
        interp,
        &Value::string(""),
        "join",
        std::slice::from_ref(parts),
    )
}

fn index_error_pop() -> PyError {
    PyError::named("IndexError", "pop from empty list")
}

// ---------------------------------------------------------------------------
// Untokenizer
// ---------------------------------------------------------------------------

/// `Untokenizer.__init__`.
pub(super) fn init(interp: &mut Interpreter, this: &Value) -> Result<()> {
    set_attr(interp, this, "tokens", Value::list(Vec::new()))?;
    set_attr(interp, this, "prev_row", Value::int(1))?;
    set_attr(interp, this, "prev_col", Value::int(0))?;
    set_attr(interp, this, "prev_type", Value::none())?;
    set_attr(interp, this, "encoding", Value::none())?;
    Ok(())
}

/// `Untokenizer.add_whitespace(start)`.
pub(super) fn add_whitespace(interp: &mut Interpreter, this: &Value, start: &Value) -> Result<()> {
    let pair = interp.unpack_exact_values(start, 2)?;
    let (row, col) = (&pair[0], &pair[1]);
    let prev_row = attr(interp, this, "prev_row")?;
    let prev_col = attr(interp, this, "prev_col")?;
    let before = compare(interp, row, BinaryOp::Lt, &prev_row)?
        || (compare(interp, row, BinaryOp::Eq, &prev_row)?
            && compare(interp, col, BinaryOp::Lt, &prev_col)?);
    if before {
        let row = str_of(interp, row)?;
        let col = str_of(interp, col)?;
        let prev_row = str_of(interp, &prev_row)?;
        let prev_col = str_of(interp, &prev_col)?;
        return Err(PyError::named(
            "ValueError",
            format!("start ({row},{col}) precedes previous end ({prev_row},{prev_col})"),
        ));
    }
    let row_offset = binary(interp, row, BinaryOp::Sub, &prev_row)?;
    let mut prev_col = prev_col;
    if truthy(interp, &row_offset)? {
        let tokens = attr(interp, this, "tokens")?;
        let backslashes = binary(interp, &Value::string("\\\n"), BinaryOp::Mul, &row_offset)?;
        append(interp, &tokens, backslashes)?;
        set_attr(interp, this, "prev_col", Value::int(0))?;
        prev_col = Value::int(0);
    }
    let col_offset = binary(interp, col, BinaryOp::Sub, &prev_col)?;
    if truthy(interp, &col_offset)? {
        let tokens = attr(interp, this, "tokens")?;
        let spaces = binary(interp, &Value::string(" "), BinaryOp::Mul, &col_offset)?;
        append(interp, &tokens, spaces)?;
    }
    Ok(())
}

/// `Untokenizer.escape_brackets` on a `str`.
fn escape_brackets_text(text: &str) -> String {
    let mut characters: Vec<char> = Vec::with_capacity(text.len() + 2);
    let mut consume_until_next_bracket = false;
    for character in text.chars() {
        if character == '}' {
            if consume_until_next_bracket {
                consume_until_next_bracket = false;
            } else {
                characters.push(character);
            }
        }
        if character == '{' {
            // n_backslashes = sum(1 for char in takewhile("\\".__eq__,
            //                                             characters[-2::-1]))
            let n_backslashes = match characters.len() {
                0 | 1 => 0,
                len => characters[..len - 1]
                    .iter()
                    .rev()
                    .take_while(|&&c| c == '\\')
                    .count(),
            };
            if n_backslashes % 2 == 0 {
                characters.push(character);
            } else {
                consume_until_next_bracket = true;
            }
        }
        characters.push(character);
    }
    characters.into_iter().collect()
}

/// `Untokenizer.escape_brackets(token)`.
pub(super) fn escape_brackets(interp: &mut Interpreter, token: &Value) -> Result<Value> {
    if let Some(text) = token.as_str() {
        return Ok(Value::string(escape_brackets_text(text)));
    }
    // `for character in token`: anything iterable of one-character strings.
    let items = interp.collect_iterable(token)?;
    let mut text = String::new();
    for character in &items {
        text.push_str(character.as_str().unwrap_or_default());
    }
    Ok(Value::string(escape_brackets_text(&text)))
}

/// `'{' in text or '}' in text`.
fn has_bracket(interp: &mut Interpreter, token: &Value) -> Result<bool> {
    if let Some(text) = token.as_str() {
        return Ok(text.contains(['{', '}']));
    }
    for bracket in ["{", "}"] {
        if compare(interp, &Value::string(bracket), BinaryOp::In, token)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `Untokenizer.untokenize(iterable)`.
pub(super) fn untokenize_method(
    interp: &mut Interpreter,
    this: &Value,
    iterable: &Value,
) -> Result<Value> {
    let it = make_iterator(interp, iterable)?;
    let mut indents: Vec<Value> = Vec::new();
    let mut startline = false;
    while let Some(t) = next_item(interp, &it)? {
        if py_len(interp, &t)? == 2 {
            let compat = attr(interp, this, "compat")?;
            super::call_pos(interp, compat, &[t, it.clone()])?;
            break;
        }
        let parts = interp.unpack_exact_values(&t, 5)?;
        let tok_type = parts[0].clone();
        let mut token = parts[1].clone();
        let start = parts[2].clone();
        let mut end = parts[3].clone();
        if is_type(interp, &tok_type, TOK_ENCODING)? {
            set_attr(interp, this, "encoding", token)?;
            continue;
        }
        if is_type(interp, &tok_type, TOK_ENDMARKER)? {
            break;
        }
        if is_type(interp, &tok_type, TOK_INDENT)? {
            indents.push(token);
            continue;
        } else if is_type(interp, &tok_type, TOK_DEDENT)? {
            indents.pop().ok_or_else(index_error_pop)?;
            let end_pair = interp.unpack_exact_values(&end, 2)?;
            set_attr(interp, this, "prev_row", end_pair[0].clone())?;
            set_attr(interp, this, "prev_col", end_pair[1].clone())?;
            continue;
        } else if is_type_in(interp, &tok_type, &[TOK_NEWLINE, TOK_NL])? {
            startline = true;
        } else if startline && !indents.is_empty() {
            let indent = indents[indents.len() - 1].clone();
            let column = item(interp, &start, 1)?;
            let indent_len = py_len(interp, &indent)?;
            if compare(interp, &column, BinaryOp::Ge, &Value::int(indent_len))? {
                let tokens = attr(interp, this, "tokens")?;
                append(interp, &tokens, indent)?;
                set_attr(interp, this, "prev_col", Value::int(indent_len))?;
            }
            startline = false;
        } else if is_type(interp, &tok_type, TOK_FSTRING_MIDDLE)? {
            if has_bracket(interp, &token)? {
                let escape = attr(interp, this, "escape_brackets")?;
                token = super::call_pos(interp, escape, &[token])?;
                let lines = call_method(interp, &token, "splitlines", &[])?;
                let last_line = item(interp, &lines, -1)?;
                let end_pair = interp.unpack_exact_values(&end, 2)?;
                let doubled = call_method(interp, &last_line, "count", &[Value::string("{{")])?;
                let closing = call_method(interp, &last_line, "count", &[Value::string("}}")])?;
                let extra_chars = binary(interp, &doubled, BinaryOp::Add, &closing)?;
                let end_col = binary(interp, &end_pair[1], BinaryOp::Add, &extra_chars)?;
                end = Value::tuple(vec![end_pair[0].clone(), end_col]);
            }
        } else if is_type_in(interp, &tok_type, &[TOK_STRING, TOK_FSTRING_START])? {
            let prev_type = attr(interp, this, "prev_type")?;
            if is_type_in(interp, &prev_type, &[TOK_STRING, TOK_FSTRING_END])? {
                let tokens = attr(interp, this, "tokens")?;
                append(interp, &tokens, Value::string(" "))?;
            }
        }

        let add = attr(interp, this, "add_whitespace")?;
        super::call_pos(interp, add, &[start])?;
        let tokens = attr(interp, this, "tokens")?;
        append(interp, &tokens, token)?;
        let end_pair = interp.unpack_exact_values(&end, 2)?;
        set_attr(interp, this, "prev_row", end_pair[0].clone())?;
        set_attr(interp, this, "prev_col", end_pair[1].clone())?;
        if is_type_in(interp, &tok_type, &[TOK_NEWLINE, TOK_NL])? {
            let prev_row = attr(interp, this, "prev_row")?;
            let next_row = binary(interp, &prev_row, BinaryOp::Add, &Value::int(1))?;
            set_attr(interp, this, "prev_row", next_row)?;
            set_attr(interp, this, "prev_col", Value::int(0))?;
        }
        set_attr(interp, this, "prev_type", tok_type)?;
    }
    let tokens = attr(interp, this, "tokens")?;
    join(interp, &tokens)
}

/// `Untokenizer.compat(token, iterable)`.
pub(super) fn compat(
    interp: &mut Interpreter,
    this: &Value,
    token: &Value,
    iterable: &Value,
) -> Result<()> {
    let mut indents: Vec<Value> = Vec::new();
    let toks = attr(interp, this, "tokens")?;
    let first_type = item(interp, token, 0)?;
    let mut startline = is_type_in(interp, &first_type, &[TOK_NEWLINE, TOK_NL])?;
    let mut prevstring = false;
    let mut in_fstring: i64 = 0;

    let mut first = Some(token.clone());
    let mut rest: Option<Value> = None;
    loop {
        let tok = match first.take() {
            Some(tok) => tok,
            None => {
                if rest.is_none() {
                    rest = Some(make_iterator(interp, iterable)?);
                }
                let Some(iterator) = rest.as_ref() else {
                    break;
                };
                match next_item(interp, iterator)? {
                    Some(tok) => tok,
                    None => break,
                }
            }
        };
        // toknum, tokval = tok[:2]
        let head_pair = head(interp, &tok, 2)?;
        let pair = interp.unpack_exact_values(&head_pair, 2)?;
        let toknum = pair[0].clone();
        let mut tokval = pair[1].clone();
        if is_type(interp, &toknum, TOK_ENCODING)? {
            set_attr(interp, this, "encoding", tokval)?;
            continue;
        }

        if is_type_in(interp, &toknum, &[TOK_NAME, TOK_NUMBER])? {
            tokval = binary(interp, &tokval, BinaryOp::Add, &Value::string(" "))?;
        }

        // Insert a space between two consecutive strings
        if is_type(interp, &toknum, TOK_STRING)? {
            if prevstring {
                tokval = binary(interp, &Value::string(" "), BinaryOp::Add, &tokval)?;
            }
            prevstring = true;
        } else {
            prevstring = false;
        }

        if is_type(interp, &toknum, TOK_FSTRING_START)? {
            in_fstring += 1;
        } else if is_type(interp, &toknum, TOK_FSTRING_END)? {
            in_fstring -= 1;
        }
        if is_type(interp, &toknum, TOK_INDENT)? {
            indents.push(tokval);
            continue;
        } else if is_type(interp, &toknum, TOK_DEDENT)? {
            indents.pop().ok_or_else(index_error_pop)?;
            continue;
        } else if is_type_in(interp, &toknum, &[TOK_NEWLINE, TOK_NL])? {
            startline = true;
        } else if startline && !indents.is_empty() {
            append(interp, &toks, indents[indents.len() - 1].clone())?;
            startline = false;
        } else if is_type(interp, &toknum, TOK_FSTRING_MIDDLE)? {
            let escape = attr(interp, this, "escape_brackets")?;
            tokval = super::call_pos(interp, escape, &[tokval])?;
        }

        // Insert a space between two consecutive brackets if we are in an
        // f-string
        if in_fstring != 0
            && matches!(tokval.as_str(), Some("{" | "}"))
            && last_token_equals(interp, this, &tokval)?
        {
            tokval = binary(interp, &Value::string(" "), BinaryOp::Add, &tokval)?;
        }

        // Insert a space between two consecutive f-strings
        if is_type_in(interp, &toknum, &[TOK_STRING, TOK_FSTRING_START])? {
            let prev_type = attr(interp, this, "prev_type")?;
            if is_type_in(interp, &prev_type, &[TOK_STRING, TOK_FSTRING_END])? {
                let tokens = attr(interp, this, "tokens")?;
                append(interp, &tokens, Value::string(" "))?;
            }
        }

        append(interp, &toks, tokval)?;
        set_attr(interp, this, "prev_type", toknum)?;
    }
    Ok(())
}

/// `self.tokens and self.tokens[-1] == tokval`.
fn last_token_equals(interp: &mut Interpreter, this: &Value, tokval: &Value) -> Result<bool> {
    let tokens = attr(interp, this, "tokens")?;
    if !truthy(interp, &tokens)? {
        return Ok(false);
    }
    let last = item(interp, &tokens, -1)?;
    compare(interp, &last, BinaryOp::Eq, tokval)
}

/// `untokenize(iterable)`: transform tokens back into Python source code.
/// It returns `bytes`, encoded using the `ENCODING` token (the first token
/// sequence `tokenize()` outputs), or `str` when there is none.
pub(super) fn untokenize(interp: &mut Interpreter, iterable: Value) -> Result<Value> {
    let class = module_global(interp, "Untokenizer")?;
    let ut = super::call_pos(interp, class, &[])?;
    let out = call_method(interp, &ut, "untokenize", &[iterable])?;
    let encoding = attr(interp, &ut, "encoding")?;
    if encoding.is_none() {
        return Ok(out);
    }
    call_method(interp, &out, "encode", &[encoding])
}
