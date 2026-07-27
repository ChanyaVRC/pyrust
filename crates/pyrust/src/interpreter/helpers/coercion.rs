/// Returns `true` if `v` is a `str` value or a `str` subclass instance.
///
/// CPython's `__format__` protocol accepts `str` subclasses as valid return
/// values (they satisfy `isinstance(result, str)`).  A subclass instance is
/// represented as a `PyInstance` whose `__builtin_data__` backing is `Str`.
pub(crate) fn is_str_or_str_subclass(v: &Value) -> bool {
    match v.kind() {
        ValueKind::Str(_) => true,
        ValueKind::PyInstance(inst) => matches!(
            inst.borrow().attrs.get(BUILTIN_DATA_ATTR).map(|b| b.kind()),
            Some(ValueKind::Str(_))
        ),
        _ => false,
    }
}

/// Extract the string content from a value that is known to satisfy
/// `is_str_or_str_subclass`.  Returns the backing `String` so the caller
/// can append it without holding a borrow across a `RefCell`.
///
/// Panics (debug) if called on a value that is neither `Str` nor a
/// `PyInstance` with a `Str` backing — callers must gate on
/// `is_str_or_str_subclass` first.
pub(crate) fn extract_str_value(v: &Value) -> String {
    match v.kind() {
        ValueKind::Str(s) => s.to_string(),
        ValueKind::PyInstance(inst) => {
            let borrowed = inst.borrow();
            if let Some(backing) = borrowed.attrs.get(BUILTIN_DATA_ATTR)
                && let ValueKind::Str(s) = backing.kind()
            {
                return s.to_string();
            }
            debug_assert!(false, "extract_str_value called on non-str instance");
            v.to_py_str()
        }
        _ => {
            debug_assert!(false, "extract_str_value called on non-str value");
            v.to_py_str()
        }
    }
}

/// Coerce a `str`-subclass instance argument to its backing `Str` value so the
/// receiver-only `pyrust_builtins::string` arg extractors (which match an exact
/// `ValueKind::Str`) accept it — CPython's `str` methods accept any `str`
/// subclass argument (an `isinstance` relationship), #1927.
///
/// The common case (an exact `str`, or any non-`PyInstance` value) is returned
/// untouched after a single cheap tag check, so genuinely-wrong-type arguments
/// still reach the extractor and raise the existing `TypeError`.  Only a
/// `PyInstance` whose `__builtin_data__` backing is `Str` is rewritten.
pub(crate) fn coerce_str_subclass_arg(v: Value) -> Value {
    let backing = if let ValueKind::PyInstance(inst) = v.kind() {
        inst.borrow()
            .attrs
            .get(BUILTIN_DATA_ATTR)
            .filter(|b| matches!(b.kind(), ValueKind::Str(_)))
            .cloned()
    } else {
        None
    };
    backing.unwrap_or(v)
}

/// Coerce a `bytes`-subclass instance argument to its backing `Bytes` value so
/// the `pyrust_builtins::bytes` arg extractors (which match an exact
/// `ValueKind::Bytes`) accept it — CPython's `bytes`/`bytearray` methods accept
/// any `bytes` subclass argument, #1928.
///
/// Like [`coerce_str_subclass_arg`], the common case is returned untouched
/// after a single tag check; only a `PyInstance` with a `Bytes` backing (a
/// `bytes` subclass) or a `bytearray` is rewritten to a real `Bytes` value.
/// CPython treats both as bytes-like objects accepted by `bytes` methods.
pub(crate) fn coerce_bytes_subclass_arg(v: Value) -> Value {
    enum Kind {
        Instance,
        Builtin,
        Other,
    }
    let kind = match v.kind() {
        ValueKind::PyInstance(_) => Kind::Instance,
        ValueKind::BuiltinObject { .. } => Kind::Builtin,
        _ => Kind::Other,
    };
    match kind {
        Kind::Instance => {
            // A bytes subclass backs `__builtin_data__` with a `Bytes`; a
            // bytearray subclass backs it with a `bytearray` (`BuiltinObject`).
            // Both are bytes-like; normalise either to a plain `Bytes` value so
            // every bytes/bytearray method's argument check accepts it (#2677),
            // mirroring the `Kind::Builtin` arm's treatment of a plain bytearray.
            let backing = v.as_py_instance_rc().and_then(|inst| {
                let raw = inst.borrow().attrs.get(BUILTIN_DATA_ATTR).cloned()?;
                if matches!(raw.kind(), ValueKind::Bytes(_)) {
                    Some(raw)
                } else {
                    pyrust_builtins::bytearray::as_bytearray_snapshot(&raw).map(Value::bytes)
                }
            });
            backing.unwrap_or(v)
        }
        Kind::Builtin => match pyrust_builtins::bytearray::as_bytearray_snapshot(&v) {
            Some(snapshot) => Value::bytes(snapshot),
            None => v,
        },
        Kind::Other => v,
    }
}

