/// Render the CPython-compatible repr of a `ValueKind::Generator` value
/// (#2019).  True generator frames carry a qualname
/// (`<generator object {qualname} at 0x...>`); built-in iterators use their
/// type name (`<{type_name} object at 0x...>`).  The address is the identity
/// of the underlying generator state, matching `id()` / `Value::value_id`.
fn generator_repr(value: &Value) -> String {
    let addr = value.value_id().unwrap_or(0) as usize;
    if let ValueKind::Generator(cell) = value.kind()
        && let Some(kind) = cell.kind().frame_type_name()
    {
        // Coroutines (`async def`, issue #1039) render as
        // `<coroutine object {qualname} at 0x...>`; async generators
        // (#2280) as `<async_generator object {qualname} at 0x...>`.  Both the
        // noun and the qualname live outside the state cell, so this renders
        // the same whether or not the body is currently running (#2978).
        let qualname = cell.qualname().unwrap_or_else(|| "?".into());
        return format!("<{kind} object {qualname} at 0x{addr:x}>");
    }
    let type_name = full_type_name_str(value);
    format!("<{type_name} object at 0x{addr:x}>")
}

/// Render a `PyKey` dict key or set element to its repr string, honouring
/// `__repr__` on user instances stored as `PyKey::Object`.
///
/// Also recurses into `PyKey::Tuple` and `PyKey::FrozenSet` so that nested
/// user objects inside hashable compound keys get their `__repr__` called.
pub(crate) fn render_key_repr(interp: &mut crate::Interpreter, key: &PyKey) -> Result<String> {
    match key {
        PyKey::Object { value, .. } => render_value_repr(interp, value),
        PyKey::Tuple(items) => {
            if items.is_empty() {
                return Ok("()".to_string());
            }
            let mut parts = Vec::with_capacity(items.len());
            for item in items.iter() {
                parts.push(render_key_repr(interp, item)?);
            }
            if items.len() == 1 {
                Ok(format!("({},)", parts[0]))
            } else {
                Ok(format!("({})", parts.join(", ")))
            }
        }
        PyKey::FrozenSet(key) => {
            let items = key.items();
            if items.is_empty() {
                return Ok("frozenset()".to_string());
            }
            let mut parts = Vec::with_capacity(items.len());
            for item in items.iter() {
                parts.push(render_key_repr(interp, item)?);
            }
            Ok(format!("frozenset({{{}}})", parts.join(", ")))
        }
        _ => Ok(pyrust_core::key_repr(key)),
    }
}

