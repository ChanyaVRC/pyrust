// `logging.config` submodule — 100% native Rust implementation of Python 3.12's
// `logging.config` module (issue #3251).
//
// Implements `valid_ident`, `ConvertingMixin`, `ConvertingDict`,
// `ConvertingList`, `ConvertingTuple`, `BaseConfigurator`, `DictConfigurator`,
// `dictConfig`, `dictConfigClass`, and `fileConfig`.
//
// Reference: <https://docs.python.org/3/library/logging.config.html>

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyBool, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, values_are_identical};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

pub(crate) const DEFAULT_LOGGING_CONFIG_PORT: i64 = 9030;

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_pos(interp, f, args)
}

fn call_method_kw(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_kw(interp, f, pos, kw)
}

fn config_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("logging.config")?;
    interp.get_attr(&m, name)
}

fn logging_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("logging")?;
    interp.get_attr(&m, name)
}

fn is_valid_ident_str(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn valid_ident_check(s: &str) -> Result<bool> {
    if !is_valid_ident_str(s) {
        return Err(PyError::named(
            "ValueError",
            format!("Not a valid Python identifier: '{s}'"),
        ));
    }
    Ok(true)
}

fn is_instance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> bool {
    call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls.clone()],
    )
    .and_then(|v| interp.truthy_value(&v))
    .unwrap_or(false)
}

fn is_subclass(interp: &mut Interpreter, sub: &Value, base: &Value) -> bool {
    call_pos(
        interp,
        Value::builtin_function("issubclass"),
        &[sub.clone(), base.clone()],
    )
    .and_then(|v| interp.truthy_value(&v))
    .unwrap_or(false)
}

fn is_converting_container(interp: &mut Interpreter, val: &Value) -> bool {
    for name in ["ConvertingDict", "ConvertingList", "ConvertingTuple"] {
        if let Ok(cls) = config_attr(interp, name)
            && is_instance(interp, val, &cls)
        {
            return true;
        }
    }
    false
}

fn dict_base_call(interp: &mut Interpreter, method: &str, args: &[Value]) -> Result<Value> {
    let dict_cls = crate::interpreter::primitive_class_by_name("dict")
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime("missing dict class".to_string()))?;
    let f = interp.get_attr(&dict_cls, method)?;
    call_pos(interp, f, args)
}

fn list_base_call(interp: &mut Interpreter, method: &str, args: &[Value]) -> Result<Value> {
    let list_cls = crate::interpreter::primitive_class_by_name("list")
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime("missing list class".to_string()))?;
    let f = interp.get_attr(&list_cls, method)?;
    call_pos(interp, f, args)
}

fn tuple_base_call(interp: &mut Interpreter, method: &str, args: &[Value]) -> Result<Value> {
    let tuple_cls = crate::interpreter::primitive_class_by_name("tuple")
        .map(Value::py_class)
        .ok_or_else(|| PyError::runtime("missing tuple class".to_string()))?;
    let f = interp.get_attr(&tuple_cls, method)?;
    call_pos(interp, f, args)
}

