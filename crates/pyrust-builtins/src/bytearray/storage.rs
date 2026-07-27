//! Direct mutation operations for bytearray's shared backing storage.
//!
//! These methods run before the read-method snapshot path in `ByteArrayOps`,
//! keeping constant-time mutations from copying the complete bytearray.

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_core::{PyError, Result, Value, ValueKind};

use super::bytearray;
use super::indexing::{bytes_from_value, value_to_byte};

/// Dispatch methods that can work directly against the shared bytearray
/// storage. Returning `None` leaves read-only methods to the bytes-compatible
/// snapshot path in `ByteArrayOps::call_method`.
pub(super) fn call_storage_method(
    data_rc: &Rc<RefCell<Vec<u8>>>,
    method: &str,
    args: &[Value],
) -> Result<Option<Value>> {
    match method {
        "append" => {
            let byte_val = args.first().ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    "append() takes exactly one argument (0 given)".to_string(),
                )
            })?;
            let byte = value_to_byte(byte_val, "bytearray.append")?;
            data_rc.borrow_mut().push(byte);
            Ok(Some(Value::none()))
        }
        "extend" => {
            let iterable = args.first().ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    "extend() takes exactly one argument (0 given)".to_string(),
                )
            })?;
            // Snapshot before taking the write borrow so `data.extend(data)`
            // remains safe and has Python's aliasing semantics.
            let bytes = bytes_from_value(iterable, "bytearray.extend")?;
            data_rc.borrow_mut().extend_from_slice(&bytes);
            Ok(Some(Value::none()))
        }
        "insert" => {
            if args.len() < 2 {
                return Err(PyError::named(
                    "TypeError",
                    format!("insert() takes exactly 2 arguments ({} given)", args.len()),
                ));
            }
            let idx = match args[0].kind() {
                ValueKind::Int(n) => n,
                ValueKind::Bool(b) => b as i64,
                ValueKind::BigInt(_) => {
                    // A BigInt never fits in a C ssize_t, so it can never be
                    // a valid index. CPython raises OverflowError here.
                    return Err(PyError::named(
                        "OverflowError",
                        "Python int too large to convert to C ssize_t".to_string(),
                    ));
                }
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        "integer argument expected".to_string(),
                    ));
                }
            };
            let byte = value_to_byte(&args[1], "bytearray.insert")?;
            let mut data = data_rc.borrow_mut();
            let len = data.len();
            let pos = if idx < 0 {
                let from_end = idx.unsigned_abs();
                if from_end >= len as u64 {
                    0
                } else {
                    len - from_end as usize
                }
            } else {
                (idx as u64).min(len as u64) as usize
            };
            data.insert(pos, byte);
            Ok(Some(Value::none()))
        }
        "pop" => {
            let mut data = data_rc.borrow_mut();
            if data.is_empty() {
                return Err(PyError::named(
                    "IndexError",
                    "pop from empty bytearray".to_string(),
                ));
            }
            let idx = match args.first() {
                None => data.len() - 1,
                Some(v) => {
                    let raw_idx = match v.kind() {
                        ValueKind::Int(n) => n,
                        ValueKind::Bool(b) => b as i64,
                        ValueKind::BigInt(_) => {
                            // A BigInt never fits in a C ssize_t, so it can
                            // never be a valid index.
                            return Err(PyError::named(
                                "OverflowError",
                                "Python int too large to convert to C ssize_t".to_string(),
                            ));
                        }
                        _ => {
                            return Err(PyError::named(
                                "TypeError",
                                "integer argument expected".to_string(),
                            ));
                        }
                    };
                    if raw_idx < 0 {
                        let from_end = raw_idx.unsigned_abs();
                        if from_end > data.len() as u64 {
                            return Err(PyError::named(
                                "IndexError",
                                "pop index out of range".to_string(),
                            ));
                        }
                        data.len() - from_end as usize
                    } else {
                        let idx = raw_idx as u64;
                        if idx >= data.len() as u64 {
                            return Err(PyError::named(
                                "IndexError",
                                "pop index out of range".to_string(),
                            ));
                        }
                        idx as usize
                    }
                }
            };
            Ok(Some(Value::int(data.remove(idx) as i64)))
        }
        "remove" => {
            let val = args.first().ok_or_else(|| {
                PyError::named(
                    "TypeError",
                    "remove() takes exactly one argument (0 given)".to_string(),
                )
            })?;
            let byte = value_to_byte(val, "bytearray.remove")?;
            let mut data = data_rc.borrow_mut();
            let pos = data.iter().position(|&b| b == byte).ok_or_else(|| {
                PyError::named("ValueError", "value not found in bytearray".to_string())
            })?;
            data.remove(pos);
            Ok(Some(Value::none()))
        }
        "reverse" => {
            data_rc.borrow_mut().reverse();
            Ok(Some(Value::none()))
        }
        "clear" => {
            data_rc.borrow_mut().clear();
            Ok(Some(Value::none()))
        }
        "copy" => Ok(Some(bytearray(data_rc.borrow().clone()))),
        _ => Ok(None),
    }
}