/// Render `value` to its Python-string form, honouring `__str__` / `__repr__`
/// on user instances (in that priority order) and falling back to
/// `<ClassName object>` for instances of classes that define neither.
///
/// Shared by `print` and `str(x)` — both want the same dunder-aware
/// rendering, just wrapped differently (`print` collects into a `Vec<String>`,
/// `str(x)` returns a `Value::string(...)`).  Exception instances without a
/// user-defined `__str__` fall back to `Value::to_py_str()` (matching
/// CPython's `BaseException.__str__`); those with a user-defined `__str__`
/// call it via the normal dunder dispatch loop.
pub(crate) fn render_instance_str(
    interp: &mut crate::Interpreter,
    value: &Value,
) -> Result<String> {
    // gh-95778: reject a base-10 int->str conversion (directly or nested inside
    // a container) that exceeds `sys.get_int_max_str_digits()`.
    // `check_int_str_conversion` fast-rejects non-BigInt/non-container values
    // from their NaN-box tag alone, so the common `str(int)` path pays nothing.
    pyrust_core::check_int_str_conversion(value)?;
    if pyrust_builtins::generic_alias::is_generic_alias(value) {
        return render_generic_alias_repr(interp, value);
    }
    if value.is_list() || value.is_tuple() || value.is_dict() || value.is_set() {
        return render_value_repr(interp, value);
    }
    let ValueKind::PyInstance(inst) = value.kind() else {
        // For containers, str() is defined as repr() in CPython.  Route
        // through render_value_repr so that PyInstance elements inside a
        // list/tuple/dict/set get their __repr__ called.
        return match value.kind() {
            // frozenset: str() == repr() in CPython; delegate to
            // render_value_repr so nested user instances get __repr__.
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
            {
                render_value_repr(interp, value)
            }
            // Issue #2936: CPython's `mappingproxy_str` is `str(proxied)`, so
            // `print(mappingproxy(od))` shows `OrderedDict({...})`.  Only
            // proxies built over a separate object carry an owner; a proxy over
            // a plain dict or a class `__dict__` keeps its current rendering.
            ValueKind::BuiltinObject { .. }
                if let Some(owner) = pyrust_builtins::mapping_proxy::owner_of(value) =>
            {
                render_instance_str(interp, &owner)
            }
            // Issue #2771: `str(cls)` / `print(cls)` dispatches the metaclass
            // `__str__` (falling back to `__repr__`) when overridden; returns
            // `None` for an ordinary class so plain classes keep `to_py_str()`.
            ValueKind::PyClass(cls_rc) => {
                let cls_rc = Rc::clone(cls_rc);
                if let Some(res) =
                    crate::interpreter::dispatch_metaclass_repr_str(interp, &cls_rc, "__str__")
                {
                    return res;
                }
                Ok(value.to_py_str())
            }
            _ => Ok(value.to_py_str()),
        };
    };
    let inst_rc = Rc::clone(inst);
    let class = Rc::clone(&inst_rc.borrow().class);
    // For exception instances, fall back to built-in exception formatting only
    // when the class has no user-defined __str__.  A user-defined __str__ is
    // one whose resolved value is not a BuiltinFunction — i.e. it was declared
    // in user code, not registered as a Rust built-in.
    if is_exception_class(&class) {
        let has_user_str = lookup_class_attr(&class, "__str__")
            .map(|v| !matches!(v.kind(), ValueKind::BuiltinFunction(_)))
            .unwrap_or(false);
        if !has_user_str {
            return crate::interpreter::exception_str_with_dispatch(
                interp, value, &inst_rc, &class,
            );
        }
    }
    // Issue #1204 / #1564: if this instance subclasses a scalar primitive,
    // delegate str() to the backing primitive value when appropriate.
    //
    // For str/bytes subclasses: CPython's str.__str__ returns self directly and
    // never consults __repr__.  So the early-return for str/bytes backing must
    // happen AFTER a user __str__ but BEFORE the __repr__ dispatch.
    //
    // For int/float/bool/BigInt subclasses: CPython's int.__str__ calls
    // __repr__, so the early-return for numeric types must only happen when
    // neither __str__ nor __repr__ is user-defined.
    let has_user_str_dunder = lookup_class_attr(&class, "__str__")
        .map(|v| matches!(v.kind(), ValueKind::UserFunction(_)))
        .unwrap_or(false);
    let has_user_repr_dunder = lookup_class_attr(&class, "__repr__")
        .map(|v| matches!(v.kind(), ValueKind::UserFunction(_)))
        .unwrap_or(false);
    // str/bytes backing: return early unless a user __str__ is defined.
    // (A user __repr__ does NOT override str.__str__ in CPython.)
    if !has_user_str_dunder && let Some(backing) = instance_builtin_data(&inst_rc) {
        match backing.kind() {
            ValueKind::Str(_) | ValueKind::Bytes(_) => return Ok(backing.to_py_str()),
            _ => {}
        }
    }
    // int/float/bool/BigInt backing: return early only when neither user
    // __str__ nor user __repr__ is defined (matching CPython's int.__str__
    // which calls __repr__).
    if !has_user_str_dunder
        && !has_user_repr_dunder
        && let Some(backing) = instance_builtin_data(&inst_rc)
    {
        match backing.kind() {
            ValueKind::Int(_)
            | ValueKind::BigInt(_)
            | ValueKind::Bool(_)
            | ValueKind::Float(_)
            | ValueKind::Complex(_, _) => return Ok(backing.to_py_str()),
            _ => {}
        }
    }
    // Issue #1537: skip `object.__str__` / `object.__repr__` sentinels when
    // the instance has a primitive backing store.  Primitive types now set
    // `object` as an explicit MRO base, making these reachable for user
    // subclasses.  The backing-data path below renders the contents correctly.
    for dunder in &["__str__", "__repr__"] {
        if let Some(method_val) = lookup_class_attr(&class, dunder) {
            let is_object_dunder = crate::interpreter::CanonicalSlot::object_named(dunder)
                .is_some_and(|slot| crate::interpreter::value_is_canonical_slot(&method_val, slot));
            if is_object_dunder && instance_builtin_data(&inst_rc).is_some() {
                continue;
            }
            let result = invoke_class_method(
                interp,
                method_val,
                Value::py_instance(Rc::clone(&inst_rc)),
                &[],
            )?;
            return match result.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(PyError::named(
                    "TypeError",
                    format!("{dunder} returned non-string"),
                )),
            };
        }
    }
    // Issue #1205: no __str__ or __repr__ in MRO (or object.* sentinels
    // skipped) — delegate to the backing
    // container so that list/dict/tuple/set subclasses render their contents
    // via str() just as they do via repr().
    if let Some(backing) = instance_builtin_data(&inst_rc) {
        if backing.is_list() || backing.is_dict() || backing.is_tuple() {
            return render_value_repr(interp, &backing);
        }
        if let Some(is_empty) = backing.set_len().map(|len| len == 0) {
            let class_name = class.borrow().name.clone();
            if is_empty {
                return Ok(format!("{class_name}()"));
            }
            let inner = render_value_repr(interp, &backing)?;
            return Ok(format!("{class_name}({inner})"));
        }
        match backing.kind() {
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
            {
                let class_name = class.borrow().name.clone();
                let items = pyrust_builtins::frozenset::as_items(&backing);
                let is_empty = items.as_ref().is_none_or(|rc| rc.is_empty());
                if is_empty {
                    return Ok(format!("{class_name}()"));
                }
                // Render elements as `{e1, e2}` without the outer `frozenset(...)`
                let snapshot: Vec<_> = items.unwrap().iter().cloned().collect();
                let mut parts = Vec::with_capacity(snapshot.len());
                for k in &snapshot {
                    parts.push(render_key_repr(interp, k)?);
                }
                return Ok(format!("{class_name}({{{}}})", parts.join(", ")));
            }
            // bytearray subclass (#2386): `str(BA(...))` == `repr(BA(...))` in
            // CPython (bytearray has no `__str__`, so `object.__str__` calls
            // `__repr__`), rendering `ClassName(b'...')`.  Delegate to
            // `render_value_repr`, which now handles the bytearray subclass.
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
            {
                return render_value_repr(interp, value);
            }
            _ => {}
        }
    }
    Ok(value.repr_raw())
}