fn resolve_dotted(interp: &mut Interpreter, importer: Option<Value>, s: &str) -> Result<Value> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.is_empty() || parts[0].is_empty() {
        return Err(PyError::named(
            "ValueError",
            format!("Cannot resolve {s:?}: invalid name"),
        ));
    }
    let import_fn = importer.unwrap_or_else(|| Value::builtin_function("__import__"));
    let mut used = parts[0].to_string();
    let mut found = call_pos(interp, import_fn.clone(), &[Value::string(&used)])?;
    for frag in &parts[1..] {
        used.push('.');
        used.push_str(frag);
        match interp.get_attr(&found, frag) {
            Ok(v) => found = v,
            Err(err) if err.class_name_is("AttributeError") => {
                call_pos(interp, import_fn.clone(), &[Value::string(&used)])?;
                found = interp.get_attr(&found, frag)?;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(found)
}

fn cfg_convert_impl(interp: &mut Interpreter, self_val: &Value, value: &str) -> Result<Value> {
    let bytes = value.as_bytes();
    let n = bytes.len();
    // Parse leading word: `[a-zA-Z_][a-zA-Z0-9_]*`
    let mut i = 0;
    while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if i == 0 {
        return Err(PyError::named(
            "ValueError",
            format!("Unable to convert {value:?}"),
        ));
    }
    let root_key = &value[..i];
    let config = interp.get_attr(self_val, "config")?;
    let mut d = interp.eval_index(&config, Value::string(root_key))?;

    while i < n {
        if bytes[i] == b'.' {
            i += 1;
            let start = i;
            while i < n && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i == start {
                return Err(PyError::named(
                    "ValueError",
                    format!("Unable to convert {value:?} at {}", &value[start..]),
                ));
            }
            let attr = &value[start..i];
            d = match interp.eval_index(&d, Value::string(attr)) {
                Ok(v) => v,
                Err(_) => interp.get_attr(&d, attr)?,
            };
        } else if bytes[i] == b'[' {
            i += 1;
            let start = i;
            while i < n && bytes[i] != b']' {
                i += 1;
            }
            if i >= n {
                return Err(PyError::named(
                    "ValueError",
                    format!("Unable to convert {value:?}: unclosed '['"),
                ));
            }
            let raw_idx = &value[start..i];
            i += 1; // consume ']'
            let key_val = if !raw_idx.is_empty() && raw_idx.bytes().all(|b| b.is_ascii_digit()) {
                let num: i64 = raw_idx.parse().unwrap_or(0);
                Value::int(num)
            } else if (raw_idx.starts_with('\'') && raw_idx.ends_with('\'') && raw_idx.len() >= 2)
                || (raw_idx.starts_with('"') && raw_idx.ends_with('"') && raw_idx.len() >= 2)
            {
                Value::string(&raw_idx[1..raw_idx.len() - 1])
            } else {
                Value::string(raw_idx)
            };
            d = interp.eval_index(&d, key_val)?;
        } else {
            return Err(PyError::named(
                "ValueError",
                format!("Unable to convert {value:?} at {}", &value[i..]),
            ));
        }
    }
    Ok(d)
}

fn make_value_converters_dict() -> Value {
    let mut d = PyDict::default();
    d.insert(PyKey::str_from("ext"), Value::string("ext_convert"));
    d.insert(PyKey::str_from("cfg"), Value::string("cfg_convert"));
    Value::dict(d)
}

fn clear_handlers(interp: &mut Interpreter, logger: &Value) -> Result<()> {
    let handlers = interp.get_attr(logger, "handlers")?;
    let snapshot = interp.collect_iterable(&handlers)?;
    for h in snapshot {
        call_method(interp, logger, "removeHandler", &[h])?;
    }
    Ok(())
}

fn handle_existing_loggers(
    interp: &mut Interpreter,
    existing: &[String],
    child_loggers: &[String],
    disable_existing: bool,
) -> Result<()> {
    let root = logging_attr(interp, "root")?;
    let manager = interp.get_attr(&root, "manager")?;
    let logger_dict = interp.get_attr(&manager, "loggerDict")?;
    let placeholder_cls = logging_attr(interp, "PlaceHolder")?;
    let notset = logging_attr(interp, "NOTSET")?;

    let child_set: HashSet<&str> = child_loggers.iter().map(String::as_str).collect();
    for log_name in existing {
        let logger = match interp.eval_index(&logger_dict, Value::string(log_name)) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if child_set.contains(log_name.as_str()) {
            if !is_instance(interp, &logger, &placeholder_cls) {
                interp.assign_attr(logger.clone(), "level", notset.clone())?;
                interp.assign_attr(logger.clone(), "handlers", Value::list(Vec::new()))?;
                interp.assign_attr(logger.clone(), "propagate", Value::bool_(true))?;
            }
        } else {
            interp.assign_attr(logger, "disabled", Value::bool_(disable_existing))?;
        }
    }
    Ok(())
}

fn collect_parent_prefixes(configured: &[String], existing: &[String]) -> Vec<String> {
    let mut child_loggers = Vec::new();
    for name in existing {
        let prefix = format!("{name}.");
        if configured.iter().any(|c| c.starts_with(&prefix)) {
            child_loggers.push(name.clone());
        }
    }
    child_loggers
}

fn dict_contains(interp: &mut Interpreter, d: &Value, key: &str) -> Result<bool> {
    let res = call_method(interp, d, "__contains__", &[Value::string(key)])?;
    interp.truthy_value(&res)
}

fn dict_pop_opt(interp: &mut Interpreter, d: &Value, key: &str) -> Result<Option<Value>> {
    if !dict_contains(interp, d, key)? {
        return Ok(None);
    }
    let val = call_method(interp, d, "pop", &[Value::string(key)])?;
    Ok(Some(val))
}

fn dict_get_opt(interp: &mut Interpreter, d: &Value, key: &str) -> Result<Option<Value>> {
    if !dict_contains(interp, d, key)? {
        return Ok(None);
    }
    let val = call_method(interp, d, "get", &[Value::string(key)])?;
    Ok(Some(val))
}

fn dict_keys_strings(interp: &mut Interpreter, d: &Value) -> Result<Vec<String>> {
    let keys = call_method(interp, d, "keys", &[])?;
    let items = interp.collect_iterable(&keys)?;
    Ok(items
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect())
}

fn call_with_valid_ident_kwargs(
    interp: &mut Interpreter,
    factory: Value,
    cfg_dict: &Value,
) -> Result<Value> {
    let keys = dict_keys_strings(interp, cfg_dict)?;
    let mut expanded = Vec::new();
    for k in keys {
        if is_valid_ident_str(&k) {
            let v = interp.eval_index(cfg_dict, Value::string(&k))?;
            expanded.push(ExpandedCallArg {
                name: Some(k),
                value: v,
            });
        }
    }
    interp.call_function_expanded(factory, &expanded)
}

fn file_config_impl(
    interp: &mut Interpreter,
    fname: Value,
    defaults: Value,
    disable_existing_loggers: bool,
    encoding: Value,
) -> Result<Value> {
    let cp_mod = interp.load_module("configparser")?;
    let raw_cp_cls = interp.get_attr(&cp_mod, "RawConfigParser")?;
    let cp = if is_instance(interp, &fname, &raw_cp_cls) {
        fname
    } else {
        let cp_cls = interp.get_attr(&cp_mod, "ConfigParser")?;
        let cp = call_pos(interp, cp_cls, &[defaults])?;
        if interp.get_attr(&fname, "readline").is_ok() {
            call_method(interp, &cp, "read_file", &[fname])?;
        } else {
            call_method_kw(interp, &cp, "read", &[fname], &[("encoding", encoding)])?;
        }
        cp
    };

    let acquire = logging_attr(interp, "_acquireLock")?;
    let release = logging_attr(interp, "_releaseLock")?;
    call_pos(interp, acquire, &[])?;

    let result = (|| -> Result<()> {
        let logging_mod = interp.load_module("logging")?;
        let _ = interp.load_module("logging.handlers")?;
        let eval_ns = interp.get_attr(&logging_mod, "__dict__")?;

        // 1. Parse [formatters]
        let flist_raw = call_method(
            interp,
            &cp,
            "get",
            &[Value::string("formatters"), Value::string("keys")],
        )?;
        let flist_str = flist_raw.as_str().unwrap_or("").trim().to_string();
        let mut formatters: IndexMap<String, Value> = IndexMap::new();
        if !flist_str.is_empty() {
            for form in flist_str
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let sect = format!("formatter_{form}");
                let fs = call_method_kw(
                    interp,
                    &cp,
                    "get",
                    &[Value::string(&sect), Value::string("format")],
                    &[("raw", Value::bool_(true)), ("fallback", Value::none())],
                )?;
                let dfs = call_method_kw(
                    interp,
                    &cp,
                    "get",
                    &[Value::string(&sect), Value::string("datefmt")],
                    &[("raw", Value::bool_(true)), ("fallback", Value::none())],
                )?;
                let stl = call_method_kw(
                    interp,
                    &cp,
                    "get",
                    &[Value::string(&sect), Value::string("style")],
                    &[
                        ("raw", Value::bool_(true)),
                        ("fallback", Value::string("%")),
                    ],
                )?;
                let validate = call_method_kw(
                    interp,
                    &cp,
                    "getboolean",
                    &[Value::string(&sect), Value::string("validate")],
                    &[("fallback", Value::bool_(true))],
                )?;
                let cls_name = call_method_kw(
                    interp,
                    &cp,
                    "get",
                    &[Value::string(&sect), Value::string("class")],
                    &[("fallback", Value::none())],
                )?;
                let cls =
                    if let Some(cn) = cls_name.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        resolve_dotted(interp, None, cn)?
                    } else {
                        logging_attr(interp, "Formatter")?
                    };
                let fs_arg = match fs.as_str() {
                    Some("") => Value::none(),
                    _ => fs,
                };
                let dfs_arg = match dfs.as_str() {
                    Some("") => Value::none(),
                    _ => dfs,
                };
                let f = call_kw(
                    interp,
                    cls,
                    &[fs_arg, dfs_arg, stl],
                    &[("validate", validate)],
                )?;
                formatters.insert(form.to_string(), f);
            }
        }

        // 2. Parse [handlers]
        let hlist_raw = call_method(
            interp,
            &cp,
            "get",
            &[Value::string("handlers"), Value::string("keys")],
        )?;
        let hlist_str = hlist_raw.as_str().unwrap_or("").trim().to_string();
        let mut handlers: IndexMap<String, Value> = IndexMap::new();
        let mut fixups: Vec<(Value, String)> = Vec::new();
        if !hlist_str.is_empty() {
            for hand in hlist_str
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                let sect = format!("handler_{hand}");
                let klass_str = call_method(
                    interp,
                    &cp,
                    "get",
                    &[Value::string(&sect), Value::string("class")],
                )?
                .as_str()
                .unwrap_or("")
                .trim()
                .to_string();
                let klass = match call_pos(
                    interp,
                    Value::builtin_function("eval"),
                    &[Value::string(&klass_str), eval_ns.clone()],
                ) {
                    Ok(v) => v,
                    Err(_) => resolve_dotted(interp, None, &klass_str)?,
                };
                let args_str = call_method_kw(
                    interp,
                    &cp,
                    "get",
                    &[Value::string(&sect), Value::string("args")],
                    &[("fallback", Value::string("()"))],
                )?
                .as_str()
                .unwrap_or("()")
                .trim()
                .to_string();
                let args_val = call_pos(
                    interp,
                    Value::builtin_function("eval"),
                    &[Value::string(&args_str), eval_ns.clone()],
                )?;
                let pos_args = interp.collect_iterable(&args_val)?;

                let kwargs_str = call_method_kw(
                    interp,
                    &cp,
                    "get",
                    &[Value::string(&sect), Value::string("kwargs")],
                    &[("fallback", Value::string("{}"))],
                )?
                .as_str()
                .unwrap_or("{}")
                .trim()
                .to_string();
                let kwargs_val = call_pos(
                    interp,
                    Value::builtin_function("eval"),
                    &[Value::string(&kwargs_str), eval_ns.clone()],
                )?;

                let mut expanded: Vec<ExpandedCallArg> = pos_args
                    .into_iter()
                    .map(|value| ExpandedCallArg { name: None, value })
                    .collect();
                for k in dict_keys_strings(interp, &kwargs_val)? {
                    let v = interp.eval_index(&kwargs_val, Value::string(&k))?;
                    expanded.push(ExpandedCallArg {
                        name: Some(k),
                        value: v,
                    });
                }
                let h = interp.call_function_expanded(klass.clone(), &expanded)?;
                interp.assign_attr(h.clone(), "name", Value::string(hand))?;

                let level_val = call_method_kw(
                    interp,
                    &cp,
                    "get",
                    &[Value::string(&sect), Value::string("level")],
                    &[("fallback", Value::string(""))],
                )?;
                if let Some(lvl_s) = level_val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                    let lvl = if let Ok(n) = lvl_s.parse::<i64>() {
                        Value::int(n)
                    } else {
                        Value::string(lvl_s)
                    };
                    call_method(interp, &h, "setLevel", &[lvl])?;
                }

                let fmt_val = call_method_kw(
                    interp,
                    &cp,
                    "get",
                    &[Value::string(&sect), Value::string("formatter")],
                    &[("fallback", Value::string(""))],
                )?;
                if let Some(fmt_s) = fmt_val.as_str().map(str::trim).filter(|s| !s.is_empty())
                    && let Some(f) = formatters.get(fmt_s)
                {
                    call_method(interp, &h, "setFormatter", std::slice::from_ref(f))?;
                }

                if let Ok(handlers_mod) = interp.load_module("logging.handlers")
                    && let Ok(mem_cls) = interp.get_attr(&handlers_mod, "MemoryHandler")
                    && is_subclass(interp, &klass, &mem_cls)
                {
                    let target_val = call_method_kw(
                        interp,
                        &cp,
                        "get",
                        &[Value::string(&sect), Value::string("target")],
                        &[("fallback", Value::string(""))],
                    )?;
                    if let Some(t) = target_val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        fixups.push((h.clone(), t.to_string()));
                    }
                }
                handlers.insert(hand.to_string(), h);
            }
            for (h, target_name) in fixups {
                if let Some(target) = handlers.get(&target_name) {
                    call_method(interp, &h, "setTarget", std::slice::from_ref(target))?;
                }
            }
        }

        // 3. Parse [loggers]
        let llist_raw = call_method(
            interp,
            &cp,
            "get",
            &[Value::string("loggers"), Value::string("keys")],
        )?;
        let llist: Vec<String> = llist_raw
            .as_str()
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let root = logging_attr(interp, "root")?;
        if llist.iter().any(|s| s == "root") {
            let sect = "logger_root";
            let lvl_val = call_method_kw(
                interp,
                &cp,
                "get",
                &[Value::string(sect), Value::string("level")],
                &[("fallback", Value::string(""))],
            )?;
            if let Some(lvl_s) = lvl_val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                let lvl = if let Ok(n) = lvl_s.parse::<i64>() {
                    Value::int(n)
                } else {
                    Value::string(lvl_s)
                };
                call_method(interp, &root, "setLevel", &[lvl])?;
            }
            clear_handlers(interp, &root)?;
            let h_val = call_method_kw(
                interp,
                &cp,
                "get",
                &[Value::string(sect), Value::string("handlers")],
                &[("fallback", Value::string(""))],
            )?;
            for hname in h_val
                .as_str()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if let Some(h) = handlers.get(hname) {
                    call_method(interp, &root, "addHandler", std::slice::from_ref(h))?;
                }
            }
        }

        let manager = interp.get_attr(&root, "manager")?;
        let logger_dict = interp.get_attr(&manager, "loggerDict")?;
        let mut existing: Vec<String> = dict_keys_strings(interp, &logger_dict)?;
        let mut configured_names: Vec<String> = Vec::new();
        let get_logger = logging_attr(interp, "getLogger")?;

        for log in &llist {
            if log == "root" {
                continue;
            }
            let sect = format!("logger_{log}");
            let qn = call_method(
                interp,
                &cp,
                "get",
                &[Value::string(&sect), Value::string("qualname")],
            )?
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
            let prop = call_method_kw(
                interp,
                &cp,
                "getint",
                &[Value::string(&sect), Value::string("propagate")],
                &[("fallback", Value::int(1))],
            )?;
            let logger = call_pos(interp, get_logger.clone(), &[Value::string(&qn)])?;
            existing.retain(|e| e != &qn);
            configured_names.push(qn.clone());

            let lvl_val = call_method_kw(
                interp,
                &cp,
                "get",
                &[Value::string(&sect), Value::string("level")],
                &[("fallback", Value::string(""))],
            )?;
            if let Some(lvl_s) = lvl_val.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                let lvl = if let Ok(n) = lvl_s.parse::<i64>() {
                    Value::int(n)
                } else {
                    Value::string(lvl_s)
                };
                call_method(interp, &logger, "setLevel", &[lvl])?;
            }
            clear_handlers(interp, &logger)?;
            interp.assign_attr(logger.clone(), "propagate", prop)?;
            interp.assign_attr(logger.clone(), "disabled", Value::bool_(false))?;
            let h_val = call_method_kw(
                interp,
                &cp,
                "get",
                &[Value::string(&sect), Value::string("handlers")],
                &[("fallback", Value::string(""))],
            )?;
            for hname in h_val
                .as_str()
                .unwrap_or("")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                if let Some(h) = handlers.get(hname) {
                    call_method(interp, &logger, "addHandler", std::slice::from_ref(h))?;
                }
            }
        }

        let child_loggers = collect_parent_prefixes(&configured_names, &existing);
        handle_existing_loggers(interp, &existing, &child_loggers, disable_existing_loggers)?;
        Ok(())
    })();

    let _ = call_pos(interp, release, &[]);
    result?;
    Ok(Value::none())
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let re_mod = interp.load_module("re")?;
    let compile_fn = interp.get_attr(&re_mod, "compile")?;
    let icase = interp.get_attr(&re_mod, "I")?;
    let ident_re = call_pos(
        interp,
        compile_fn,
        &[Value::string(r"^[a-z_][a-z0-9_]*$"), icase],
    )?;
    module
        .borrow_mut()
        .insert_attr("IDENTIFIER".to_string(), ident_re);

    if let Ok(errno_mod) = interp.load_module("errno")
        && let Ok(econnreset) = interp.get_attr(&errno_mod, "ECONNRESET")
    {
        module
            .borrow_mut()
            .insert_attr("RESET_ERROR".to_string(), econnreset);
    }

    let dc = module.borrow().attrs.get("DictConfigurator").cloned();
    if let Some(dc) = dc {
        module
            .borrow_mut()
            .insert_attr("dictConfigClass".to_string(), dc);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("logging"),
        "DEFAULT_LOGGING_CONFIG_PORT" => Value::int(DEFAULT_LOGGING_CONFIG_PORT),
        "RESET_ERROR" => Value::int(104),
        "IDENTIFIER" => Value::none(),
        "dictConfigClass" => Value::none(),
    }

    fn valid_ident(s: PyStr<'_>) -> Result<Value> {
        valid_ident_check(s.0.as_ref())?;
        Ok(Value::bool_(true))
    }

    // ── Converting Containers ────────────────────────────────────────────────

    class ConvertingMixin {
        fn convert_with_key(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            value: PyValue,
            #[default(true.into())] replace: PyBool,
        ) -> Result<Value> {
            let configurator = _interp.get_attr(self_val.0, "configurator")?;
            let result = call_method(_interp, &configurator, "convert", std::slice::from_ref(&value.0))?;
            if !values_are_identical(&value.0, &result) {
                if replace.0 {
                    call_method(
                        _interp,
                        self_val.0,
                        "__setitem__",
                        &[key.0.clone(), result.clone()],
                    )?;
                }
                if is_converting_container(_interp, &result) {
                    _interp.assign_attr(result.clone(), "parent", self_val.0.clone())?;
                    _interp.assign_attr(result.clone(), "key", key.0)?;
                }
            }
            Ok(result)
        }

        fn convert(
            #[positional_only] self_val: PyValueRef<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let configurator = _interp.get_attr(self_val.0, "configurator")?;
            let result = call_method(_interp, &configurator, "convert", std::slice::from_ref(&value.0))?;
            if !values_are_identical(&value.0, &result) && is_converting_container(_interp, &result)
            {
                _interp.assign_attr(result.clone(), "parent", self_val.0.clone())?;
            }
            Ok(result)
        }
    }

    class ConvertingDict("dict", "ConvertingMixin") {
        fn __getitem__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let value = dict_base_call(_interp, "__getitem__", &[self_val.0.clone(), key.0.clone()])?;
            call_method(_interp, self_val.0, "convert_with_key", &[key.0, value])
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let def = default.as_value().cloned().unwrap_or_else(Value::none);
            let value = dict_base_call(_interp, "get", &[self_val.0.clone(), key.0.clone(), def])?;
            call_method(_interp, self_val.0, "convert_with_key", &[key.0, value])
        }

        fn pop(
            #[positional_only] self_val: PyValueRef<'_>,
            key: PyValue,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let value = match default.as_value() {
                Some(def) => {
                    dict_base_call(_interp, "pop", &[self_val.0.clone(), key.0.clone(), def.clone()])?
                }
                None => dict_base_call(_interp, "pop", &[self_val.0.clone(), key.0.clone()])?,
            };
            call_method_kw(
                _interp,
                self_val.0,
                "convert_with_key",
                &[key.0, value],
                &[("replace", Value::bool_(false))],
            )
        }
    }

    class ConvertingList("list", "ConvertingMixin") {
        fn __getitem__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let value = list_base_call(_interp, "__getitem__", &[self_val.0.clone(), key.0.clone()])?;
            if let Some(slice_cls) = crate::interpreter::primitive_class_by_name("slice")
                && is_instance(_interp, &key.0, &Value::py_class(slice_cls))
            {
                return Ok(value);
            }
            call_method(_interp, self_val.0, "convert_with_key", &[key.0, value])
        }

        fn pop(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] idx: OptValueRef<'_>,
        ) -> Result<Value> {
            let idx_val = idx.as_value().cloned().unwrap_or_else(|| Value::int(-1));
            let value = list_base_call(_interp, "pop", &[self_val.0.clone(), idx_val])?;
            call_method(_interp, self_val.0, "convert", &[value])
        }
    }

    class ConvertingTuple("tuple", "ConvertingMixin") {
        fn __getitem__(#[positional_only] self_val: PyValueRef<'_>, key: PyValue) -> Result<Value> {
            let value = tuple_base_call(_interp, "__getitem__", &[self_val.0.clone(), key.0.clone()])?;
            if let Some(slice_cls) = crate::interpreter::primitive_class_by_name("slice")
                && is_instance(_interp, &key.0, &Value::py_class(slice_cls))
            {
                return Ok(value);
            }
            call_method_kw(
                _interp,
                self_val.0,
                "convert_with_key",
                &[key.0, value],
                &[("replace", Value::bool_(false))],
            )
        }
    }

    // ── BaseConfigurator ─────────────────────────────────────────────────────

    class BaseConfigurator {
        constants {
            "value_converters" => make_value_converters_dict(),
        }

        #[staticmethod]
        fn importer(
            name: PyValue,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let mut expanded = vec![ExpandedCallArg {
                name: None,
                value: name.0,
            }];
            for a in args {
                expanded.push(ExpandedCallArg {
                    name: None,
                    value: a.clone(),
                });
            }
            for (k, v) in kwargs {
                expanded.push(ExpandedCallArg {
                    name: Some(k),
                    value: v,
                });
            }
            _interp.call_function_expanded(Value::builtin_function("__import__"), &expanded)
        }

        fn __init__(#[positional_only] self_val: PyValueRef<'_>, config: PyValue) -> Result<Value> {
            let cd_cls = config_attr(_interp, "ConvertingDict")?;
            let cd = call_pos(_interp, cd_cls, &[config.0])?;
            _interp.assign_attr(cd.clone(), "configurator", self_val.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "config", cd)?;
            Ok(Value::none())
        }

        fn resolve(#[positional_only] self_val: PyValueRef<'_>, s: PyStr<'_>) -> Result<Value> {
            let imp = _interp.get_attr(self_val.0, "importer").ok();
            resolve_dotted(_interp, imp, s.0.as_ref())
        }

        fn ext_convert(#[positional_only] self_val: PyValueRef<'_>, value: PyStr<'_>) -> Result<Value> {
            call_method(_interp, self_val.0, "resolve", &[Value::string(value.0.as_ref())])
        }

        fn cfg_convert(#[positional_only] self_val: PyValueRef<'_>, value: PyStr<'_>) -> Result<Value> {
            cfg_convert_impl(_interp, self_val.0, value.0.as_ref())
        }

        fn convert(#[positional_only] self_val: PyValueRef<'_>, value: PyValue) -> Result<Value> {
            let val = value.0;
            let cd_cls = config_attr(_interp, "ConvertingDict")?;
            let cl_cls = config_attr(_interp, "ConvertingList")?;
            let ct_cls = config_attr(_interp, "ConvertingTuple")?;
            let dict_cls = Value::py_class(
                crate::interpreter::primitive_class_by_name("dict")
                    .expect("dict class exists"),
            );
            let list_cls = Value::py_class(
                crate::interpreter::primitive_class_by_name("list")
                    .expect("list class exists"),
            );
            let tuple_cls = Value::py_class(
                crate::interpreter::primitive_class_by_name("tuple")
                    .expect("tuple class exists"),
            );

            if !is_instance(_interp, &val, &cd_cls) && is_instance(_interp, &val, &dict_cls) {
                let wrapped = call_pos(_interp, cd_cls, &[val])?;
                _interp.assign_attr(wrapped.clone(), "configurator", self_val.0.clone())?;
                return Ok(wrapped);
            }
            if !is_instance(_interp, &val, &cl_cls) && is_instance(_interp, &val, &list_cls) {
                let wrapped = call_pos(_interp, cl_cls, &[val])?;
                _interp.assign_attr(wrapped.clone(), "configurator", self_val.0.clone())?;
                return Ok(wrapped);
            }
            if !is_instance(_interp, &val, &ct_cls)
                && is_instance(_interp, &val, &tuple_cls)
                && _interp.get_attr(&val, "_fields").is_err()
            {
                let wrapped = call_pos(_interp, ct_cls, &[val])?;
                _interp.assign_attr(wrapped.clone(), "configurator", self_val.0.clone())?;
                return Ok(wrapped);
            }
            if let Some(s) = val.as_str()
                && let Some((prefix, suffix)) = s.split_once("://")
                && !prefix.is_empty()
                && prefix.bytes().all(|b| b.is_ascii_lowercase())
            {
                let converters = _interp.get_attr(self_val.0, "value_converters")?;
                if let Some(conv_name_val) = dict_get_opt(_interp, &converters, prefix)?
                    && let Some(conv_name) = conv_name_val.as_str()
                {
                    let conv_name = conv_name.to_string();
                    return call_method(_interp, self_val.0, &conv_name, &[Value::string(suffix)]);
                }
            }
            Ok(val)
        }

        fn configure_custom(
            #[positional_only] self_val: PyValueRef<'_>,
            config: PyValue,
        ) -> Result<Value> {
            let c_raw = call_method(_interp, &config.0, "pop", &[Value::string("()")])?;
            let is_call = call_pos(
                _interp,
                Value::builtin_function("callable"),
                std::slice::from_ref(&c_raw),
            )?;
            let c = if _interp.truthy_value(&is_call)? {
                c_raw
            } else if let Some(s) = c_raw.as_str() {
                call_method(_interp, self_val.0, "resolve", &[Value::string(s)])?
            } else {
                c_raw
            };
            let props = dict_pop_opt(_interp, &config.0, ".")?;
            let result = call_with_valid_ident_kwargs(_interp, c, &config.0)?;
            if let Some(p) = props {
                for name in dict_keys_strings(_interp, &p)? {
                    let v = _interp.eval_index(&p, Value::string(&name))?;
                    _interp.assign_attr(result.clone(), &name, v)?;
                }
            }
            Ok(result)
        }

        fn as_tuple(#[positional_only] _self_val: PyValueRef<'_>, value: PyValue) -> Result<Value> {
            if matches!(value.0.kind(), ValueKind::List(_)) {
                let items = _interp.collect_iterable(&value.0)?;
                Ok(Value::tuple(items))
            } else {
                Ok(value.0)
            }
        }
    }

    // ── DictConfigurator ─────────────────────────────────────────────────────

    class DictConfigurator : "BaseConfigurator" {
        fn configure(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let config = _interp.get_attr(self_val.0, "config")?;
            let version_opt = dict_get_opt(_interp, &config, "version")?;
            let Some(version) = version_opt else {
                return Err(PyError::named(
                    "ValueError",
                    "dictionary doesn't specify a version".to_string(),
                ));
            };
            if version.as_int() != Some(1) {
                let ver_s = _interp.render_value_as_str(&version).unwrap_or_default();
                return Err(PyError::named(
                    "ValueError",
                    format!("Unsupported version: {ver_s}"),
                ));
            }

            let incremental = dict_pop_opt(_interp, &config, "incremental")?
                .map(|v| _interp.truthy_value(&v))
                .transpose()?
                .unwrap_or(false);

            let acquire = logging_attr(_interp, "_acquireLock")?;
            let release = logging_attr(_interp, "_releaseLock")?;
            call_pos(_interp, acquire, &[])?;

            let run = (|| -> Result<()> {
                if incremental {
                    if let Some(handlers_cfg) = dict_get_opt(_interp, &config, "handlers")? {
                        let global_handlers = logging_attr(_interp, "_handlers")?;
                        for name in dict_keys_strings(_interp, &handlers_cfg)? {
                            let h_opt = dict_get_opt(_interp, &global_handlers, &name)?;
                            let Some(h) = h_opt else {
                                return Err(PyError::named(
                                    "ValueError",
                                    format!("No handler found with name {name:?}"),
                                ));
                            };
                            let hcfg = _interp.eval_index(&handlers_cfg, Value::string(&name))?;
                            if let Some(lvl) = dict_get_opt(_interp, &hcfg, "level")? {
                                call_method(_interp, &h, "setLevel", &[lvl])?;
                            }
                        }
                    }
                    if let Some(loggers_cfg) = dict_get_opt(_interp, &config, "loggers")? {
                        for name in dict_keys_strings(_interp, &loggers_cfg)? {
                            let lcfg = _interp.eval_index(&loggers_cfg, Value::string(&name))?;
                            call_method(
                                _interp,
                                self_val.0,
                                "configure_logger",
                                &[Value::string(&name), lcfg, Value::bool_(true)],
                            )?;
                        }
                    }
                    if let Some(root_cfg) = dict_get_opt(_interp, &config, "root")? {
                        call_method(
                            _interp,
                            self_val.0,
                            "configure_root",
                            &[root_cfg, Value::bool_(true)],
                        )?;
                    }
                    return Ok(());
                }

                let disable_existing = dict_pop_opt(_interp, &config, "disable_existing_loggers")?
                    .map(|v| _interp.truthy_value(&v))
                    .transpose()?
                    .unwrap_or(true);

                // Clear existing handlers cleanly.
                let global_handlers = logging_attr(_interp, "_handlers")?;
                let _ = call_method(_interp, &global_handlers, "clear", &[]);
                let handler_list = logging_attr(_interp, "_handlerList")?;
                let _ = call_method(_interp, &handler_list, "clear", &[]);

                // 1. Formatters
                if let Some(formatters_cfg) = dict_get_opt(_interp, &config, "formatters")? {
                    for name in dict_keys_strings(_interp, &formatters_cfg)? {
                        let fcfg = _interp.eval_index(&formatters_cfg, Value::string(&name))?;
                        let fmt = call_method(_interp, self_val.0, "configure_formatter", &[fcfg])
                            .map_err(|e| {
                                PyError::named(
                                    "ValueError",
                                    format!("Unable to configure formatter {name:?}: {e}"),
                                )
                            })?;
                        call_method(
                            _interp,
                            &formatters_cfg,
                            "__setitem__",
                            &[Value::string(&name), fmt],
                        )?;
                    }
                }

                // 2. Filters
                if let Some(filters_cfg) = dict_get_opt(_interp, &config, "filters")? {
                    for name in dict_keys_strings(_interp, &filters_cfg)? {
                        let flt_cfg = _interp.eval_index(&filters_cfg, Value::string(&name))?;
                        let flt = call_method(_interp, self_val.0, "configure_filter", &[flt_cfg])
                            .map_err(|e| {
                                PyError::named(
                                    "ValueError",
                                    format!("Unable to configure filter {name:?}: {e}"),
                                )
                            })?;
                        call_method(
                            _interp,
                            &filters_cfg,
                            "__setitem__",
                            &[Value::string(&name), flt],
                        )?;
                    }
                }

                // 3. Handlers (with deferral for cross-handler references like MemoryHandler.target)
                if let Some(handlers_cfg) = dict_get_opt(_interp, &config, "handlers")? {
                    let mut names = dict_keys_strings(_interp, &handlers_cfg)?;
                    names.sort();
                    let mut deferred = Vec::new();
                    for name in names {
                        let hcfg = _interp.eval_index(&handlers_cfg, Value::string(&name))?;
                        match call_method(_interp, self_val.0, "configure_handler", &[hcfg]) {
                            Ok(handler) => {
                                _interp.assign_attr(
                                    handler.clone(),
                                    "name",
                                    Value::string(&name),
                                )?;
                                call_method(
                                    _interp,
                                    &handlers_cfg,
                                    "__setitem__",
                                    &[Value::string(&name), handler],
                                )?;
                            }
                            Err(err) => {
                                let msg = format!("{err}");
                                if msg.contains("target not configured yet") {
                                    deferred.push(name);
                                } else {
                                    return Err(PyError::named(
                                        "ValueError",
                                        format!("Unable to configure handler {name:?}: {err}"),
                                    ));
                                }
                            }
                        }
                    }
                    for name in deferred {
                        let hcfg = _interp.eval_index(&handlers_cfg, Value::string(&name))?;
                        let handler = call_method(_interp, self_val.0, "configure_handler", &[hcfg])
                            .map_err(|e| {
                                PyError::named(
                                    "ValueError",
                                    format!("Unable to configure handler {name:?}: {e}"),
                                )
                            })?;
                        _interp.assign_attr(handler.clone(), "name", Value::string(&name))?;
                        call_method(
                            _interp,
                            &handlers_cfg,
                            "__setitem__",
                            &[Value::string(&name), handler],
                        )?;
                    }
                }

                // 4. Loggers
                let root = logging_attr(_interp, "root")?;
                let manager = _interp.get_attr(&root, "manager")?;
                let logger_dict = _interp.get_attr(&manager, "loggerDict")?;
                let mut existing = dict_keys_strings(_interp, &logger_dict)?;
                let mut configured_names = Vec::new();

                if let Some(loggers_cfg) = dict_get_opt(_interp, &config, "loggers")? {
                    for name in dict_keys_strings(_interp, &loggers_cfg)? {
                        existing.retain(|e| e != &name);
                        configured_names.push(name.clone());
                        let lcfg = _interp.eval_index(&loggers_cfg, Value::string(&name))?;
                        call_method(
                            _interp,
                            self_val.0,
                            "configure_logger",
                            &[Value::string(&name), lcfg],
                        )
                        .map_err(|e| {
                            PyError::named(
                                "ValueError",
                                format!("Unable to configure logger {name:?}: {e}"),
                            )
                        })?;
                    }
                }

                let child_loggers = collect_parent_prefixes(&configured_names, &existing);
                handle_existing_loggers(_interp, &existing, &child_loggers, disable_existing)?;

                // 5. Root logger
                if let Some(root_cfg) = dict_get_opt(_interp, &config, "root")? {
                    call_method(_interp, self_val.0, "configure_root", &[root_cfg]).map_err(
                        |e| {
                            PyError::named(
                                "ValueError",
                                format!("Unable to configure root logger: {e}"),
                            )
                        },
                    )?;
                }
                Ok(())
            })();

            let _ = call_pos(_interp, release, &[]);
            run?;
            Ok(Value::none())
        }

        fn configure_formatter(
            #[positional_only] self_val: PyValueRef<'_>,
            config: PyValue,
        ) -> Result<Value> {
            let cfg = config.0;
            if dict_get_opt(_interp, &cfg, "()")?.is_some() {
                return call_method(_interp, self_val.0, "configure_custom", &[cfg]);
            }
            let fmt = dict_get_opt(_interp, &cfg, "format")?.unwrap_or_else(Value::none);
            let dfmt = dict_get_opt(_interp, &cfg, "datefmt")?.unwrap_or_else(Value::none);
            let style = dict_get_opt(_interp, &cfg, "style")?
                .unwrap_or_else(|| Value::string("%"));
            let validate = dict_get_opt(_interp, &cfg, "validate")?
                .unwrap_or_else(|| Value::bool_(true));
            let defaults = dict_get_opt(_interp, &cfg, "defaults")?;
            let cname = dict_get_opt(_interp, &cfg, "class")?;
            let cls = match cname {
                Some(v) if !v.is_none() => {
                    if let Some(s) = v.as_str() {
                        call_method(_interp, self_val.0, "resolve", &[Value::string(s)])?
                    } else {
                        v
                    }
                }
                _ => logging_attr(_interp, "Formatter")?,
            };
            let mut kw = vec![("validate", validate)];
            if let Some(defs) = defaults {
                kw.push(("defaults", defs));
            }
            call_kw(_interp, cls, &[fmt, dfmt, style], &kw)
        }

        fn configure_filter(
            #[positional_only] self_val: PyValueRef<'_>,
            config: PyValue,
        ) -> Result<Value> {
            let cfg = config.0;
            if dict_get_opt(_interp, &cfg, "()")?.is_some() {
                return call_method(_interp, self_val.0, "configure_custom", &[cfg]);
            }
            let name = dict_get_opt(_interp, &cfg, "name")?.unwrap_or_else(|| Value::string(""));
            let filter_cls = logging_attr(_interp, "Filter")?;
            call_pos(_interp, filter_cls, &[name])
        }

        fn add_filters(
            #[positional_only] self_val: PyValueRef<'_>,
            filterer: PyValue,
            filters: PyValue,
        ) -> Result<Value> {
            let config = _interp.get_attr(self_val.0, "config")?;
            let filters_map = dict_get_opt(_interp, &config, "filters")?;
            for f in _interp.collect_iterable(&filters.0)? {
                let flt = if let Some(fname) = f.as_str() {
                    if let Some(ref fm) = filters_map {
                        _interp.eval_index(fm, Value::string(fname)).map_err(|_| {
                            PyError::named(
                                "ValueError",
                                format!("Unable to add filter {fname:?}"),
                            )
                        })?
                    } else {
                        return Err(PyError::named(
                            "ValueError",
                            format!("Unable to add filter {fname:?}"),
                        ));
                    }
                } else {
                    f
                };
                call_method(_interp, &filterer.0, "addFilter", &[flt])?;
            }
            Ok(Value::none())
        }

        fn add_handlers(
            #[positional_only] self_val: PyValueRef<'_>,
            logger: PyValue,
            handlers: PyValue,
        ) -> Result<Value> {
            let config = _interp.get_attr(self_val.0, "config")?;
            let handlers_map = dict_get_opt(_interp, &config, "handlers")?;
            for h in _interp.collect_iterable(&handlers.0)? {
                let hdlr = if let Some(hname) = h.as_str() {
                    if let Some(ref hm) = handlers_map {
                        _interp.eval_index(hm, Value::string(hname)).map_err(|_| {
                            PyError::named(
                                "ValueError",
                                format!("Unable to add handler {hname:?}"),
                            )
                        })?
                    } else {
                        return Err(PyError::named(
                            "ValueError",
                            format!("Unable to add handler {hname:?}"),
                        ));
                    }
                } else {
                    h
                };
                call_method(_interp, &logger.0, "addHandler", &[hdlr])?;
            }
            Ok(Value::none())
        }

        fn configure_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            config: PyValue,
        ) -> Result<Value> {
            let cfg = config.0;
            let root_cfg = _interp.get_attr(self_val.0, "config")?;
            let config_copy = call_pos(
                _interp,
                Value::builtin_function("dict"),
                std::slice::from_ref(&cfg),
            )?;
            let formatter_opt = dict_pop_opt(_interp, &cfg, "formatter")?;
            let formatter = if let Some(fmt_v) = formatter_opt {
                if let Some(fname) = fmt_v.as_str() {
                    let fmts = _interp.eval_index(&root_cfg, Value::string("formatters"))?;
                    Some(_interp.eval_index(&fmts, Value::string(fname))?)
                } else {
                    Some(fmt_v)
                }
            } else {
                None
            };
            let level = dict_pop_opt(_interp, &cfg, "level")?;
            let filters = dict_pop_opt(_interp, &cfg, "filters")?;
            let factory = if dict_get_opt(_interp, &cfg, "()")?.is_some() {
                let c = call_method(_interp, &cfg, "pop", &[Value::string("()")])?;
                if let Some(s) = c.as_str() {
                    call_method(_interp, self_val.0, "resolve", &[Value::string(s)])?
                } else {
                    c
                }
            } else {
                let cname_val = call_method(_interp, &cfg, "pop", &[Value::string("class")])?;
                let klass = if let Some(s) = cname_val.as_str() {
                    call_method(_interp, self_val.0, "resolve", &[Value::string(s)])?
                } else {
                    cname_val
                };
                // Special-case MemoryHandler target resolution + deferral
                if let Ok(handlers_mod) = _interp.load_module("logging.handlers")
                    && let Ok(mem_cls) = _interp.get_attr(&handlers_mod, "MemoryHandler")
                    && is_subclass(_interp, &klass, &mem_cls)
                    && let Some(target_v) = dict_get_opt(_interp, &cfg, "target")?
                    && let Some(tname) = target_v.as_str()
                {
                    let all_handlers = _interp.eval_index(&root_cfg, Value::string("handlers"))?;
                    let target_obj = _interp.eval_index(&all_handlers, Value::string(tname))?;
                    let handler_cls = logging_attr(_interp, "Handler")?;
                    if !is_instance(_interp, &target_obj, &handler_cls) {
                        // Restore cfg keys before deferring
                        call_method(_interp, &cfg, "update", &[config_copy])?;
                        return Err(PyError::named(
                            "ValueError",
                            "target not configured yet".to_string(),
                        ));
                    }
                    call_method(
                        _interp,
                        &cfg,
                        "__setitem__",
                        &[Value::string("target"), target_obj],
                    )?;
                }
                klass
            };

            let props = dict_pop_opt(_interp, &cfg, ".")?;
            let result = call_with_valid_ident_kwargs(_interp, factory, &cfg)?;
            if let Some(fmt) = formatter {
                call_method(_interp, &result, "setFormatter", &[fmt])?;
            }
            if let Some(lvl) = level {
                call_method(_interp, &result, "setLevel", &[lvl])?;
            }
            if let Some(flts) = filters {
                call_method(_interp, self_val.0, "add_filters", &[result.clone(), flts])?;
            }
            if let Some(p) = props {
                for name in dict_keys_strings(_interp, &p)? {
                    let v = _interp.eval_index(&p, Value::string(&name))?;
                    _interp.assign_attr(result.clone(), &name, v)?;
                }
            }
            Ok(result)
        }

        fn common_logger_config(
            #[positional_only] self_val: PyValueRef<'_>,
            logger: PyValue,
            config: PyValue,
            #[default(false.into())] incremental: PyBool,
        ) -> Result<Value> {
            let level = dict_get_opt(_interp, &config.0, "level")?;
            if let Some(lvl) = level {
                call_method(_interp, &logger.0, "setLevel", &[lvl])?;
            }
            if !incremental.0 {
                clear_handlers(_interp, &logger.0)?;
                if let Some(handlers) = dict_get_opt(_interp, &config.0, "handlers")? {
                    call_method(
                        _interp,
                        self_val.0,
                        "add_handlers",
                        &[logger.0.clone(), handlers],
                    )?;
                }
                if let Some(filters) = dict_get_opt(_interp, &config.0, "filters")? {
                    call_method(
                        _interp,
                        self_val.0,
                        "add_filters",
                        &[logger.0, filters],
                    )?;
                }
            }
            Ok(Value::none())
        }

        fn configure_logger(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            config: PyValue,
            #[default(false.into())] incremental: PyBool,
        ) -> Result<Value> {
            let get_logger = logging_attr(_interp, "getLogger")?;
            let logger = call_pos(_interp, get_logger, &[Value::string(name.0.as_ref())])?;
            call_method(
                _interp,
                self_val.0,
                "common_logger_config",
                &[logger.clone(), config.0.clone(), Value::bool_(incremental.0)],
            )?;
            _interp.assign_attr(logger.clone(), "disabled", Value::bool_(false))?;
            if let Some(prop) = dict_get_opt(_interp, &config.0, "propagate")? {
                _interp.assign_attr(logger, "propagate", prop)?;
            }
            Ok(Value::none())
        }

        fn configure_root(
            #[positional_only] self_val: PyValueRef<'_>,
            config: PyValue,
            #[default(false.into())] incremental: PyBool,
        ) -> Result<Value> {
            let root = logging_attr(_interp, "root")?;
            call_method(
                _interp,
                self_val.0,
                "common_logger_config",
                &[root, config.0, Value::bool_(incremental.0)],
            )?;
            Ok(Value::none())
        }
    }

    // ── Public Entrypoints ───────────────────────────────────────────────────

    fn dictConfig(config: PyValue) -> Result<Value> {
        let cls = config_attr(_interp, "dictConfigClass")?;
        let configurator = call_pos(_interp, cls, &[config.0])?;
        call_method(_interp, &configurator, "configure", &[])
    }

    fn fileConfig(
        fname: PyValue,
        #[default(OptionalArg::Omitted)] defaults: OptValueRef<'_>,
        #[default(true.into())] disable_existing_loggers: PyBool,
        #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
    ) -> Result<Value> {
        let defs = defaults.as_value().cloned().unwrap_or_else(Value::none);
        let enc = encoding.as_value().cloned().unwrap_or_else(Value::none);
        file_config_impl(_interp, fname.0, defs, disable_existing_loggers.0, enc)
    }
}
