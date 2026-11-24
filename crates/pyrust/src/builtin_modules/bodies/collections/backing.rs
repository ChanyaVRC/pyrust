// Dict-subclass backing, shared mapping methods, and mapping-protocol adapters
// shared by `Counter` and `defaultdict`.

fn snapshot_dict_backing(
    args: &[ExpandedCallArg],
    fn_name: &str,
    type_name: &str,
) -> Result<PyDict> {
    let inst = expect_self(args, fn_name)?;
    let borrow = inst.borrow();
    match borrow.attrs.builtin_data() {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Ok(map.clone()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}: {type_name} backing store has been overwritten with a non-dict; \
                     don't assign to internal attributes",
                ),
            )),
        },
        None => Ok(PyDict::default()),
    }
}

/// Snapshot the Counter's builtin-subclass backing dict.
fn snapshot_counts(args: &[ExpandedCallArg], fn_name: &str) -> Result<PyDict> {
    snapshot_dict_backing(args, fn_name, "Counter")
}

/// Return the live `builtin_data` dict `Value` for Counter/defaultdict.
fn collection_backing(args: &[ExpandedCallArg], fn_name: &str, type_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    {
        let borrow = inst.borrow();
        match borrow.attrs.builtin_data() {
            Some(v) if matches!(v.kind(), ValueKind::Dict(_)) => return Ok(v.clone()),
            Some(_) => {
                return Err(PyError::named(
                    "TypeError",
                    format!(
                        "{fn_name}: {type_name} backing store has been overwritten with a non-dict; \
                         don't assign to internal attributes"
                    ),
                ));
            }
            None => {}
        }
    }
    // No backing yet (e.g. raw PyInstance, `__init__` not run): install one
    // so views and subsequent direct operations share the same Rc.
    let backing = Value::dict(PyDict::default());
    inst.borrow_mut().attrs.set_builtin_data(backing.clone());
    Ok(backing)
}

fn counter_backing(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    collection_backing(args, fn_name, "Counter")
}

fn defaultdict_backing(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    collection_backing(args, fn_name, "defaultdict")
}

fn read_items(args: &[ExpandedCallArg], fn_name: &str) -> Result<PyDict> {
    snapshot_dict_backing(args, fn_name, "defaultdict")
}

// ── Shared dict-subclass method bodies (Counter & defaultdict) ───────────────

fn dict_subclass_setitem(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    type_name: &str,
) -> Result<Value> {
    expect_self(args, fn_name)?;
    if args.len() != 3 {
        return Err(PyError::Runtime(format!(
            "{fn_name}() takes exactly 2 arguments",
        )));
    }
    let key = require_key_arg(args, 1, fn_name)?.clone();
    let backing = collection_backing(args, fn_name, type_name)?;
    interp.dict_set_value(&backing, key, args[2].value.clone())?;
    Ok(Value::none())
}

fn dict_subclass_contains(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    type_name: &str,
) -> Result<Value> {
    let backing = collection_backing(args, fn_name, type_name)?;
    let key = require_key_arg(args, 1, fn_name)?;
    Ok(Value::bool_(
        interp.dict_lookup_value(&backing, key)?.is_some(),
    ))
}

fn dict_subclass_len(args: &[ExpandedCallArg], fn_name: &str, type_name: &str) -> Result<Value> {
    let backing = collection_backing(args, fn_name, type_name)?;
    let len = backing
        .dict_with(|items| items.len())
        .ok_or_else(|| PyError::Runtime(format!("internal: {type_name} backing is not a dict")))?;
    Ok(Value::int(len as i64))
}

fn dict_subclass_iter(args: &[ExpandedCallArg], fn_name: &str, type_name: &str) -> Result<Value> {
    let backing = collection_backing(args, fn_name, type_name)?;
    Ok(make_guarded_dict_subclass_iter(backing))
}

fn dict_subclass_get(
    interp: &mut Interpreter,
    args: &[ExpandedCallArg],
    fn_name: &str,
    type_name: &str,
) -> Result<Value> {
    let backing = collection_backing(args, fn_name, type_name)?;
    let user = &args[1..];
    if user.is_empty() || user.len() > 2 {
        return Err(PyError::named(
            "TypeError",
            "get() takes 1 or 2 arguments".to_string(),
        ));
    }
    let key = require_key_arg(args, 1, fn_name)?;
    Ok(match interp.dict_lookup_value(&backing, key)? {
        Some((_, value)) => value,
        None => user
            .get(1)
            .cloned()
            .map(|a| a.value)
            .unwrap_or_else(Value::none),
    })
}

fn dict_subclass_view(
    args: &[ExpandedCallArg],
    fn_name: &str,
    type_name: &str,
    view_name: &str,
) -> Result<Value> {
    require_no_args(args, view_name)?;
    let backing = collection_backing(args, fn_name, type_name)?;
    crate::Interpreter::dict_view_for_backing(&backing, view_name, false)
}

/// Build the ordinary live key iterator for a `Counter` / `defaultdict`.
fn make_guarded_dict_subclass_iter(backing: Value) -> Value {
    Value::generator(Box::new(NativeIterFrame::live_keys(
        backing,
        0,
        "dict_keyiterator",
    )))
}

