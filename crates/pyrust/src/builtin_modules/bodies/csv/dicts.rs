//! `csv.DictReader` / `csv.DictWriter` — CPython 3.12.3 `Lib/csv.py`.
//!
//! Like the pure-Python classes, every piece of state lives in the instance
//! `__dict__` and is re-read through `self` (including the `fieldnames`
//! property) in the same order CPython's code reads it.

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::{Interpreter, is_stop_iteration_error, render_value_repr};
use crate::value::{Value, ValueKind};

fn get(interp: &mut Interpreter, this: &Value, name: &str) -> Result<Value> {
    interp.get_attr(this, name)
}

fn set(interp: &mut Interpreter, this: &Value, name: &str, value: Value) -> Result<()> {
    interp.assign_attr(this.clone(), name, value)
}

fn py_eq(interp: &mut Interpreter, a: &Value, b: &Value) -> Result<bool> {
    let res = interp.eval_binary(a.clone(), BinaryOp::Eq, b.clone())?;
    interp.truthy_value(&res)
}

fn py_len(interp: &mut Interpreter, value: &Value) -> Result<i64> {
    let len = super::call_builtin(interp, "len", std::slice::from_ref(value))?;
    len.as_int()
        .ok_or_else(|| PyError::runtime("len() did not return an int".to_string()))
}

/// `value[start:]`.
fn slice_from(interp: &mut Interpreter, value: &Value, start: i64) -> Result<Value> {
    let slice = super::call_builtin(interp, "slice", &[Value::int(start), Value::none()])?;
    interp.eval_index(value, slice)
}

/// `if fieldnames is not None and iter(fieldnames) is fieldnames:
///      fieldnames = list(fieldnames)`
fn materialize_fieldnames(interp: &mut Interpreter, fieldnames: Value) -> Result<Value> {
    if fieldnames.is_none() {
        return Ok(fieldnames);
    }
    let iter = super::call_builtin(interp, "iter", std::slice::from_ref(&fieldnames))?;
    if iter.is_identical_to(&fieldnames) {
        super::call_builtin(interp, "list", &[fieldnames])
    } else {
        Ok(fieldnames)
    }
}