/// Coerce a `startswith`/`endswith` first argument, which may be either a
/// single prefix/suffix or a *tuple* of them.  A tuple has each element coerced
/// (and is rebuilt); any other value is coerced directly via `coerce` (a no-op
/// for non-subclass values).  Shared by the str and bytes coercion paths.
fn coerce_prefix_arg(v: Value, coerce: fn(Value) -> Value) -> Value {
    let tuple_items: Option<Vec<Value>> = match v.kind() {
        ValueKind::Tuple(items) => Some(items.to_vec()),
        _ => None,
    };
    match tuple_items {
        Some(items) => Value::tuple(items.into_iter().map(coerce).collect()),
        None => coerce(v),
    }
}

/// Coerce the positional arguments of a `str` method so str-subclass instances
/// are accepted (#1927).  Every top-level argument is run through
/// [`coerce_str_subclass_arg`] (a no-op for the common exact-str / int / None
/// cases).  For `startswith`/`endswith` the first argument may be a *tuple* of
/// prefixes; its elements are coerced too.
pub(crate) fn coerce_str_subclass_method_args(method: &str, mut args: Vec<Value>) -> Vec<Value> {
    // Hot path: the overwhelmingly common case is exact-str (or int / None)
    // arguments with no `PyInstance` and no tuple to descend into.  Bail out
    // after a single scan so a normal `"x".count("y")` pays nothing beyond it —
    // no per-element coercion, no Vec rebuild.
    if !args
        .iter()
        .any(|a| matches!(a.kind(), ValueKind::PyInstance(_) | ValueKind::Tuple(_)))
    {
        return args;
    }
    let tuple_arg0 = matches!(method, "startswith" | "endswith");
    for (i, a) in args.iter_mut().enumerate() {
        let taken = std::mem::replace(a, Value::none());
        *a = if tuple_arg0 && i == 0 {
            coerce_prefix_arg(taken, coerce_str_subclass_arg)
        } else {
            coerce_str_subclass_arg(taken)
        };
    }
    args
}

/// Coerce the positional arguments of a `bytes`/`bytearray` method so
/// bytes-subclass and bytearray instances are accepted (#1928).  Mirror of
/// [`coerce_str_subclass_method_args`].
pub(crate) fn coerce_bytes_subclass_method_args(method: &str, mut args: Vec<Value>) -> Vec<Value> {
    // Hot path: exact-bytes / int args need no coercion.  A bytes-subclass is a
    // `PyInstance`; a bytearray is a `BuiltinObject`; the tuple form is a
    // `Tuple`.  Anything else (Bytes, Int, Bool, …) is left untouched.
    if !args.iter().any(|a| {
        matches!(
            a.kind(),
            ValueKind::PyInstance(_) | ValueKind::BuiltinObject { .. } | ValueKind::Tuple(_)
        )
    }) {
        return args;
    }
    let tuple_arg0 = matches!(method, "startswith" | "endswith");
    for (i, a) in args.iter_mut().enumerate() {
        let taken = std::mem::replace(a, Value::none());
        *a = if tuple_arg0 && i == 0 {
            coerce_prefix_arg(taken, coerce_bytes_subclass_arg)
        } else {
            coerce_bytes_subclass_arg(taken)
        };
    }
    args
}

/// Coerce bytes-subclass / bytearray keyword values for bytes methods.
///
/// Returns `None` for the common map containing only exact primitive values,
/// allowing callers to keep borrowing the original map without allocation.
pub(crate) fn coerce_bytes_subclass_method_kwargs(
    kw: &pyrust_core::PyDict,
) -> Option<pyrust_core::PyDict> {
    if !kw.values().any(|value| {
        matches!(
            value.kind(),
            ValueKind::PyInstance(_) | ValueKind::BuiltinObject { .. }
        )
    }) {
        return None;
    }
    Some(
        kw.iter()
            .map(|(key, value)| (key.clone(), coerce_bytes_subclass_arg(value.clone())))
            .collect(),
    )
}

