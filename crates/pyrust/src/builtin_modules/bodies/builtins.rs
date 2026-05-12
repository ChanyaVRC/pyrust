// `builtins` module — included into `pub mod builtins { … }` declared by
// the `@flat builtins,` entry in `pyrust_builtin_modules!` in
// `builtin_modules/mod.rs`.
//
// `@flat` means functions register under their short name only (no
// `builtins.` prefix), so `abs` resolves to `BuiltinReg { name: "abs", … }`.
// Therefore `BuiltinFunction("abs")` from the global env (set up in
// `helpers.rs::register_builtins`) hits this dispatch via the registry
// probe in `calls.rs::call_function_expanded`.  Importable as
// `import builtins` too, which yields a `PyModule { name: "builtins", … }`
// containing every fn declared here plus declared constants.
//
// Reference: <https://docs.python.org/3/library/functions.html>

use std::rc::Rc;

use crate::error::{PyError, Result};
use crate::interpreter::ExpandedCallArg;
use crate::interpreter::{ascii_repr, lookup_class_attr, reject_keyword_args_expanded};
use crate::value::{Value, ValueKind};
use pyrust_derive::pyrust_module;

pyrust_module! {
    /// CPython: chr(i) — return the string of one Unicode codepoint i.
    /// <https://docs.python.org/3/library/functions.html#chr>
    fn chr(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let code_point = match args[0].value.kind() {
            ValueKind::Int(v) => v,
            ValueKind::Bool(b) => b as i64,
            _ => return Err(PyError::named(
                "TypeError",
                "an integer is required (got type {})".to_string(),
            )),
        };
        if !(0..=1114111).contains(&code_point) {
            return Err(PyError::named(
                "ValueError",
                format!("{FN_NAME}() arg not in range(0x110000): {code_point}"),
            ));
        }
        let ch = char::from_u32(code_point as u32).ok_or_else(|| {
            PyError::named(
                "ValueError",
                format!("{FN_NAME}() arg not in range(0x110000): {code_point}"),
            )
        })?;
        Ok(Value::string(ch.to_string()))
    }

    /// CPython: ord(c) — return the Unicode codepoint of a one-character string.
    /// <https://docs.python.org/3/library/functions.html#ord>
    fn ord(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Str(s) => {
                let mut chars = s.chars();
                let first = chars.next();
                let second = chars.next();
                match (first, second) {
                    (Some(c), None) => Ok(Value::int(c as i64)),
                    (None, _) => Err(PyError::named(
                        "TypeError",
                        format!("{FN_NAME}() expected a character, but string of length 0 found"),
                    )),
                    (Some(_), Some(_)) => Err(PyError::named(
                        "TypeError",
                        format!(
                            "{FN_NAME}() expected a character, but string of length {} found",
                            s.chars().count()
                        ),
                    )),
                }
            }
            _ => Err(PyError::named(
                "TypeError",
                format!("{FN_NAME}() expected string of length 1, but got non-string"),
            )),
        }
    }

    /// CPython: bin(x) — integer to '0b…' / '-0b…' string.
    /// <https://docs.python.org/3/library/functions.html#bin>
    fn bin(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Int(v) => {
                if v < 0 { Ok(Value::string(format!("-0b{:b}", -v))) }
                else { Ok(Value::string(format!("0b{:b}", v))) }
            }
            ValueKind::Bool(b) => Ok(Value::string(if b { "0b1".to_string() } else { "0b0".to_string() })),
            _ => Err(PyError::named(
                "TypeError",
                "'{}' object cannot be interpreted as an integer".to_string(),
            )),
        }
    }

    /// CPython: oct(x) — integer to '0o…' / '-0o…' string.
    /// <https://docs.python.org/3/library/functions.html#oct>
    fn oct(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Int(v) => {
                if v < 0 { Ok(Value::string(format!("-0o{:o}", -v))) }
                else { Ok(Value::string(format!("0o{:o}", v))) }
            }
            ValueKind::Bool(b) => Ok(Value::string(if b { "0o1".to_string() } else { "0o0".to_string() })),
            _ => Err(PyError::named(
                "TypeError",
                "'{}' object cannot be interpreted as an integer".to_string(),
            )),
        }
    }

    /// CPython: hex(x) — integer to '0x…' / '-0x…' string.
    /// <https://docs.python.org/3/library/functions.html#hex>
    fn hex(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        match args[0].value.kind() {
            ValueKind::Int(v) => {
                if v < 0 { Ok(Value::string(format!("-0x{:x}", -v))) }
                else { Ok(Value::string(format!("0x{:x}", v))) }
            }
            ValueKind::Bool(b) => Ok(Value::string(if b { "0x1".to_string() } else { "0x0".to_string() })),
            _ => Err(PyError::named(
                "TypeError",
                "'{}' object cannot be interpreted as an integer".to_string(),
            )),
        }
    }

    /// CPython: ascii(object) — ASCII-only escaped repr.
    /// <https://docs.python.org/3/library/functions.html#ascii>
    fn ascii(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        Ok(Value::string(ascii_repr(&args[0].value)))
    }

    /// CPython: id(object) — identity (CPython returns memory address).
    /// <https://docs.python.org/3/library/functions.html#id>
    fn id(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly 1 argument")));
        }
        let id_val: i64 = match args[0].value.kind() {
            ValueKind::PyInstance(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::PyClass(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::PyModule(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::UserFunction(rc) => Rc::as_ptr(rc) as i64,
            ValueKind::Int(n) => n,
            ValueKind::Bool(b) => b as i64,
            ValueKind::None => 0,
            _ => args[0].value.value_id().unwrap_or(0),
        };
        Ok(Value::int(id_val))
    }

    /// CPython: callable(object) — true if the object is callable.
    /// <https://docs.python.org/3/library/functions.html#callable>
    fn callable(args) -> Result<Value> {
        reject_keyword_args_expanded(FN_NAME, args)?;
        if args.len() != 1 {
            return Err(PyError::Runtime(format!("{FN_NAME}() takes exactly one argument")));
        }
        let is_callable = match args[0].value.kind() {
            ValueKind::UserFunction(_)
            | ValueKind::BuiltinFunction(_)
            | ValueKind::BoundMethod { .. }
            | ValueKind::ClassBoundMethod { .. }
            | ValueKind::PyClass(_) => true,
            // Only accessor partials (intermediate results of
            // prop.setter / prop.getter / prop.deleter) are callable —
            // a plain property descriptor isn't.
            ValueKind::BuiltinObject { .. } => {
                pyrust_builtins::property::property_partial_slot(&args[0].value)
                    .is_some_and(|slot| slot.is_some())
            }
            ValueKind::PyInstance(inst) => {
                let class = Rc::clone(&inst.borrow().class);
                lookup_class_attr(&class, "__call__").is_some()
            }
            _ => false,
        };
        Ok(Value::bool_(is_callable))
    }
}
