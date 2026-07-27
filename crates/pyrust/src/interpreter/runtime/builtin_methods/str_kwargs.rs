/// Merge keyword arguments into the positional buffer for string methods.
///
/// The receiver-only built-in implementation deliberately has no interpreter
/// or keyword-argument dependency. The concrete built-in-method domain owns
/// CPython-compatible keyword binding before control crosses that boundary.
pub(super) fn str_merge_kwargs(
    binder: pyrust_builtins::string::KeywordBinder,
    pos: &mut Vec<Value>,
    kw: PyDict,
) -> Result<()> {
    use pyrust_builtins::string::KeywordBinder;
    let method = binder.name();
    match binder {
        KeywordBinder::Split | KeywordBinder::RSplit => {
            let mut sep: Option<Value> = None;
            let mut maxsplit: Option<Value> = None;
            for (k, v) in kw {
                let key_str = match &k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => String::new(),
                };
                match key_str.as_str() {
                    "sep" => {
                        if !pos.is_empty() {
                            return Err(pyrust_core::type_err!(
                                "argument for {method}() given by name ('sep') and position (1)"
                            ));
                        }
                        sep = Some(v);
                    }
                    "maxsplit" => {
                        if pos.get(1).is_some() {
                            return Err(pyrust_core::type_err!(
                                "argument for {method}() given by name ('maxsplit') and position (2)"
                            ));
                        }
                        maxsplit = Some(v);
                    }
                    other => {
                        return Err(pyrust_core::type_err!(
                            "'{other}' is an invalid keyword argument for {method}()"
                        ));
                    }
                }
            }
            if let Some(ms) = maxsplit {
                if pos.is_empty() {
                    pos.push(sep.unwrap_or_else(Value::none));
                } else if let Some(sep_val) = sep {
                    pos[0] = sep_val;
                }
                if pos.len() < 2 {
                    pos.push(ms);
                }
            } else if let Some(sep_val) = sep {
                if pos.is_empty() {
                    pos.push(sep_val);
                } else {
                    pos[0] = sep_val;
                }
            }
            Ok(())
        }
        KeywordBinder::SplitLines => {
            *pos = pyrust_builtins::bytes::merge_single_kwarg("splitlines", "keepends", pos, &kw)?;
            Ok(())
        }
        KeywordBinder::Encode => {
            let total = pos.len() + kw.len();
            if total > 2 {
                if pos.is_empty() {
                    return Err(pyrust_core::type_err!(
                        "encode() takes at most 2 keyword arguments ({total} given)"
                    ));
                }
                return Err(pyrust_core::type_err!(
                    "encode() takes at most 2 arguments ({total} given)"
                ));
            }
            let mut encoding: Option<Value> = None;
            let mut errors: Option<Value> = None;
            for (k, v) in kw {
                let key_str = match &k {
                    PyKey::Str(s) => s.as_str().unwrap_or("").to_owned(),
                    _ => String::new(),
                };
                match key_str.as_str() {
                    "encoding" => {
                        if !pos.is_empty() {
                            return Err(pyrust_core::type_err!(
                                "argument for encode() given by name ('encoding') and position (1)"
                            ));
                        }
                        encoding = Some(v);
                    }
                    "errors" => {
                        if pos.get(1).is_some() {
                            return Err(pyrust_core::type_err!(
                                "argument for encode() given by name ('errors') and position (2)"
                            ));
                        }
                        errors = Some(v);
                    }
                    other => {
                        return Err(pyrust_core::type_err!(
                            "'{other}' is an invalid keyword argument for encode()"
                        ));
                    }
                }
            }
            if let Some(err_val) = errors {
                if pos.is_empty() {
                    pos.push(encoding.unwrap_or_else(|| Value::string("utf-8")));
                } else if let Some(enc_val) = encoding {
                    pos[0] = enc_val;
                }
                if pos.len() < 2 {
                    pos.push(err_val);
                }
            } else if let Some(enc_val) = encoding {
                if pos.is_empty() {
                    pos.push(enc_val);
                } else {
                    pos[0] = enc_val;
                }
            }
            Ok(())
        }
        KeywordBinder::ExpandTabs => {
            *pos = pyrust_builtins::bytes::merge_single_kwarg("expandtabs", "tabsize", pos, &kw)?;
            Ok(())
        }
    }
}
