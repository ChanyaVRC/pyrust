// Dict-subclass backing and mapping-protocol adapters shared by Counter and
// defaultdict.
//
// Counter-specific update and arithmetic policy lives in counter.rs; this file
// only owns storage access plus generic mapping ingestion.

/// Snapshot the Counter's builtin-subclass backing dict.
///
/// Use this only when the operation needs an owned stable map while it may run
/// Python code (Counter algebra) or must create an independent copy. Simple
/// read-only methods borrow the live backing directly instead.
///
/// If the internal backing was overwritten externally,
/// returns a `TypeError` rather than the internal-error path — the
/// failure is *user-caused*, not an interpreter bug.
fn snapshot_counts(args: &[ExpandedCallArg], fn_name: &str) -> Result<PyDict> {
    let inst = expect_self(args, fn_name)?;
    let borrow = inst.borrow();
    match borrow.attrs.get(BUILTIN_DATA_ATTR) {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Ok(map.clone()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}: Counter backing store has been overwritten with a non-dict; \
                     don't assign to internal attributes",
                ),
            )),
        },
        None => Ok(PyDict::default()),
    }
}

/// Return the live `__builtin_data__` dict `Value` for Counter/defaultdict.
///
/// Cloning the `Value` only increments the backing `Rc`; it does not clone the
/// `IndexMap`.  Read paths can therefore use `Interpreter::dict_lookup`
/// directly, while its object-key slow path still releases the map borrow
/// before dispatching user `__eq__`.
fn collection_backing(args: &[ExpandedCallArg], fn_name: &str, type_name: &str) -> Result<Value> {
    let inst = expect_self(args, fn_name)?;
    {
        let borrow = inst.borrow();
        match borrow.attrs.get(BUILTIN_DATA_ATTR) {
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
    inst.borrow_mut()
        .attrs
        .insert(BUILTIN_DATA_ATTR, backing.clone());
    Ok(backing)
}

fn counter_backing(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    collection_backing(args, fn_name, "Counter")
}

fn defaultdict_backing(args: &[ExpandedCallArg], fn_name: &str) -> Result<Value> {
    collection_backing(args, fn_name, "defaultdict")
}

fn read_items(args: &[ExpandedCallArg], fn_name: &str) -> Result<PyDict> {
    let inst = expect_self(args, fn_name)?;
    let borrow = inst.borrow();
    match borrow.attrs.get(BUILTIN_DATA_ATTR) {
        Some(v) => match v.kind() {
            ValueKind::Dict(map) => Ok(map.clone()),
            _ => Err(PyError::named(
                "TypeError",
                format!(
                    "{fn_name}: defaultdict backing store has been overwritten with a non-dict; \
                     don't assign to internal attributes",
                ),
            )),
        },
        None => Ok(PyDict::default()),
    }
}

/// Build the ordinary live key iterator for a `Counter` / `defaultdict`.
///
/// The iterator owns only a cursor at construction time.  Short walks are
/// therefore O(1) in both time and memory; long mutation-free walks reuse the
/// adaptive snapshot in `NativeIterFrame::live_keys`.  The cursor retains the
/// shared backing `Rc` and its mutation generation, so advancing a normal
/// Counter neither re-resolves `__builtin_data__` nor needs a second size guard
/// on every key. Counter/defaultdict mutation paths update that backing in
/// place, retaining dict's size/key-change errors and value-only mutation
/// behavior.
fn make_guarded_dict_subclass_iter(backing: Value) -> Value {
    Value::generator(Box::new(NativeIterFrame::live_keys(
        backing,
        0,
        "dict_keyiterator",
    )))
}

/// Hashable-key extraction at index `i` with a uniform TypeError on
/// non-hashable input.  Uses the interpreter-aware hash path so that
/// slice keys (and any other type with a custom `__hash__`) are handled
/// correctly rather than falling back to the pure `Value::to_key()` path
/// which cannot hash slices (issue #905).
fn require_key(
    interp: &mut crate::Interpreter,
    args: &[ExpandedCallArg],
    i: usize,
    fn_name: &str,
) -> Result<PyKey> {
    let v = args
        .get(i)
        .ok_or_else(|| PyError::Runtime(format!("internal: {fn_name}() missing arg {i}")))?;
    interp.value_to_pykey(&v.value)
}

/// `__eq__`-aware `get` against an owned `_counts`/`_items` snapshot map
/// (issue #1919).  Routes `PyKey::Object` keys through the interpreter's
/// `dict_lookup_in` (the same `__hash__`-then-`__eq__` path the builtin dict
/// uses); primitive keys hit the raw `IndexMap::get` fast path inside
/// `dict_lookup_in`.  The snapshot is a local copy, so running user `__eq__`
/// against it cannot alias the live store.
fn map_get_eq(interp: &mut crate::Interpreter, map: &PyDict, key: &PyKey) -> Result<Option<Value>> {
    Ok(interp.dict_lookup_in(map, key)?.map(|(_, v)| v))
}

/// `__eq__`-aware `contains_key` against an owned snapshot map (issue #1919).
fn map_contains_eq(interp: &mut crate::Interpreter, map: &PyDict, key: &PyKey) -> Result<bool> {
    Ok(interp.dict_lookup_in(map, key)?.is_some())
}

/// Mirror of the `callable()` builtin (builtins.rs): decide whether `v` may be
/// used as `defaultdict`'s `default_factory`.  CPython accepts *any* callable,
/// not just functions/types — a class with `__call__`, a bound method, or a
/// `functools.partial` are all valid factories.  Keep this in sync with the
/// `callable()` body; the earlier hand-rolled `matches!` here wrongly rejected
/// `__call__` instances and `partial` (#2099 review).
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

/// Apply `dict.__init__`/`dict.update` semantics into `items`: an optional
/// positional mapping-or-iterable-of-pairs followed by string-keyed keyword
/// arguments (#2099 — `defaultdict(factory, mapping)` / `(factory, pairs)` /
/// `(factory, **kw)`).  Mirrors CPython: a mapping (anything with `keys()`) is
/// copied key/value; any other positional is iterated as length-2
/// `(key, value)` pairs.  Insertion is `__eq__`-aware via [`map_insert_eq`] so
/// equal user-keys dedup (#1919).
fn dict_init_into_backing(
    interp: &mut crate::Interpreter,
    backing: &Value,
    positional: Option<&Value>,
    kwargs: &[&ExpandedCallArg],
) -> Result<()> {
    if let Some(arg) = positional {
        // Mapping form: a plain dict is copied verbatim, matching CPython's
        // `dict(mapping)`. Snapshot before insertion so
        // defaultdict.__init__(factory, its_own_backing) does not hold an
        // immutable RefCell borrow while mutating the same map.
        if let Some(entries) = arg.dict_with(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>()
        }) {
            for (key, value) in entries {
                interp.dict_insert_value(backing, key, value)?;
            }
        } else if let Some(cls_rc) = pyrust_builtins::mapping_proxy::as_class_rc(arg) {
            // Class-backed `mappingproxy` (`vars(C)`): copy attrs verbatim.
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
            // Dict-backed `mappingproxy` (`d.keys().mapping`, #2679): copy pairs.
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
            // Any `keys()`-bearing mapping (dict subclasses like Counter /
            // defaultdict / OrderedDict, ChainMap, UserDict, duck-typed user
            // mappings) — keyed via `keys()` + `__getitem__`, exactly like
            // `dict(mapping)`. Each completed __getitem__ is inserted before
            // the next key lookup, preserving CPython's partial update if a
            // later lookup fails.
        } else {
            // Iterable-of-pairs form: each element must be a length-2
            // sequence, unpacked into `(key, value)`. Drive and commit one
            // element at a time so a later source error leaves its completed
            // prefix visible.
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
    // Keyword arguments overlay the positional data, matching CPython order.
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
