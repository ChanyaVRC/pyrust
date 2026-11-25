// `wsgiref.headers` module — 100% native-Rust Python 3.12 implementation using
// PyRust's expanded macro toolkit (`pyrust_module!`, `class`, `#[varkwargs]`).
//
// Provides `Headers`, a case-insensitive multi-value HTTP response header
// container backed by a Python list of `(name, value)` string tuples.
//
// Reference: <https://docs.python.org/3/library/wsgiref.html#module-wsgiref.headers>

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr, value_class};
use crate::value::{Value, ValueKind};

fn check_exact_str(val: &Value, label: &str) -> Result<String> {
    match val.kind() {
        ValueKind::Str(s) => Ok(s.to_string()),
        _ => Err(PyError::named(
            "TypeError",
            format!("{label} must be of type str (got {val:?})"),
        )),
    }
}

fn format_param(param: &str, value: Option<&str>) -> String {
    let Some(val) = value else {
        return param.to_string();
    };
    let escaped = val.replace('\\', "\\\\").replace('"', "\\\"");
    format!("{param}=\"{escaped}\"")
}

fn get_headers_list(interp: &mut Interpreter, self_val: &Value) -> Result<Vec<(String, String)>> {
    let hdrs_val = interp.get_attr(self_val, "_headers")?;
    let items = interp.collect_iterable(&hdrs_val)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let pair = interp.collect_iterable(&item)?;
        if pair.len() != 2 {
            return Err(PyError::named(
                "ValueError",
                "header entry must be a 2-tuple",
            ));
        }
        let k = check_exact_str(&pair[0], "Header names/values")?;
        let v = check_exact_str(&pair[1], "Header names/values")?;
        out.push((k, v));
    }
    Ok(out)
}

fn write_headers_list(
    interp: &mut Interpreter,
    self_val: &Value,
    pairs: Vec<(String, String)>,
) -> Result<()> {
    let hdrs_val = interp.get_attr(self_val, "_headers")?;
    let items: Vec<Value> = pairs
        .into_iter()
        .map(|(k, v)| Value::tuple(vec![Value::string(k), Value::string(v)]))
        .collect();
    if matches!(hdrs_val.kind(), ValueKind::List(_)) {
        hdrs_val.list_clear()?;
        hdrs_val.list_extend(items)?;
        return Ok(());
    }
    interp.assign_attr(self_val.clone(), "_headers", Value::list(items))?;
    Ok(())
}

