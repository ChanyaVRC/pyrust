//! The `_csv.reader` parser — CPython 3.12.3's `parse_process_char` /
//! `Reader_iternext` state machine (`Modules/_csv.c`, issue #3340).
//!
//! The state machine is carried across the physical lines pulled from the
//! input iterator, so a quoted field or an escaped newline can span several
//! lines; `EOL` (`None` here) is fed after the last character of each line.

use std::rc::Rc;

use crate::error::Result;
use crate::interpreter::{Interpreter, is_stop_iteration_error};
use crate::value::Value;

use super::dialect::{DialectCfg, Quoting};

/// Native payload of a `_csv.reader` instance.
pub(super) struct ReaderState {
    pub(super) input_iter: Value,
    pub(super) dialect: Value,
    pub(super) cfg: Rc<DialectCfg>,
    pub(super) line_num: u64,
}

/// `ParserState`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    StartRecord,
    StartField,
    EscapedChar,
    InField,
    InQuotedField,
    EscapeInQuotedField,
    QuoteInQuotedField,
    EatCrnl,
    AfterEscapedCrnl,
}

struct Parser<'a> {
    cfg: &'a DialectCfg,
    state: State,
    fields: Vec<Value>,
    field: String,
    /// Length of `field` in characters (CPython's `field_len`).
    field_len: usize,
    numeric_field: bool,
    field_limit: i64,
}

#[inline]
fn is_crlf(c: Option<char>) -> bool {
    matches!(c, Some('\n' | '\r'))
}

impl<'a> Parser<'a> {
    fn new(cfg: &'a DialectCfg) -> Self {
        Parser {
            cfg,
            state: State::StartRecord,
            fields: Vec::new(),
            field: String::new(),
            field_len: 0,
            numeric_field: false,
            field_limit: super::field_limit(),
        }
    }

    /// `parse_save_field`.
    fn save_field(&mut self, interp: &mut Interpreter) -> Result<()> {
        let text = std::mem::take(&mut self.field);
        self.field_len = 0;
        let value = if self.numeric_field {
            self.numeric_field = false;
            super::call_builtin(interp, "float", &[Value::string(text)])?
        } else {
            Value::string(text)
        };
        self.fields.push(value);
        Ok(())
    }

    /// `parse_add_char`.
    fn add_char(&mut self, c: char) -> Result<()> {
        if self.field_len as i64 >= self.field_limit {
            return Err(super::csv_error(format!(
                "field larger than field limit ({})",
                self.field_limit
            )));
        }
        self.field.push(c);
        self.field_len += 1;
        Ok(())
    }

    /// Finish the current field at an end of line: `[fields]` is complete
    /// when `c` is `EOL`, otherwise the rest of the line terminator is eaten.
    fn end_line(&mut self, interp: &mut Interpreter, c: Option<char>) -> Result<()> {
        self.save_field(interp)?;
        self.state = if c.is_none() {
            State::StartRecord
        } else {
            State::EatCrnl
        };
        Ok(())
    }

