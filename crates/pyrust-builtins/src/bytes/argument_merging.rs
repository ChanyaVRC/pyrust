/// Normalise `split`/`rsplit` keyword arguments (`sep`, `maxsplit`) into the
/// positional slots `[sep, maxsplit]` that `bytes_split_args` expects.
///
/// CPython 3.12 accepts both `sep` and `maxsplit` by keyword for
/// `bytes`/`bytearray` `split`/`rsplit`; passing the same value by both name
/// and position, an unknown keyword, or more than two positionals all raise
/// `TypeError`.  Error messages use the bare method name (`split()` /
/// `rsplit()`), matching CPython's `Objects/bytesobject.c`.
fn merge_split_kwargs(method: &str, args: &[Value], kwargs: &PyDict) -> Result<Vec<Value>> {
    merge_split_kwargs_iter(
        method,
        args,
        kwargs.len(),
        kwargs.iter().map(|(k, v)| {
            let key = match k {
                PyKey::Str(s) => s.as_str().unwrap_or(""),
                _ => "",
            };
            (key, v)
        }),
    )
}

/// `bytearray.split`/`rsplit` keep their kwargs in a `String`-keyed map; this
/// shim threads them through the same merge logic as `bytes`.
pub fn merge_split_kwargs_str(
    method: &str,
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Vec<Value>> {
    merge_split_kwargs_iter(
        method,
        args,
        kwargs.len(),
        kwargs.iter().map(|(k, v)| (k.as_str(), v)),
    )
}

/// Shared core: normalise `split`/`rsplit` `sep`/`maxsplit` keywords into the
/// positional slots `[sep, maxsplit]`, generic over the keyword key type.
fn merge_split_kwargs_iter<'a>(
    method: &str,
    args: &[Value],
    kwargs_len: usize,
    kwargs: impl Iterator<Item = (&'a str, &'a Value)>,
) -> Result<Vec<Value>> {
    if kwargs_len == 0 {
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "{method}() takes at most 2 arguments ({} given)",
                    args.len()
                ),
            ));
        }
        return Ok(args.to_vec());
    }

    // CPython's argument parser checks the total argument count against the
    // two-positional limit before resolving per-keyword position conflicts, so
    // `split(b" ", 1, maxsplit=1)` reports "takes at most 2 arguments" rather
    // than a name/position clash.
    let total = args.len() + kwargs_len;
    if total > 2 {
        return Err(PyError::named(
            "TypeError",
            format!("{method}() takes at most 2 arguments ({total} given)"),
        ));
    }

    let mut pos = args.to_vec();
    let mut sep: Option<Value> = None;
    let mut maxsplit: Option<Value> = None;
    for (key_str, v) in kwargs {
        match key_str {
            "sep" => {
                if !pos.is_empty() {
                    return Err(PyError::named(
                        "TypeError",
                        format!("argument for {method}() given by name ('sep') and position (1)"),
                    ));
                }
                sep = Some(v.clone());
            }
            "maxsplit" => {
                maxsplit = Some(v.clone());
            }
            other => {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{other}' is an invalid keyword argument for {method}()"),
                ));
            }
        }
    }

    // Fill positional slots: pos[0] = sep, pos[1] = maxsplit.
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
    Ok(pos)
}

/// Normalise a single-keyword method (`expandtabs(tabsize=…)`,
/// `splitlines(keepends=…)`) into a `[value]` positional slot.
///
/// CPython 3.12 accepts the sole argument by keyword as well as position for
/// these `bytes`/`bytearray` methods. Passing it both ways, supplying more
/// than one positional, or using an unknown keyword all raise `TypeError`.
/// CPython's arg parser checks the total argument count against the
/// one-positional limit before resolving the name/position clash, so both
/// `m(x, kw=y)` and `m(x, y)` report `takes at most 1 argument (2 given)`.
pub fn merge_single_kwarg(
    method: &str,
    keyword: &str,
    args: &[Value],
    kwargs: &PyDict,
) -> Result<Vec<Value>> {
    merge_single_kwarg_iter(
        method,
        keyword,
        args,
        kwargs.len(),
        kwargs.iter().map(|(k, v)| {
            let key = match k {
                PyKey::Str(s) => s.as_str().unwrap_or(""),
                _ => "",
            };
            (key, v)
        }),
    )
}

/// `bytearray` keeps its kwargs in a `String`-keyed map; this shim threads them
/// through the same single-keyword merge logic as `bytes`.
pub fn merge_single_kwarg_str(
    method: &str,
    keyword: &str,
    args: &[Value],
    kwargs: &IndexMap<String, Value>,
) -> Result<Vec<Value>> {
    merge_single_kwarg_iter(
        method,
        keyword,
        args,
        kwargs.len(),
        kwargs.iter().map(|(k, v)| (k.as_str(), v)),
    )
}

/// Shared core for [`merge_single_kwarg`] / [`merge_single_kwarg_str`].
fn merge_single_kwarg_iter<'a>(
    method: &str,
    keyword: &str,
    args: &[Value],
    kwargs_len: usize,
    kwargs: impl Iterator<Item = (&'a str, &'a Value)>,
) -> Result<Vec<Value>> {
    if kwargs_len == 0 {
        if args.len() > 1 {
            return Err(PyError::named(
                "TypeError",
                format!("{method}() takes at most 1 argument ({} given)", args.len()),
            ));
        }
        return Ok(args.to_vec());
    }

    let total = args.len() + kwargs_len;
    if total > 1 {
        // CPython distinguishes the all-keyword overflow ("keyword argument")
        // from the mixed/positional overflow ("argument").
        let noun = if args.is_empty() {
            "keyword argument"
        } else {
            "argument"
        };
        return Err(PyError::named(
            "TypeError",
            format!("{method}() takes at most 1 {noun} ({total} given)"),
        ));
    }

    // total == 1 here, so args is empty and there is exactly one keyword.
    let (key_str, v) = kwargs.into_iter().next().expect("one keyword");
    if key_str != keyword {
        return Err(PyError::named(
            "TypeError",
            format!("'{key_str}' is an invalid keyword argument for {method}()"),
        ));
    }
    Ok(vec![v.clone()])
}
