//! Process-environment access and the private `_Environ` mapping.
//!
//! The process environment is the storage of record: there is no shadow map.
//! Every access controlled by PyRust is serialized through `ENV_LOCK`, which
//! also scopes the safety justification for Rust's unsafe mutation APIs.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};

use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, NativeIterFrame};
use crate::value::{InstanceAttrs, PyClass, PyInstance, Value};

use super::arguments::{require_key_arg, require_no_user_args, require_self, require_str};

/// Serializes all environment access issued by this module.
///
/// This is best-effort protection: foreign threads and libraries can still
/// access the process environment without taking PyRust's lock.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(super) fn getenv(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let mut key_value: Option<Value> = None;
    let mut default_value: Option<Value> = None;
    let mut key_from_keyword = false;
    let mut default_from_keyword = false;

    for (index, arg) in args.iter().enumerate() {
        match arg.name.as_deref() {
            Some("key") => {
                if key_value.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'key'"),
                    ));
                }
                key_value = Some(arg.value.clone());
                key_from_keyword = true;
            }
            Some("default") => {
                if default_value.is_some() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() got multiple values for argument 'default'"),
                    ));
                }
                default_value = Some(arg.value.clone());
                default_from_keyword = true;
            }
            Some(other) => {
                return Err(PyError::named(
                    "TypeError",
                    format!("{fn_name}() got an unexpected keyword argument '{other}'"),
                ));
            }
            None => match index {
                0 if !key_from_keyword => key_value = Some(arg.value.clone()),
                1 if !default_from_keyword => default_value = Some(arg.value.clone()),
                _ => {
                    return Err(PyError::named(
                        "TypeError",
                        format!("{fn_name}() takes 1 or 2 arguments ({} given)", args.len()),
                    ));
                }
            },
        }
    }

    let key_value = key_value.ok_or_else(|| {
        PyError::named(
            "TypeError",
            format!("{fn_name}() missing required argument: 'key'"),
        )
    })?;
    let key = require_str(fn_name, &key_value, "key")?;
    let default = default_value.unwrap_or_else(Value::none);

    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    match std::env::var_os(&key) {
        Some(value) => Ok(Value::string(value.to_string_lossy())),
        None => Ok(default),
    }
}

pub(super) fn environ_getitem(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let key = require_key_arg(fn_name, args)?;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    match std::env::var_os(&key) {
        Some(value) => Ok(Value::string(value.to_string_lossy())),
        None => Err(PyError::key_error(Value::string(key))),
    }
}

pub(super) fn environ_setitem(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_self(args, fn_name)?;
    if args.len() != 3 {
        return Err(PyError::named(
            "TypeError",
            format!("__setitem__ takes 2 arguments ({} given)", args.len() - 1),
        ));
    }
    let key = require_str(fn_name, &args[1].value, "key")?;
    let value = require_str(fn_name, &args[2].value, "value")?;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    // SAFETY: all process-environment access controlled by this module holds
    // `ENV_LOCK`. External code may still access the environment concurrently,
    // so this remains the same best-effort guarantee as the original code.
    unsafe { std::env::set_var(&key, &value) };
    Ok(Value::none())
}

pub(super) fn environ_delitem(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let key = require_key_arg(fn_name, args)?;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if std::env::var_os(&key).is_none() {
        return Err(PyError::key_error(Value::string(key)));
    }
    // SAFETY: the presence check and mutation are serialized by `ENV_LOCK`;
    // see the module-level caveat about foreign environment access.
    unsafe { std::env::remove_var(&key) };
    Ok(Value::none())
}

pub(super) fn environ_contains(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let key = require_key_arg(fn_name, args)?;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    Ok(Value::bool_(std::env::var_os(&key).is_some()))
}

pub(super) fn environ_iter(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("__iter__ takes no arguments ({} given)", args.len() - 1),
        ));
    }

    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let keys = std::env::vars_os()
        .map(|(key, _)| Value::string(key.to_string_lossy()))
        .collect();
    Ok(Value::generator(Box::new(NativeIterFrame::generator(keys))))
}

pub(super) fn environ_len(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_self(args, fn_name)?;
    if args.len() != 1 {
        return Err(PyError::named(
            "TypeError",
            format!("__len__ takes no arguments ({} given)", args.len() - 1),
        ));
    }
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    Ok(Value::int(std::env::vars_os().count() as i64))
}

pub(super) fn environ_repr(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_self(args, fn_name)?;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let count = std::env::vars_os().count();
    Ok(Value::string(format!("environ({{...{count} entries...}})")))
}

pub(super) fn environ_get(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_self(args, fn_name)?;
    if args.len() < 2 || args.len() > 3 {
        return Err(PyError::named(
            "TypeError",
            format!("get() takes 1 or 2 arguments ({} given)", args.len() - 1),
        ));
    }
    let key = require_str(fn_name, &args[1].value, "key")?;
    let default = args
        .get(2)
        .map(|arg| arg.value.clone())
        .unwrap_or_else(Value::none);

    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    match std::env::var_os(&key) {
        Some(value) => Ok(Value::string(value.to_string_lossy())),
        None => Ok(default),
    }
}

pub(super) fn environ_keys(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_user_args(args, fn_name, "keys")?;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let keys = std::env::vars_os()
        .map(|(key, _)| Value::string(key.to_string_lossy()))
        .collect();
    Ok(Value::list(keys))
}

pub(super) fn environ_values(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_user_args(args, fn_name, "values")?;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let values = std::env::vars_os()
        .map(|(_, value)| Value::string(value.to_string_lossy()))
        .collect();
    Ok(Value::list(values))
}

pub(super) fn environ_items(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    require_no_user_args(args, fn_name, "items")?;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let items = std::env::vars_os()
        .map(|(key, value)| {
            Value::tuple(vec![
                Value::string(key.to_string_lossy()),
                Value::string(value.to_string_lossy()),
            ])
        })
        .collect();
    Ok(Value::list(items))
}

/// Build one generation-local `_Environ` class and its singleton instance.
///
/// Registry names are supplied by the registration owner so this module does
/// not own or infer Python-facing callable names.
pub(super) fn make_environ_instance(methods: &'static [(&'static str, &'static str)]) -> Value {
    let mut class_attrs = indexmap::IndexMap::new();
    for &(short_name, registry_name) in methods {
        class_attrs.insert(
            short_name.to_string(),
            Value::builtin_function(registry_name),
        );
    }
    class_attrs.insert("__module__".to_string(), Value::string("os"));
    let class = Rc::new(RefCell::new(PyClass::new(
        "_Environ",
        "_Environ",
        None,
        class_attrs,
    )));
    Value::py_instance(Rc::new(RefCell::new(PyInstance {
        class,
        attrs: InstanceAttrs::new(),
    })))
}