/// `reader(f, dialect, *args, **kwds)` / `writer(...)` call arguments.
fn factory_args(
    f: Value,
    dialect: Value,
    args: &[Value],
    kwds: &[(String, Value)],
) -> Vec<crate::interpreter::ExpandedCallArg> {
    let mut positional = vec![f, dialect];
    positional.extend(args.iter().cloned());
    super::expand_args(&positional, kwds)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dict_reader_init(
    interp: &mut Interpreter,
    this: &Value,
    f: Value,
    fieldnames: Value,
    restkey: Value,
    restval: Value,
    dialect: Value,
    args: &[Value],
    kwds: &[(String, Value)],
) -> Result<()> {
    let fieldnames = materialize_fieldnames(interp, fieldnames)?;
    set(interp, this, "_fieldnames", fieldnames)?; // list of keys for the dict
    set(interp, this, "restkey", restkey)?; // key to catch long rows
    set(interp, this, "restval", restval)?; // default value for short rows
    let reader = super::make_reader(interp, &factory_args(f, dialect.clone(), args, kwds))?;
    set(interp, this, "reader", reader)?;
    set(interp, this, "dialect", dialect)?;
    set(interp, this, "line_num", Value::int(0))?;
    Ok(())
}

/// The `DictReader.fieldnames` property getter.
pub(super) fn dict_reader_fieldnames(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let fieldnames = get(interp, this, "_fieldnames")?;
    if fieldnames.is_none() {
        let reader = get(interp, this, "reader")?;
        match interp.call_next(&reader, None) {
            Ok(row) => set(interp, this, "_fieldnames", row)?,
            Err(err) if is_stop_iteration_error(&err) => {}
            Err(err) => return Err(err),
        }
    }
    let reader = get(interp, this, "reader")?;
    let line_num = get(interp, &reader, "line_num")?;
    set(interp, this, "line_num", line_num)?;
    get(interp, this, "_fieldnames")
}

fn is_empty_row(interp: &mut Interpreter, row: &Value) -> Result<bool> {
    if let ValueKind::List(items) = row.kind() {
        return Ok(items.is_empty());
    }
    py_eq(interp, row, &Value::list(Vec::new()))
}

/// `DictReader.__next__`.
pub(super) fn dict_reader_next(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let line_num = get(interp, this, "line_num")?;
    let at_start = match line_num.kind() {
        ValueKind::Int(n) => n == 0,
        _ => py_eq(interp, &line_num, &Value::int(0))?,
    };
    if at_start {
        // Used only for its side effect.
        get(interp, this, "fieldnames")?;
    }
    let reader = get(interp, this, "reader")?;
    let mut row = interp.call_next(&reader, None)?;
    let line_num = get(interp, &reader, "line_num")?;
    set(interp, this, "line_num", line_num)?;

    // unlike the basic reader, we prefer not to return blanks,
    // because we will typically wind up with a dict full of None
    // values
    while is_empty_row(interp, &row)? {
        let reader = get(interp, this, "reader")?;
        row = interp.call_next(&reader, None)?;
    }
    let fieldnames = get(interp, this, "fieldnames")?;
    let pairs = super::call_builtin(interp, "zip", &[fieldnames, row.clone()])?;
    let d = super::call_builtin(interp, "dict", &[pairs])?;
    let fieldnames = get(interp, this, "fieldnames")?;
    let lf = py_len(interp, &fieldnames)?;
    let lr = py_len(interp, &row)?;
    if lf < lr {
        let restkey = get(interp, this, "restkey")?;
        let rest = slice_from(interp, &row, lf)?;
        interp.dict_set_value(&d, restkey, rest)?;
    } else if lf > lr {
        let fieldnames = get(interp, this, "fieldnames")?;
        let missing = slice_from(interp, &fieldnames, lr)?;
        for key in interp.collect_iterable(&missing)? {
            let restval = get(interp, this, "restval")?;
            interp.dict_set_value(&d, key, restval)?;
        }
    }
    Ok(d)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dict_writer_init(
    interp: &mut Interpreter,
    this: &Value,
    f: Value,
    fieldnames: Value,
    restval: Value,
    extrasaction: Value,
    dialect: Value,
    args: &[Value],
    kwds: &[(String, Value)],
) -> Result<()> {
    let fieldnames = materialize_fieldnames(interp, fieldnames)?;
    set(interp, this, "fieldnames", fieldnames)?; // list of keys for the dict
    set(interp, this, "restval", restval)?; // for writing short dicts
    let extrasaction = super::call_method(interp, &extrasaction, "lower", &[])?;
    let allowed = Value::tuple(vec![Value::string("raise"), Value::string("ignore")]);
    let found = interp.eval_in(allowed, extrasaction.clone())?;
    if !interp.truthy_value(&found)? {
        let text = interp.render_value_as_str(&extrasaction)?;
        return Err(PyError::named(
            "ValueError",
            format!("extrasaction ({text}) must be 'raise' or 'ignore'"),
        ));
    }
    set(interp, this, "extrasaction", extrasaction)?;
    let writer = super::make_writer(interp, &factory_args(f, dialect, args, kwds))?;
    set(interp, this, "writer", writer)?;
    Ok(())
}

/// `DictWriter.writeheader`.
pub(super) fn dict_writer_writeheader(interp: &mut Interpreter, this: &Value) -> Result<Value> {
    let first = get(interp, this, "fieldnames")?;
    let second = get(interp, this, "fieldnames")?;
    let pairs = super::call_builtin(interp, "zip", &[first, second])?;
    let header = super::call_builtin(interp, "dict", &[pairs])?;
    super::call_method(interp, this, "writerow", &[header])
}

/// `DictWriter._dict_to_list`: the row's values in `fieldnames` order.
pub(super) fn dict_writer_dict_to_list(
    interp: &mut Interpreter,
    this: &Value,
    rowdict: &Value,
) -> Result<Value> {
    let extrasaction = get(interp, this, "extrasaction")?;
    if py_eq(interp, &extrasaction, &Value::string("raise"))? {
        let keys = super::call_method(interp, rowdict, "keys", &[])?;
        let fieldnames = get(interp, this, "fieldnames")?;
        let wrong_fields = interp.eval_binary(keys, BinaryOp::Sub, fieldnames)?;
        if interp.truthy_value(&wrong_fields)? {
            let mut reprs = Vec::new();
            for field in interp.collect_iterable(&wrong_fields)? {
                reprs.push(render_value_repr(interp, &field)?);
            }
            return Err(PyError::named(
                "ValueError",
                format!(
                    "dict contains fields not in fieldnames: {}",
                    reprs.join(", ")
                ),
            ));
        }
    }
    // `(rowdict.get(key, self.restval) for key in self.fieldnames)`
    let fieldnames = get(interp, this, "fieldnames")?;
    let mut values = Vec::new();
    for key in interp.collect_iterable(&fieldnames)? {
        let restval = get(interp, this, "restval")?;
        values.push(super::call_method(interp, rowdict, "get", &[key, restval])?);
    }
    super::call_builtin(interp, "iter", &[Value::list(values)])
}
