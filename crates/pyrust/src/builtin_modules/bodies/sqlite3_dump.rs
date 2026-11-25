// `sqlite3.dump` submodule — 100% native Rust implementation of Python 3.12's
// `sqlite3.dump` module (issue #3251).
//
// Exposes `_iterdump(connection)` delegating to `connection.iterdump()`, plus
// the internal quoting helpers `_quote_name` and `_quote_value` for parity.
//
// Reference: <https://docs.python.org/3/library/sqlite3.html#sqlite3.Connection.iterdump>

use pyrust_derive::pyrust_module;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::builtin_args::{PyStr, PyValue};
use crate::value::Value;

pyrust_module! {
    constants {
        "__package__" => Value::string("sqlite3"),
    }

    fn _quote_name(name: PyStr<'_>) -> Result<Value> {
        let escaped = name.0.replace('"', "\"\"");
        Ok(Value::string(format!("\"{escaped}\"")))
    }

    fn _quote_value(value: PyValue) -> Result<Value> {
        let s = _interp.render_value_as_str(&value.0)?;
        let escaped = s.replace('\'', "''");
        Ok(Value::string(format!("'{escaped}'")))
    }

    fn _iterdump(connection: PyValue) -> Result<Value> {
        let iterdump_fn = _interp.get_attr(&connection.0, "iterdump")?;
        _interp.call_function_expanded(iterdump_fn, &[] as &[ExpandedCallArg])
    }
}
