// `encodings` package — 100% native-Rust implementation of Python 3.12's
// `encodings` package (issue #3250).
//
// Exposes `CodecRegistryError(LookupError, SystemError)`,
// `normalize_encoding(encoding)`, `search_function(encoding)`, `aliases`,
// `_aliases`, `_cache`, `_unknown`, `_import_tail`, and registers
// `search_function` with `codecs.register`.
//
// Reference: <https://docs.python.org/3.12/library/codecs.html#standard-encodings>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValueRef;
use crate::interpreter::{ExpandedCallArg, Interpreter, value_type_name_str};
use crate::value::{PyDict, PyKey, PyModule, Value, ValueKind};

const UNKNOWN_SENTINEL: &str = "--unknown--";

fn normalize_encoding_str(s: &str) -> String {
    let mut chars = String::with_capacity(s.len());
    let mut punct = false;
    for c in s.chars() {
        if c.is_alphanumeric() || c == '.' {
            if punct && !chars.is_empty() {
                chars.push('_');
            }
            if c.is_ascii() {
                chars.push(c);
            }
            punct = false;
        } else {
            punct = true;
        }
    }
    chars
}

fn extract_normalize_input(val: &Value) -> Result<String> {
    if let ValueKind::Bytes(rc) = val.kind() {
        let decoded = pyrust_builtins::bytes::decode_bytes(rc.as_slice(), "ascii", "strict")?;
        return Ok(decoded.as_str().unwrap_or("").to_string());
    }
    if let Some(s) = val.as_str() {
        return Ok(s.to_string());
    }
    Err(PyError::named(
        "TypeError",
        format!("'{}' object is not iterable", value_type_name_str(val)),
    ))
}