/// Dispatch a prevalidated `bytes` method, coercing bytes-subclass /
/// bytearray arguments (#1928) and, for `partition`/`rpartition`, restoring
/// the *original* separator object as the middle element of the result tuple
/// (#2680).
///
/// CPython's `bytes.partition` / `bytes.rpartition` echo the actual separator
/// argument in the returned tuple: `b'abc'.partition(bytearray(b'b'))[1]` is the
/// very `bytearray` object that was passed (same type *and* identity).  Our
/// receiver-only `bytes::call` only sees the coerced `Bytes` value, so it
/// rebuilds the middle as plain `bytes`.  We capture the pre-coercion separator
/// here and splice it back in when a match is found (the middle element is
/// non-empty).  On a no-match the middle is `b''` and CPython keeps it as plain
/// `bytes`, so we leave it untouched.  The interpreter-aware builtin-method
/// boundary owns signature validation and any Python protocol dispatch before
/// entering this helper.
pub(crate) fn call_bytes_method_coerced_prevalidated(
    method: &str,
    receiver: &Value,
    args: Vec<Value>,
    kw: &pyrust_core::PyDict,
) -> Result<Value> {
    let is_partition = matches!(method, "partition" | "rpartition");
    // Preserve the original separator (before coercion flattens it to `Bytes`)
    // only for the partition pair, and only when it isn't already exact `bytes`
    // (which round-trips correctly); every other call pays nothing.
    let orig_sep = if is_partition {
        match args.first() {
            Some(sep) if !matches!(sep.kind(), ValueKind::Bytes(_)) => Some(sep.clone()),
            _ => None,
        }
    } else {
        None
    };
    let coerced = coerce_bytes_subclass_method_args(method, args);
    // Accepted bytes keywords such as `split(sep=...)`, `hex(sep=...)`, and
    // `translate(delete=...)` use the same bytes-like coercion as their
    // positional slots. Allocate a replacement map only for the uncommon
    // subclass/bytearray value; exact-bytes kwargs retain the borrowed map.
    let coerced_kw = coerce_bytes_subclass_method_kwargs(kw);
    let kw = coerced_kw.as_ref().unwrap_or(kw);
    let result = pyrust_builtins::bytes::call_prevalidated(method, receiver, &coerced, kw)?;
    if let Some(sep) = orig_sep
        && let ValueKind::Tuple(items) = result.kind()
    {
        // A match produced a non-empty middle element; a no-match leaves it
        // empty, and CPython keeps that empty middle as plain `bytes`.
        let mid_nonempty = matches!(
            items.get(1).map(|m| m.kind()),
            Some(ValueKind::Bytes(rc)) if !rc.is_empty()
        );
        if mid_nonempty {
            let mut parts: Vec<Value> = items.to_vec();
            parts[1] = sep;
            return Ok(Value::tuple(parts));
        }
    }
    Ok(result)
}

/// Coerce the elements of a `join` iterable (`str.join`) so str-subclass items
/// join by their str value (#1927).  Only `List`/`Tuple` fast-path containers
/// are rewritten, and only when an element actually needs coercing — an
/// all-exact-str container is returned untouched after a scan.  Any other
/// iterable kind is returned unchanged for the builtins join fn to handle.
pub(crate) fn coerce_str_subclass_join_iterable(iterable: Value) -> Value {
    let needs_coerce = |v: &Value| {
        matches!(v.kind(), ValueKind::PyInstance(inst)
        if matches!(
            inst.borrow().attrs.get(BUILTIN_DATA_ATTR).map(|b| b.kind()),
            Some(ValueKind::Str(_))
        ))
    };
    // Scan the elements *under the borrow* without cloning the container — the
    // all-exact-str case (overwhelmingly common) then pays only the scan and
    // returns `iterable` untouched.  Only when an element actually needs
    // coercing do we snapshot and rebuild a coerced list.
    let snapshot: Option<Vec<Value>> = match iterable.kind() {
        ValueKind::List(items) => {
            if !items.iter().any(needs_coerce) {
                None
            } else {
                Some(items.iter().cloned().collect())
            }
        }
        ValueKind::Tuple(items) => {
            if !items.iter().any(needs_coerce) {
                None
            } else {
                Some(items.to_vec())
            }
        }
        _ => None,
    };
    match snapshot {
        Some(items) => Value::list(items.into_iter().map(coerce_str_subclass_arg).collect()),
        None => iterable,
    }
}

/// Coerce the elements of a `bytes.join` iterable so bytes-subclass / bytearray
/// items join by their bytes value (#1928).  Mirror of
/// [`coerce_str_subclass_join_iterable`].
pub(crate) fn coerce_bytes_subclass_join_iterable(iterable: Value) -> Value {
    let snapshot: Option<Vec<Value>> = match iterable.kind() {
        ValueKind::List(items) => Some(items.iter().cloned().collect()),
        ValueKind::Tuple(items) => Some(items.to_vec()),
        _ => None,
    };
    let Some(items) = snapshot else {
        return iterable;
    };
    let needs = items.iter().any(|v| match v.kind() {
        ValueKind::PyInstance(inst) => matches!(
            inst.borrow().attrs.get(BUILTIN_DATA_ATTR).map(|b| b.kind()),
            Some(ValueKind::Bytes(_))
        ),
        ValueKind::BuiltinObject { ops, .. } => {
            ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray)
        }
        _ => false,
    });
    if !needs {
        return iterable;
    }
    Value::list(items.into_iter().map(coerce_bytes_subclass_arg).collect())
}
