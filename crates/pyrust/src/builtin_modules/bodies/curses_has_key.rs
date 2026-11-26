// `curses.has_key` module — 100% native Rust implementation of Python 3.12's
// `Lib/curses/has_key.py` (`has_key`, `_capability_names`) (issue #3266).
//
// Gated with `#[cfg(unix)]` in `builtin_modules/mod.rs` to match CPython 3.12.

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::builtin_modules::curses_engine as eng;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

fn build_capability_names_dict() -> Result<Value> {
    let d = Value::dict(PyDict::default());
    for &(key_code, cap_name) in eng::KEY_CAPABILITY_TABLE {
        d.dict_insert(PyKey::Int(key_code), Value::string(cap_name))?;
    }
    Ok(d)
}

pub(crate) fn post_load_init(
    _interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let caps = build_capability_names_dict()?;
    module
        .borrow_mut()
        .insert_attr("_capability_names".to_string(), caps);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("curses"),
    }

    fn has_key(ch: PyValue) -> Result<Value> {
        let code: i64 = match ch.0.kind() {
            ValueKind::Str(s) => {
                let mut cs = s.chars();
                match (cs.next(), cs.next()) {
                    (Some(c), None) => c as i64,
                    _ => {
                        return Err(PyError::named(
                            "TypeError",
                            "ord() expected a character".to_string(),
                        ));
                    }
                }
            }
            ValueKind::Int(n) => n,
            ValueKind::Bool(b) => i64::from(b),
            _ => return Ok(Value::bool_(false)),
        };
        let Some(&(_, cap_name)) = eng::KEY_CAPABILITY_TABLE
            .iter()
            .find(|&&(k, _)| k == code)
        else {
            return Ok(Value::bool_(false));
        };
        let c_mod = _interp.load_module("_curses")?;
        let tigetstr_fn = _interp.get_attr(&c_mod, "tigetstr")?;
        let res = _interp.call_function_expanded(
            tigetstr_fn,
            &[ExpandedCallArg {
                name: None,
                value: Value::string(cap_name),
            }],
        )?;
        Ok(Value::bool_(_interp.truthy_value(&res)?))
    }
}
