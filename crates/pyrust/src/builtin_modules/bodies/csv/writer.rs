//! The `_csv.writer` record joiner — CPython 3.12.3's `join_append_data` /
//! `csv_writerow` / `csv_writerows` (`Modules/_csv.c`).

use std::rc::Rc;

use crate::error::Result;
use crate::interpreter::{Interpreter, is_stop_iteration_error, make_iterator};
use crate::value::Value;

use super::dialect::{DialectCfg, Quoting};

/// Native payload of a `_csv.writer` instance.
pub(super) struct WriterState {
    /// The output file's bound `write`, looked up once at construction.
    pub(super) write: Value,
    pub(super) dialect: Value,
    pub(super) cfg: Rc<DialectCfg>,
}

/// One record being joined (`rec` / `num_fields`).
struct Record<'a> {
    cfg: &'a DialectCfg,
    rec: String,
    num_fields: usize,
}

impl Record<'_> {
    /// `join_append(self, field, quoted)`; `field == None` is a `NULL` field.
    fn append(&mut self, field: Option<&str>, mut quoted: bool) -> Result<()> {
        let cfg = self.cfg;
        let field_empty = field.is_none_or(str::is_empty);
        if field_empty && cfg.delimiter == ' ' && cfg.skipinitialspace {
            if cfg.quoting == Quoting::None
                || (field.is_none() && matches!(cfg.quoting, Quoting::Strings | Quoting::NotNull))
            {
                return Err(super::csv_error(
                    "empty field must be quoted if delimiter is a space and skipinitialspace is true",
                ));
            }
            quoted = true;
        }

        // Copy/count field data, deciding per character whether the field
        // must be quoted and whether the character needs escaping.
        let mut body = String::with_capacity(field.map_or(0, str::len));
        for c in field.unwrap_or("").chars() {
            let is_quote = cfg.quotechar == Some(c);
            let is_escape = cfg.escapechar == Some(c);
            if c == cfg.delimiter
                || is_escape
                || is_quote
                || c == '\n'
                || c == '\r'
                || cfg.lineterminator_text.contains(c)
            {
                let mut want_escape = false;
                if cfg.quoting == Quoting::None {
                    want_escape = true;
                } else {
                    if is_quote {
                        if cfg.doublequote {
                            body.push(c);
                        } else {
                            want_escape = true;
                        }
                    } else if is_escape {
                        want_escape = true;
                    }
                    if !want_escape {
                        quoted = true;
                    }
                }
                if want_escape {
                    let Some(escapechar) = cfg.escapechar else {
                        return Err(super::csv_error("need to escape, but no escapechar set"));
                    };
                    body.push(escapechar);
                }
            }
            body.push(c);
        }

        // If this is not the first field we need a field separator.
        if self.num_fields > 0 {
            self.rec.push(cfg.delimiter);
        }
        let quotechar = cfg.quotechar.filter(|_| quoted);
        if let Some(q) = quotechar {
            self.rec.push(q);
        }
        self.rec.push_str(&body);
        if let Some(q) = quotechar {
            self.rec.push(q);
        }
        self.num_fields += 1;
        Ok(())
    }
}

/// `PyNumber_Check`: does `value`'s type fill `nb_index`, `nb_int` or
/// `nb_float`, or is it a `complex`?
fn is_number(value: &Value) -> bool {
    use crate::value::ValueKind;
    match value.kind() {
        ValueKind::Bool(_)
        | ValueKind::Int(_)
        | ValueKind::BigInt(_)
        | ValueKind::Float(_)
        | ValueKind::Complex(..) => true,
        ValueKind::PyInstance(inst) => {
            let inst = inst.borrow();
            if inst.attrs.builtin_data().is_some_and(|data| {
                matches!(
                    data.kind(),
                    ValueKind::Int(_)
                        | ValueKind::BigInt(_)
                        | ValueKind::Float(_)
                        | ValueKind::Complex(..)
                        | ValueKind::Bool(_)
                )
            }) {
                return true;
            }
            ["__index__", "__int__", "__float__"]
                .iter()
                .any(|name| crate::interpreter::lookup_class_attr(&inst.class, name).is_some())
        }
        _ => false,
    }
}

/// `csv_writerow(self, seq)`.
pub(super) fn writerow(interp: &mut Interpreter, this: &Value, seq: &Value) -> Result<Value> {
    let (write, cfg) =
        super::with_writer_state(this, |st| Ok((st.write.clone(), Rc::clone(&st.cfg))))?;
    let iter = match make_iterator(interp, seq) {
        Ok(iter) => iter,
        Err(err) if err.class_name_is("TypeError") => {
            return Err(super::csv_error(format!(
                "iterable expected, not {}",
                super::type_name(seq)
            )));
        }
        Err(err) => return Err(err),
    };

    // Join all fields in internal buffer.
    let mut record = Record {
        cfg: &cfg,
        rec: String::new(),
        num_fields: 0,
    };
    let mut null_field = false;
    loop {
        let field = match interp.call_next(&iter, None) {
            Ok(field) => field,
            Err(err) if is_stop_iteration_error(&err) => break,
            Err(err) => return Err(err),
        };
        let quoted = match cfg.quoting {
            Quoting::NonNumeric => !is_number(&field),
            Quoting::All => true,
            Quoting::Strings => super::str_text(&field).is_some(),
            Quoting::NotNull => !field.is_none(),
            Quoting::Minimal | Quoting::None => false,
        };
        null_field = field.is_none();
        if null_field {
            record.append(None, quoted)?;
        } else if let Some(text) = super::str_text(&field) {
            record.append(Some(&text), quoted)?;
        } else {
            let text = interp.render_value_as_str(&field)?;
            record.append(Some(&text), quoted)?;
        }
    }

    if record.num_fields > 0 && record.rec.is_empty() {
        if cfg.quoting == Quoting::None
            || (null_field && matches!(cfg.quoting, Quoting::Strings | Quoting::NotNull))
        {
            return Err(super::csv_error("single empty field record must be quoted"));
        }
        record.num_fields -= 1;
        record.append(None, true)?;
    }

    // Add line terminator.
    record.rec.push_str(&cfg.lineterminator_text);
    let line = Value::string(record.rec);
    super::call_value(interp, write, &[line])
}

/// `csv_writerows(self, seqseq)`.
pub(super) fn writerows(interp: &mut Interpreter, this: &Value, rows: &Value) -> Result<Value> {
    let iter = make_iterator(interp, rows)?;
    loop {
        let row = match interp.call_next(&iter, None) {
            Ok(row) => row,
            Err(err) if is_stop_iteration_error(&err) => break,
            Err(err) => return Err(err),
        };
        writerow(interp, this, &row)?;
    }
    Ok(Value::none())
}