/// Extract a required key argument at index `i` without eagerly allocating a
/// `PyKey`, allowing callers to probe via `dict_lookup_value` / `dict_set_value`.
fn require_key_arg<'a>(args: &'a [ExpandedCallArg], i: usize, fn_name: &str) -> Result<&'a Value> {
    let v = args
        .get(i)
        .ok_or_else(|| PyError::Runtime(format!("internal: {fn_name}() missing arg {i}")))?;
    Ok(&v.value)
}

/// `__eq__`-aware `get` against an owned `_counts`/`_items` snapshot map (#1919).
fn map_get_eq(interp: &mut crate::Interpreter, map: &PyDict, key: &PyKey) -> Result<Option<Value>> {
    Ok(interp.dict_lookup_in(map, key)?.map(|(_, v)| v))
}

/// `__eq__`-aware `contains_key` against an owned snapshot map (#1919).
fn map_contains_eq(interp: &mut crate::Interpreter, map: &PyDict, key: &PyKey) -> Result<bool> {
    Ok(interp.dict_lookup_in(map, key)?.is_some())
}

/// Mirror of the `callable()` builtin: decide whether `v` may be used as
/// `defaultdict`'s `default_factory`.
fn value_is_callable(v: &Value) -> bool {
    match v.kind() {
        ValueKind::UserFunction(_)
        | ValueKind::BuiltinFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::PyClass(_) => true,
        ValueKind::BuiltinObject { .. } => crate::interpreter::is_builtin_callable_adapter(v),
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            lookup_class_attr(&class, "__call__").is_some()
        }
        _ => false,
    }
}

/// Apply `dict.__init__`/`dict.update` semantics into `backing`.
fn dict_init_into_backing(
    interp: &mut crate::Interpreter,
    backing: &Value,
    positional: Option<&Value>,
    kwargs: &[&ExpandedCallArg],
) -> Result<()> {
    if let Some(arg) = positional {
        if let Some(entries) = arg.dict_with(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>()
        }) {
            for (key, value) in entries {
                interp.dict_insert_value(backing, key, value)?;
            }
        } else if let Some(cls_rc) = pyrust_builtins::mapping_proxy::as_class_rc(arg) {
            let entries = cls_rc
                .borrow()
                .attrs
                .iter()
                .map(|(key, value)| (PyKey::str_from(key), value.clone()))
                .collect::<Vec<_>>();
            for (key, value) in entries {
                interp.dict_insert_value(backing, key, value)?;
            }
        } else if let Some(dict_rc) = pyrust_builtins::mapping_proxy::as_dict_rc(arg) {
            let entries = dict_rc
                .borrow()
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>();
            for (key, value) in entries {
                interp.dict_insert_value(backing, key, value)?;
            }
        } else if crate::interpreter::visit_mapping_pairs_via_protocol(
            interp,
            arg,
            |interp, key, value| interp.dict_insert_value(backing, key, value),
        )? {
            // Handled via mapping protocol (`keys()` + `__getitem__`).
        } else {
            let iterator = crate::interpreter::make_iterator(interp, arg)?;
            let exhausted = Value::list(Vec::new());
            let exhausted_id = exhausted.value_id();
            let mut idx = 0usize;
            loop {
                let elem = interp.call_next(&iterator, Some(exhausted.clone()))?;
                if elem.value_id().is_some() && elem.value_id() == exhausted_id {
                    break;
                }
                let (k_val, v_val) = match elem.kind() {
                    ValueKind::List(els) => {
                        let len = els.len();
                        if len != 2 {
                            return Err(pyrust_core::value_err!(
                                "dictionary update sequence element #{idx} has length {len}; 2 is required"
                            ));
                        }
                        (els[0].clone(), els[1].clone())
                    }
                    ValueKind::Tuple(els) => {
                        let len = els.len();
                        if len != 2 {
                            return Err(pyrust_core::value_err!(
                                "dictionary update sequence element #{idx} has length {len}; 2 is required"
                            ));
                        }
                        (els[0].clone(), els[1].clone())
                    }
                    ValueKind::Str(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        let len = chars.len();
                        if len != 2 {
                            return Err(pyrust_core::value_err!(
                                "dictionary update sequence element #{idx} has length {len}; 2 is required"
                            ));
                        }
                        (
                            Value::string(chars[0].to_string()),
                            Value::string(chars[1].to_string()),
                        )
                    }
                    _ => {
                        return Err(pyrust_core::type_err!(
                            "cannot convert dictionary update sequence element #{idx} to a sequence"
                        ));
                    }
                };
                let pk = interp.value_to_pykey(&k_val)?;
                interp.dict_insert_value(backing, pk, v_val)?;
                idx += 1;
            }
        }
    }
    for kw in kwargs {
        let name = kw.name.as_deref().unwrap_or("");
        interp.dict_insert_value(backing, PyKey::str_from(name), kw.value.clone())?;
    }
    Ok(())
}

/// Method-body convention: `keys()`, `values()`, `items()` etc. take no
/// args beyond `self`.  Centralised so the error message is uniform.
fn require_no_args(args: &[ExpandedCallArg], method: &str) -> Result<()> {
    if args.len() > 1 {
        Err(PyError::named(
            "TypeError",
            format!("{method}() takes no arguments"),
        ))
    } else {
        Ok(())
    }
}
