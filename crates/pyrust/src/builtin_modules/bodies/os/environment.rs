//! Process-environment access, `_Environ` mapping, `putenv`/`unsetenv`,
//! `fsencode`/`fsdecode`, and `get_exec_path`.
//!
//! The process environment is the storage of record: there is no shadow map.
//! Every access controlled by PyRust is serialized through `ENV_LOCK`, which
//! also scopes the safety justification for Rust's unsafe mutation APIs.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};

use crate::builtin_modules::genericpath::ops::{PathValue, fspath_resolve};
use crate::error::{PyError, Result};
use crate::interpreter::{ExpandedCallArg, Interpreter, NativeIterFrame, value_type_name_str};
use crate::value::{InstanceAttrs, PyClass, PyInstance, PyKey, Value, ValueKind};

use super::arguments::{require_key_arg, require_no_user_args, require_self, require_str};

/// Serializes all environment access issued by this module.
///
/// This is best-effort protection: foreign threads and libraries can still
/// access the environment without taking PyRust's lock.
static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[cfg(unix)]
pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

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

fn env_str_or_bytes(value: &Value, fn_name: &str, what: &str) -> Result<String> {
    match value.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        ValueKind::Bytes(b) => Ok(String::from_utf8_lossy(b).into_owned()),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "{fn_name}() {what} must be str or bytes, not {}",
                value_type_name_str(value),
            ),
        )),
    }
}

pub(super) fn putenv(name: &Value, value: &Value, fn_name: &str) -> Result<Value> {
    let key = env_str_or_bytes(name, fn_name, "name")?;
    let val = env_str_or_bytes(value, fn_name, "value")?;
    if key.is_empty() || key.contains('\0') || key.contains('=') {
        return Err(PyError::named(
            "ValueError",
            "illegal environment variable name".to_string(),
        ));
    }
    if val.contains('\0') {
        return Err(PyError::named(
            "ValueError",
            "embedded null character".to_string(),
        ));
    }
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: serialized by `ENV_LOCK`.
    unsafe { std::env::set_var(&key, &val) };
    Ok(Value::none())
}

pub(super) fn unsetenv(name: &Value, fn_name: &str) -> Result<Value> {
    let key = env_str_or_bytes(name, fn_name, "name")?;
    if key.is_empty() || key.contains('\0') || key.contains('=') {
        return Err(PyError::named(
            "ValueError",
            "illegal environment variable name".to_string(),
        ));
    }
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: serialized by `ENV_LOCK`.
    unsafe { std::env::remove_var(&key) };
    Ok(Value::none())
}

pub(super) fn fsencode(interp: &mut Interpreter, filename: &Value) -> Result<Value> {
    match fspath_resolve(interp, filename)? {
        PathValue::Bytes(b) => Ok(Value::bytes(b)),
        PathValue::Str(s) => {
            let errors = if cfg!(windows) {
                "strict"
            } else {
                "surrogateescape"
            };
            pyrust_builtins::string::encode_str_to_bytes(&s, "utf-8", errors)
        }
    }
}

pub(super) fn fsdecode(interp: &mut Interpreter, filename: &Value) -> Result<Value> {
    match fspath_resolve(interp, filename)? {
        PathValue::Str(s) => Ok(Value::string(s)),
        PathValue::Bytes(b) => {
            let errors = if cfg!(windows) {
                "strict"
            } else {
                "surrogateescape"
            };
            pyrust_builtins::bytes::decode_bytes(&b, "utf-8", errors)
        }
    }
}

/// CPython 3.12 `os.get_exec_path(env=None)`.
pub(super) fn get_exec_path(interp: &mut Interpreter, env: Option<&Value>) -> Result<Value> {
    let pathsep_str = if cfg!(windows) { ";" } else { ":" };
    let defpath_str = if cfg!(windows) {
        ".;C:\\bin"
    } else {
        "/bin:/usr/bin"
    };
    let supports_bytes_environ = cfg!(unix);

    let path_val: Option<Value> = match env {
        None => {
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::var_os("PATH").map(|v| Value::string(v.to_string_lossy()))
        }
        Some(v) if v.is_none() => {
            let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::var_os("PATH").map(|v| Value::string(v.to_string_lossy()))
        }
        Some(mapping) => {
            let mut str_path: Option<Value> = None;
            let mut bytes_path: Option<Value> = None;
            if let ValueKind::Dict(d) = mapping.kind() {
                str_path = d.get(&PyKey::str_from("PATH")).cloned();
                if supports_bytes_environ {
                    bytes_path = d.get(&PyKey::Bytes(b"PATH".to_vec().into())).cloned();
                }
            } else if let Ok(get_m) = interp.get_attr(mapping, "get") {
                let s_res = interp.call_function_expanded(
                    get_m.clone(),
                    &[ExpandedCallArg {
                        name: None,
                        value: Value::string("PATH"),
                    }],
                )?;
                if !s_res.is_none() {
                    str_path = Some(s_res);
                }
                if supports_bytes_environ {
                    let b_res = interp.call_function_expanded(
                        get_m,
                        &[ExpandedCallArg {
                            name: None,
                            value: Value::bytes(b"PATH".to_vec()),
                        }],
                    )?;
                    if !b_res.is_none() {
                        bytes_path = Some(b_res);
                    }
                }
            }
            if str_path.is_some() && bytes_path.is_some() {
                return Err(PyError::named(
                    "ValueError",
                    "env cannot contain 'PATH' and b'PATH' keys".to_string(),
                ));
            }
            str_path.or(bytes_path)
        }
    };

    let Some(path_val) = path_val else {
        return Ok(Value::list(
            defpath_str.split(pathsep_str).map(Value::string).collect(),
        ));
    };

    let path_str = match path_val.kind() {
        ValueKind::Str(s) => s.to_string(),
        ValueKind::Bytes(_) if supports_bytes_environ => {
            let decoded = fsdecode(interp, &path_val)?;
            decoded.as_str().unwrap_or_default().to_string()
        }
        _ => {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "get_exec_path() PATH value must be str, not {}",
                    value_type_name_str(&path_val),
                ),
            ));
        }
    };
    Ok(Value::list(
        path_str.split(pathsep_str).map(Value::string).collect(),
    ))
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
    // SAFETY: all process-environment access controlled by this module holds `ENV_LOCK`.
    unsafe { std::env::set_var(&key, &value) };
    Ok(Value::none())
}

pub(super) fn environ_delitem(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    let key = require_key_arg(fn_name, args)?;
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if std::env::var_os(&key).is_none() {
        return Err(PyError::key_error(Value::string(key)));
    }
    // SAFETY: the presence check and mutation are serialized by `ENV_LOCK`.
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