    /// `parse_process_char`; `c == None` is CPython's `EOL` sentinel.
    fn process_char(&mut self, interp: &mut Interpreter, c: Option<char>) -> Result<()> {
        let cfg = self.cfg;
        let quote_on = cfg.quoting != Quoting::None;
        let is_quote = |c: Option<char>| quote_on && c.is_some() && c == cfg.quotechar;
        let is_escape = |c: Option<char>| c.is_some() && c == cfg.escapechar;
        let is_delim = |c: Option<char>| c == Some(cfg.delimiter);

        if self.state == State::StartRecord {
            if c.is_none() {
                // empty line - return []
                return Ok(());
            }
            if is_crlf(c) {
                self.state = State::EatCrnl;
                return Ok(());
            }
            // normal character - handle as START_FIELD
            self.state = State::StartField;
        }

        match self.state {
            State::StartRecord => unreachable!("handled above"),
            State::StartField => {
                // expecting field
                if is_crlf(c) || c.is_none() {
                    // save empty field - return [fields]
                    self.end_line(interp, c)?;
                } else if is_quote(c) {
                    // start quoted field
                    self.state = State::InQuotedField;
                } else if is_escape(c) {
                    // possible escaped character
                    self.state = State::EscapedChar;
                } else if c == Some(' ') && cfg.skipinitialspace {
                    // ignore spaces at start of field
                } else if is_delim(c) {
                    // save empty field
                    self.save_field(interp)?;
                } else if let Some(ch) = c {
                    // begin new unquoted field
                    if cfg.quoting == Quoting::NonNumeric {
                        self.numeric_field = true;
                    }
                    self.add_char(ch)?;
                    self.state = State::InField;
                }
            }
            State::EscapedChar => {
                if let Some(ch @ ('\n' | '\r')) = c {
                    self.add_char(ch)?;
                    self.state = State::AfterEscapedCrnl;
                } else {
                    self.add_char(c.unwrap_or('\n'))?;
                    self.state = State::InField;
                }
            }
            State::AfterEscapedCrnl if c.is_none() => {}
            // `AFTER_ESCAPED_CRNL` falls through to `IN_FIELD` for any other
            // character *without* leaving its state: an ordinary character
            // keeps waiting for the record's real line end.
            State::InField | State::AfterEscapedCrnl => {
                // in unquoted field
                if is_crlf(c) || c.is_none() {
                    // end of line - return [fields]
                    self.end_line(interp, c)?;
                } else if is_escape(c) {
                    // possible escaped character
                    self.state = State::EscapedChar;
                } else if is_delim(c) {
                    // save field - wait for new field
                    self.save_field(interp)?;
                    self.state = State::StartField;
                } else if let Some(ch) = c {
                    // normal character - save in field
                    self.add_char(ch)?;
                }
            }
            State::InQuotedField => {
                // in quoted field
                if c.is_none() {
                } else if is_escape(c) {
                    // Possible escape character
                    self.state = State::EscapeInQuotedField;
                } else if is_quote(c) {
                    self.state = if cfg.doublequote {
                        // doublequote; " represented by ""
                        State::QuoteInQuotedField
                    } else {
                        // end of quote part of field
                        State::InField
                    };
                } else if let Some(ch) = c {
                    // normal character - save in field
                    self.add_char(ch)?;
                }
            }
            State::EscapeInQuotedField => {
                self.add_char(c.unwrap_or('\n'))?;
                self.state = State::InQuotedField;
            }
            State::QuoteInQuotedField => {
                // doublequote - seen a quote in a quoted field
                if is_quote(c) {
                    // save "" as "
                    if let Some(ch) = c {
                        self.add_char(ch)?;
                    }
                    self.state = State::InQuotedField;
                } else if is_delim(c) {
                    // save field - wait for new field
                    self.save_field(interp)?;
                    self.state = State::StartField;
                } else if is_crlf(c) || c.is_none() {
                    // end of line - return [fields]
                    self.end_line(interp, c)?;
                } else if !cfg.strict {
                    if let Some(ch) = c {
                        self.add_char(ch)?;
                    }
                    self.state = State::InField;
                } else {
                    // illegal
                    return Err(super::csv_error(format!(
                        "'{}' expected after '{}'",
                        cfg.delimiter,
                        cfg.quotechar.unwrap_or('"')
                    )));
                }
            }
            State::EatCrnl => {
                if is_crlf(c) {
                } else if c.is_none() {
                    self.state = State::StartRecord;
                } else {
                    return Err(super::csv_error(
                        "new-line character seen in unquoted field - do you need to open the file with newline=''?",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// `Reader_iternext`: parse and return the next record.
pub(super) fn next_record(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let (input_iter, cfg) =
        super::with_reader_state(this, |st| Ok((st.input_iter.clone(), Rc::clone(&st.cfg))))?;
    let mut parser = Parser::new(&cfg);
    loop {
        let line = match interp.call_next(&input_iter, None) {
            Ok(line) => line,
            Err(err) if is_stop_iteration_error(&err) => {
                // End of input inside a record.
                if parser.field_len != 0 || parser.state == State::InQuotedField {
                    if cfg.strict {
                        return Err(super::csv_error("unexpected end of data"));
                    }
                    parser.save_field(interp)?;
                    break;
                }
                return Err(err);
            }
            Err(err) => return Err(err),
        };
        let Some(text) = super::str_text(&line) else {
            return Err(super::csv_error(format!(
                "iterator should return strings, not {} (the file should be opened in text mode)",
                super::type_name(&line)
            )));
        };
        super::with_reader_state_mut(this, |st| {
            st.line_num += 1;
            Ok(())
        })?;
        parser.field_limit = super::field_limit();
        for ch in text.chars() {
            parser.process_char(interp, Some(ch))?;
        }
        parser.process_char(interp, None)?;
        if parser.state == State::StartRecord {
            break;
        }
    }
    Ok(Value::list(parser.fields))
}