fn call_positional(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn is_callable(interp: &mut Interpreter, val: &Value) -> bool {
    call_positional(
        interp,
        Value::builtin_function("callable"),
        std::slice::from_ref(val),
    )
    .ok()
    .and_then(|v| interp.truthy_value(&v).ok())
    .unwrap_or(false)
}

fn encodings_module_attr(interp: &mut Interpreter, attr: &str) -> Result<Value> {
    let enc_mod = interp.load_module("encodings")?;
    interp.get_attr(&enc_mod, attr)
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    // Wire `_aliases` to the exact same dict object as `encodings.aliases.aliases`.
    let aliases_mod = interp.load_module("encodings.aliases")?;
    let aliases_dict = interp.get_attr(&aliases_mod, "aliases")?;
    module
        .borrow_mut()
        .insert_attr("_aliases".to_string(), aliases_dict);

    // Register `encodings.search_function` in `codecs` (matching CPython 3.12).
    let codecs_mod = interp.load_module("codecs")?;
    let register_fn = interp.get_attr(&codecs_mod, "register")?;
    if let Some(search_fn) = module.borrow().get_attr_value("search_function") {
        let _ = call_positional(interp, register_fn, &[search_fn]);
    }
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("encodings"),
        "aliases" => super::encodings_aliases::module(),
        "_cache" => Value::dict(PyDict::default()),
        "_unknown" => Value::string(UNKNOWN_SENTINEL),
        "_import_tail" => Value::list(vec![Value::string("*")]),
        "_aliases" => Value::dict(PyDict::default()),
    }

    exception CodecRegistryError(LookupError, SystemError);

    fn normalize_encoding(encoding: PyValueRef<'_>) -> Result<Value> {
        let _ = _interp;
        let s = extract_normalize_input(encoding.0)?;
        Ok(Value::string(normalize_encoding_str(&s)))
    }

    fn search_function(encoding: PyValueRef<'_>) -> Result<Value> {
        let cache = encodings_module_attr(_interp, "_cache")?;
        let key_str = extract_normalize_input(encoding.0)?;
        let cache_key = PyKey::str_from(&key_str);

        if let Some(cached) = cache.as_dict().and_then(|d| d.get(&cache_key).cloned()) {
            return Ok(cached);
        }

        let norm_encoding = normalize_encoding_str(&key_str);
        let aliases = encodings_module_attr(_interp, "_aliases")?;
        let aliased = aliases.as_dict().and_then(|d| {
            d.get(&PyKey::str_from(&norm_encoding))
                .or_else(|| d.get(&PyKey::str_from(norm_encoding.replace('.', "_"))))
                .and_then(|v| v.as_str().map(str::to_string))
        });

        let mut modnames = Vec::with_capacity(2);
        if let Some(a) = aliased {
            modnames.push(a);
        }
        modnames.push(norm_encoding);

        let mut loaded_mod: Option<(String, Value)> = None;
        for modname in modnames {
            if modname.is_empty() || modname.contains('.') {
                continue;
            }
            let full = format!("encodings.{modname}");
            match _interp.load_module(&full) {
                Ok(m) => {
                    loaded_mod = Some((modname, m));
                    break;
                }
                Err(e) if e.class_name_is("ImportError") || e.class_name_is("ModuleNotFoundError") => {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        let Some((modname, mod_val)) = loaded_mod else {
            let _ = cache.dict_insert(cache_key, Value::none());
            return Ok(Value::none());
        };

        let Ok(getregentry) = _interp.get_attr(&mod_val, "getregentry") else {
            let _ = cache.dict_insert(cache_key, Value::none());
            return Ok(Value::none());
        };

        let mut entry = call_positional(_interp, getregentry, &[])?;
        let codecs_mod = _interp.load_module("codecs")?;
        let codec_info_cls = _interp.get_attr(&codecs_mod, "CodecInfo")?;
        let is_ci = call_positional(
            _interp,
            Value::builtin_function("isinstance"),
            &[entry.clone(), codec_info_cls.clone()],
        )?;
        if !_interp.truthy_value(&is_ci)? {
            let items = _interp.collect_iterable(&entry)?;
            let mod_name_str = _interp
                .get_attr(&mod_val, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("encodings.{modname}"));
            let registry_err_cls = encodings_module_attr(_interp, "CodecRegistryError")?;
            if !(4..=7).contains(&items.len()) {
                let exc = call_positional(
                    _interp,
                    registry_err_cls,
                    &[Value::string(format!(
                        "module \"{mod_name_str}\" failed to register"
                    ))],
                )?;
                return Err(PyError::raised(exc));
            }
            let valid_callables = is_callable(_interp, &items[0])
                && is_callable(_interp, &items[1])
                && (items[2].is_none() || is_callable(_interp, &items[2]))
                && (items[3].is_none() || is_callable(_interp, &items[3]))
                && (items.len() <= 4 || items[4].is_none() || is_callable(_interp, &items[4]))
                && (items.len() <= 5 || items[5].is_none() || is_callable(_interp, &items[5]));
            if !valid_callables {
                let exc = call_positional(
                    _interp,
                    registry_err_cls,
                    &[Value::string(format!(
                        "incompatible codecs in module \"{mod_name_str}\""
                    ))],
                )?;
                return Err(PyError::raised(exc));
            }
            let mut full_args = items;
            while full_args.len() < 6 {
                full_args.push(Value::none());
            }
            if full_args.len() < 7 || full_args[6].is_none() {
                let short = mod_name_str
                    .split_once('.')
                    .map(|(_, r)| r)
                    .unwrap_or(&mod_name_str);
                if full_args.len() < 7 {
                    full_args.push(Value::string(short));
                } else {
                    full_args[6] = Value::string(short);
                }
            }
            entry = call_positional(_interp, codec_info_cls, &full_args)?;
        }

        let _ = cache.dict_insert(cache_key, entry.clone());

        if let Ok(getaliases) = _interp.get_attr(&mod_val, "getaliases")
            && let Ok(alias_seq) = call_positional(_interp, getaliases, &[])
            && let Ok(alias_items) = _interp.collect_iterable(&alias_seq)
        {
            for item in alias_items {
                if let Some(alias_s) = item.as_str() {
                    let k = PyKey::str_from(alias_s);
                    let present = aliases.as_dict().is_some_and(|d| d.contains_key(&k));
                    if !present {
                        let _ = aliases.dict_insert(k, Value::string(&modname));
                    }
                }
            }
        }

        Ok(entry)
    }
}
