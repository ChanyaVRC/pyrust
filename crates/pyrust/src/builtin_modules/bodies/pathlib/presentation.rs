//! Python-facing display, comparison, hashing, and path-protocol operations.

use std::cmp::Ordering;
use std::rc::Rc;

use crate::error::Result;
use crate::interpreter::ExpandedCallArg;
use crate::value::{Value, ValueKind};

use super::class_registry::{
    PathFlavor, expect_self, flavor_for_instance, get_path, is_path_instance,
};
use super::lexical::parse_path;

/// FNV-1a hash of a string, matching `PyKey::Str`'s hash algorithm.
fn fnv1a_hash(value: &str) -> i64 {
    let mut hash: u64 = 14695981039346656037;
    for byte in value.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    hash as i64
}

fn comparison_key(path: &str, flavor: PathFlavor) -> Vec<String> {
    let parsed = parse_path(path, flavor);
    let mut parts = Vec::with_capacity(parsed.tail.len() + 2);
    match flavor {
        PathFlavor::Posix => {
            parts.push(parsed.drive);
            parts.push(parsed.root);
            parts.extend(parsed.tail);
        }
        PathFlavor::Windows => {
            parts.push(parsed.drive.to_lowercase());
            parts.push(parsed.root.to_lowercase());
            parts.extend(parsed.tail.into_iter().map(|p| p.to_lowercase()));
        }
    }
    parts
}

pub(super) fn path_str(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::string(path))
}

pub(super) fn path_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let path = get_path(&inst, fn_name)?;
    // CPython uses `as_posix()` inside `PurePath.__repr__` (so Windows paths
    // render with forward slashes inside `PureWindowsPath('C:/a/b')`).
    let display = match flavor {
        PathFlavor::Posix => path,
        PathFlavor::Windows => path.replace('\\', "/"),
    };
    let escaped = display
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\'', "\\'");
    let class_name = inst.borrow().class.borrow().name.clone();
    Ok(Value::string(format!("{class_name}('{escaped}')")))
}

pub(super) fn path_eq(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    if args.len() != 2 {
        return Ok(Value::not_implemented());
    }
    let inst = expect_self(args, fn_name)?;
    let lhs_flavor = flavor_for_instance(&inst);
    let lhs = get_path(&inst, fn_name)?;
    let (rhs_flavor, rhs) = match args[1].value.kind() {
        ValueKind::PyInstance(other) => {
            if !is_path_instance(other) {
                return Ok(Value::not_implemented());
            }
            let rf = flavor_for_instance(other);
            match get_path(&Rc::clone(other), fn_name) {
                Ok(path) => (rf, path),
                Err(_) => return Ok(Value::not_implemented()),
            }
        }
        _ => return Ok(Value::not_implemented()),
    };
    if lhs_flavor != rhs_flavor {
        return Ok(Value::bool_(false));
    }
    Ok(Value::bool_(
        comparison_key(&lhs, lhs_flavor) == comparison_key(&rhs, rhs_flavor),
    ))
}

fn path_cmp(args: &[ExpandedCallArg], fn_name: &str) -> Result<Option<Ordering>> {
    if args.len() != 2 {
        return Ok(None);
    }
    let inst = expect_self(args, fn_name)?;
    let lhs_flavor = flavor_for_instance(&inst);
    let lhs = get_path(&inst, fn_name)?;
    let (rhs_flavor, rhs) = match args[1].value.kind() {
        ValueKind::PyInstance(other) if is_path_instance(other) => {
            let rf = flavor_for_instance(other);
            match get_path(&Rc::clone(other), fn_name) {
                Ok(path) => (rf, path),
                Err(_) => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    if lhs_flavor != rhs_flavor {
        return Ok(None);
    }
    Ok(Some(
        comparison_key(&lhs, lhs_flavor).cmp(&comparison_key(&rhs, rhs_flavor)),
    ))
}

pub(super) fn path_lt(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    Ok(match path_cmp(args, fn_name)? {
        Some(ord) => Value::bool_(ord.is_lt()),
        None => Value::not_implemented(),
    })
}

pub(super) fn path_le(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    Ok(match path_cmp(args, fn_name)? {
        Some(ord) => Value::bool_(ord.is_le()),
        None => Value::not_implemented(),
    })
}

pub(super) fn path_gt(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    Ok(match path_cmp(args, fn_name)? {
        Some(ord) => Value::bool_(ord.is_gt()),
        None => Value::not_implemented(),
    })
}

pub(super) fn path_ge(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    Ok(match path_cmp(args, fn_name)? {
        Some(ord) => Value::bool_(ord.is_ge()),
        None => Value::not_implemented(),
    })
}

pub(super) fn path_hash(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let flavor = flavor_for_instance(&inst);
    let path = get_path(&inst, fn_name)?;
    let key = match flavor {
        PathFlavor::Posix => format!("P:{path}"),
        PathFlavor::Windows => format!("W:{}", path.to_lowercase()),
    };
    Ok(Value::int(fnv1a_hash(&key)))
}

pub(super) fn path_fspath(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    let path = get_path(&inst, fn_name)?;
    Ok(Value::string(path))
}