fn format_headers_str(pairs: &[(String, String)]) -> String {
    let mut out = String::new();
    for (k, v) in pairs {
        out.push_str(k);
        out.push_str(": ");
        out.push_str(v);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![Value::string("Headers")]),
    }

    class Headers {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] headers: OptValueRef<'_>,
        ) -> Result<Value> {
            let hdrs = match headers.as_value() {
                Some(v) if !v.is_none() => {
                    if !matches!(v.kind(), ValueKind::List(_)) {
                        return Err(PyError::named(
                            "TypeError",
                            "Headers must be a list of name/value tuples",
                        ));
                    }
                    let items = _interp.collect_iterable(v)?;
                    for item in items {
                        let pair = _interp.collect_iterable(&item)?;
                        if pair.len() != 2 {
                            return Err(PyError::named(
                                "ValueError",
                                "header entry must be a 2-tuple",
                            ));
                        }
                        check_exact_str(&pair[0], "Header names/values")?;
                        check_exact_str(&pair[1], "Header names/values")?;
                    }
                    v.clone()
                }
                _ => Value::list(Vec::new()),
            };
            _interp.assign_attr(self_val.0.clone(), "_headers", hdrs)?;
            Ok(Value::none())
        }

        fn __len__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pairs = get_headers_list(_interp, self_val.0)?;
            Ok(Value::int(pairs.len() as i64))
        }

        fn __setitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            val: PyValue,
        ) -> Result<Value> {
            let k = check_exact_str(&name.0, "Header names/values")?;
            let v = check_exact_str(&val.0, "Header names/values")?;
            let lower = k.to_ascii_lowercase();
            let mut pairs = get_headers_list(_interp, self_val.0)?;
            pairs.retain(|(ek, _)| ek.to_ascii_lowercase() != lower);
            pairs.push((k, v));
            write_headers_list(_interp, self_val.0, pairs)?;
            Ok(Value::none())
        }

        fn __delitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let lower = name.0.to_ascii_lowercase();
            let mut pairs = get_headers_list(_interp, self_val.0)?;
            pairs.retain(|(ek, _)| ek.to_ascii_lowercase() != lower);
            write_headers_list(_interp, self_val.0, pairs)?;
            Ok(Value::none())
        }

        fn __getitem__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let lower = name.0.to_ascii_lowercase();
            let pairs = get_headers_list(_interp, self_val.0)?;
            for (k, v) in pairs {
                if k.to_ascii_lowercase() == lower {
                    return Ok(Value::string(v));
                }
            }
            Ok(Value::none())
        }

        fn __contains__(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let lower = name.0.to_ascii_lowercase();
            let pairs = get_headers_list(_interp, self_val.0)?;
            let found = pairs.iter().any(|(k, _)| k.to_ascii_lowercase() == lower);
            Ok(Value::bool_(found))
        }

        fn get_all(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
        ) -> Result<Value> {
            let lower = name.0.to_ascii_lowercase();
            let pairs = get_headers_list(_interp, self_val.0)?;
            let vals: Vec<Value> = pairs
                .into_iter()
                .filter(|(k, _)| k.to_ascii_lowercase() == lower)
                .map(|(_, v)| Value::string(v))
                .collect();
            Ok(Value::list(vals))
        }

        fn get(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyStr<'_>,
            #[default(OptionalArg::Omitted)] default: OptValueRef<'_>,
        ) -> Result<Value> {
            let lower = name.0.to_ascii_lowercase();
            let pairs = get_headers_list(_interp, self_val.0)?;
            for (k, v) in pairs {
                if k.to_ascii_lowercase() == lower {
                    return Ok(Value::string(v));
                }
            }
            Ok(default.as_value().cloned().unwrap_or_else(Value::none))
        }

        fn keys(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pairs = get_headers_list(_interp, self_val.0)?;
            Ok(Value::list(
                pairs.into_iter().map(|(k, _)| Value::string(k)).collect(),
            ))
        }

        fn values(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pairs = get_headers_list(_interp, self_val.0)?;
            Ok(Value::list(
                pairs.into_iter().map(|(_, v)| Value::string(v)).collect(),
            ))
        }

        fn items(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pairs = get_headers_list(_interp, self_val.0)?;
            Ok(Value::list(
                pairs
                    .into_iter()
                    .map(|(k, v)| Value::tuple(vec![Value::string(k), Value::string(v)]))
                    .collect(),
            ))
        }

        fn setdefault(
            #[positional_only] self_val: PyValueRef<'_>,
            name: PyValue,
            value: PyValue,
        ) -> Result<Value> {
            let k = check_exact_str(&name.0, "Header names/values")?;
            let v = check_exact_str(&value.0, "Header names/values")?;
            let lower = k.to_ascii_lowercase();
            let mut pairs = get_headers_list(_interp, self_val.0)?;
            for (ek, ev) in &pairs {
                if ek.to_ascii_lowercase() == lower {
                    return Ok(Value::string(ev.clone()));
                }
            }
            pairs.push((k, v.clone()));
            write_headers_list(_interp, self_val.0, pairs)?;
            Ok(Value::string(v))
        }

        fn add_header(
            #[positional_only] self_val: PyValueRef<'_>,
            _name: PyValue,
            _value: PyValue,
            #[varkwargs] _params: IndexMap<String, Value>,
        ) -> Result<Value> {
            let name = check_exact_str(&_name.0, "Header names/values")?;
            let mut parts: Vec<String> = Vec::new();
            if !_value.0.is_none() {
                parts.push(check_exact_str(&_value.0, "Header names/values")?);
            }
            for (pk, pv) in _params {
                let param_name = pk.replace('_', "-");
                if pv.is_none() {
                    parts.push(format_param(&param_name, None));
                } else {
                    let pv_str = check_exact_str(&pv, "Header names/values")?;
                    parts.push(format_param(&param_name, Some(&pv_str)));
                }
            }
            let joined = parts.join("; ");
            let mut pairs = get_headers_list(_interp, self_val.0)?;
            pairs.push((name, joined));
            write_headers_list(_interp, self_val.0, pairs)?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = value_class(self_val.0);
            let name = _interp
                .get_attr(&cls, "__name__")
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "Headers".to_string());
            let hdrs = _interp.get_attr(self_val.0, "_headers")?;
            let r = render_value_repr(_interp, &hdrs)?;
            Ok(Value::string(format!("{name}({r})")))
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pairs = get_headers_list(_interp, self_val.0)?;
            Ok(Value::string(format_headers_str(&pairs)))
        }

        fn __bytes__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let pairs = get_headers_list(_interp, self_val.0)?;
            let s = format_headers_str(&pairs);
            let mut bytes = Vec::with_capacity(s.len());
            for ch in s.chars() {
                let code = ch as u32;
                if code > 0xFF {
                    return Err(PyError::named(
                        "UnicodeEncodeError",
                        "'latin-1' codec can't encode character in header",
                    ));
                }
                bytes.push(code as u8);
            }
            Ok(Value::bytes(bytes))
        }
    }
}
