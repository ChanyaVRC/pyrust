//! Python-facing display, comparison, hashing, and path-protocol operations.

use std::rc::Rc;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::{Value, ValueKind};

use super::class_registry::{expect_self, get_path, is_path_instance};

/// FNV-1a hash of a string, matching `PyKey::Str`'s hash algorithm.
fn fnv1a_hash(value: &str) -> i64 {
    let mut hash: u64 = 14695981039346656037;
    for byte in value.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash as i64
}

pub(super) fn path_str(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::string(path))
}

pub(super) fn path_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    let escaped = path
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\'', "\\'");
    // Use the runtime class name so subclasses retain their visible identity.
    let class_name = inst.borrow().class.borrow().name.clone();
    Ok(Value::string(format!("{class_name}('{escaped}')")))
}

pub(super) fn path_eq(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Ok(Value::not_implemented());
    }
    let inst = expect_self(args, fn_name)?;
    let lhs = get_path(&inst, fn_name)?;
    let rhs = match args[1].value.kind() {
        // A string is not a Path. Let reflected comparison/identity fallback
        // decide the result by returning NotImplemented.
        ValueKind::Str(_) => return Ok(Value::not_implemented()),
        ValueKind::PyInstance(other) => {
            if !is_path_instance(other) {
                return Ok(Value::not_implemented());
            }
            match get_path(&Rc::clone(other), fn_name) {
                Ok(path) => path,
                Err(_) => return Ok(Value::not_implemented()),
            }
        }
        _ => return Ok(Value::not_implemented()),
    };
    Ok(Value::bool_(lhs == rhs))
}

pub(super) fn path_hash(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::int(fnv1a_hash(&path)))
}

pub(super) fn path_fspath(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::string(path))
}
