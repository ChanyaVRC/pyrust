/// Format an i64 as Python's `bin()` output — `"0bN"` / `"-0bN"`.  Used
/// by both the `PyInt` and `PyBool` overloads of the typed `bin`
/// builtin (#400).  Widens through i128 first so `i64::MIN.abs()`
/// doesn't overflow.
pub(crate) fn format_bin_i64(v: i64) -> String {
    if v < 0 {
        format!("-0b{:b}", -(v as i128))
    } else {
        format!("0b{:b}", v)
    }
}

/// Format an i64 as Python's `oct()` output — `"0oN"` / `"-0oN"`.  Used
/// by both the `PyInt` and `PyBool` overloads of the typed `oct`
/// builtin (#400).  Widens through i128 first so `i64::MIN.abs()`
/// doesn't overflow.
pub(crate) fn format_oct_i64(v: i64) -> String {
    if v < 0 {
        format!("-0o{:o}", -(v as i128))
    } else {
        format!("0o{:o}", v)
    }
}

/// Returns `true` when a container element (a `Value`) requires interpreter
/// access during repr — i.e., when `Value::repr_raw()` alone is insufficient.
///
/// The only cases that need interpreter dispatch are:
/// - `PyInstance` — may have a user-defined `__repr__`
/// - Container types (`List`, `Tuple`, `Dict`, `Set`) — may *contain* an
///   instance at any nesting depth
/// - `BuiltinObject` — may be a frozenset containing `PyKey::Object`, or
///   another builtin type with user-backing
/// - `Generator` — its public repr is reconstructed by the interpreter;
///   `repr_raw()` only exposes the internal generator carrier
///
/// Plain scalars (`Int`, `Str`, `Float`, `Bool`, `None`, `BigInt`, `Bytes`,
/// `Complex`, `Ellipsis`, `NotImplemented`) always return `false`.
#[inline]
fn value_needs_interp_repr(v: &Value) -> bool {
    match v.kind() {
        ValueKind::PyInstance(_)
        | ValueKind::BuiltinObject { .. }
        | ValueKind::List(_)
        | ValueKind::Tuple(_)
        | ValueKind::Dict(_)
        | ValueKind::Set(_)
        | ValueKind::Generator(_) => true,
        // Issue #2771: a class whose metaclass overrides `__repr__` needs the
        // interpreter to dispatch it (e.g. `[Color]` -> `[<enum 'Color'>]`).
        // Only classes with a *custom* metatype can carry such an override, so
        // the common case (`[int, str]`, metatype is the built-in `type`) keeps
        // the pure `repr_raw()` fast path.
        ValueKind::PyClass(cls) => cls.borrow().metatype.is_some(),
        _ => false,
    }
}

/// Returns `true` when a container key (`PyKey`) requires interpreter access
/// during repr.  Only `PyKey::Object` (user instance), `PyKey::Tuple` (may
/// contain an Object), and `PyKey::FrozenSet` (may contain an Object) need
/// the slow path; all primitive key variants are handled by `key_repr`.
#[inline]
fn key_needs_interp_repr(k: &PyKey) -> bool {
    matches!(
        k,
        PyKey::Object { .. } | PyKey::Tuple(_) | PyKey::FrozenSet(_)
    )
}

/// Render `value` to its Python repr string, honouring `__repr__` on user
/// instances and recursing into containers (list/tuple/dict/set) with the same
/// interpreter-aware dispatch on each element.
///
/// Shared by the `repr()` builtin and `render_instance_str` (for the container
/// case, where `str(list)` is defined as `repr(list)` in CPython).
///
/// Cycle detection mirrors `Value::repr_raw()`: a per-call-stack thread-local
/// tracks which container object ids are currently being formatted; a second
/// visit short-circuits to the CPython placeholder (`[...]` / `(...)` /
/// `{...}`).
pub(crate) fn render_value_repr(interp: &mut crate::Interpreter, value: &Value) -> Result<String> {
    // GenericAlias's core operations table cannot invoke user `__repr__`
    // methods. Delegate ordinary type arguments back through this renderer;
    // the builtins helper snapshots alias state before calling us, so a user
    // repr may safely re-enter the same alias.
    if pyrust_builtins::generic_alias::is_generic_alias(value) {
        return render_generic_alias_repr(interp, value);
    }

    // Dispatch __repr__ for user instances.
    if let ValueKind::PyInstance(instance) = value.kind() {
        let instance_rc = Rc::clone(instance);
        let class = Rc::clone(&instance_rc.borrow().class);
        if let Some(method_val) = lookup_class_attr(&class, "__repr__") {
            // Issue #1537: primitive types now have `object` as an explicit
            // MRO base, so `object.__repr__` is reachable for user subclasses
            // (e.g. `class MyList(list): pass`).  Skip the `object.__repr__`
            // sentinel when backing data is present — the backing-data path
            // below renders the contents correctly, matching CPython's
            // `list.__repr__`, `dict.__repr__`, etc. behaviour.
            let is_object_repr = crate::interpreter::value_is_canonical_slot(
                &method_val,
                crate::interpreter::CanonicalSlot::ObjectRepr,
            );
            // Builtin BaseException.__repr__ sentinel: render arg reprs with
            // interpreter dispatch when any arg is a PyInstance — core's
            // data-only exception_repr cannot honour a user __repr__
            // override on an arg (issue #2390 review).
            if matches!(method_val.kind(), ValueKind::BuiltinFunction(_))
                && pyrust_core::is_exception_instance(&instance_rc)
                && let Some(rendered) =
                    crate::interpreter::exception_repr_with_dispatch(interp, &instance_rc)?
            {
                return Ok(rendered);
            }
            if !is_object_repr || instance_builtin_data(&instance_rc).is_none() {
                let result =
                    invoke_class_method(interp, method_val, Value::py_instance(instance_rc), &[])?;
                return if matches!(result.kind(), ValueKind::Str(_)) {
                    Ok(result.as_str().unwrap_or("").to_string())
                } else {
                    Err(PyError::named(
                        "TypeError",
                        format!(
                            "__repr__ returned non-string (type {})",
                            pyrust_core::builtin_type_name(&result)
                        ),
                    ))
                };
            }
        }
        // Issue #1204: no __repr__ defined (or object.__repr__ skipped) —
        // if the instance has a scalar
        // primitive backing (str/int/float/bytes subclass), delegate repr()
        // to the backing value so that repr(MyInt(42)) gives "42" rather
        // than the default object repr.  (Counter/defaultdict/deque define
        // their own __repr__ as BuiltinFunctions; the lookup above handles
        // those; this path only fires when lookup returned None.)
        // Issue #1205: extend to container backings (list/dict/tuple/set
        // subclasses).  list/dict/tuple render the same as the backing
        // container.  set/frozenset subclasses prefix the class name:
        // `MySet({1, 2})` / `MySet()`, matching CPython's set_repr().
        if let Some(backing) = instance_builtin_data(&instance_rc) {
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
                ValueKind::Str(_)
                | ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Bool(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _)
                | ValueKind::Bytes(_) => return Ok(backing.repr_raw()),
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
                {
                    let class_name = class.borrow().name.clone();
                    let items = pyrust_builtins::frozenset::as_items(&backing);
                    let is_empty = items.as_ref().is_none_or(|rc| rc.is_empty());
                    if is_empty {
                        return Ok(format!("{class_name}()"));
                    }
                    // Render elements as `{e1, e2}` (without the outer
                    // `frozenset(...)` wrapper that render_value_repr adds).
                    let snapshot: Vec<_> = items.unwrap().iter().cloned().collect();
                    let mut parts = Vec::with_capacity(snapshot.len());
                    for k in &snapshot {
                        parts.push(render_key_repr(interp, k)?);
                    }
                    return Ok(format!("{class_name}({{{}}})", parts.join(", ")));
                }
                // bytearray subclass (#2386): CPython renders `ClassName(b'...')`
                // — the subclass name wrapping the bytes-content repr — unlike a
                // bytes subclass, which renders the bare base `b'...'` form.
                ValueKind::BuiltinObject { ops, .. }
                    if ops.canonical_class_tag()
                        == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
                {
                    if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing)
                    {
                        let class_name = class.borrow().name.clone();
                        let inner = Value::bytes(data).repr_raw();
                        return Ok(format!("{class_name}({inner})"));
                    }
                }
                _ => {}
            }
        }
        // No __repr__ defined — fall back to default object repr (handles
        // exception instances via exception_repr() and plain instances via
        // the address-based format).
        return Ok(value.repr_raw());
    }

    // Issue #2936: a `mappingproxy` over a dict subclass (or over another
    // proxy) renders as `mappingproxy(<repr of the proxied object>)`.  The
    // proxied object is typically a `PyInstance` whose repr needs interpreter
    // dispatch, which the built-in ops table cannot do, so intercept here.
    if let Some(owner) = pyrust_builtins::mapping_proxy::owner_of(value) {
        let inner = render_value_repr(interp, &owner)?;
        return Ok(format!("mappingproxy({inner})"));
    }

    // For containers, we need to recurse with interpreter access on each
    // element.  Use a thread-local cycle-detection stack identical in spirit
    // to the one in `Value::repr_raw()`.
    thread_local! {
        static REPR_IN_PROGRESS: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
    }

    match value.kind() {
        ValueKind::List(items) => {
            // Fast path: all elements are plain scalars — no interpreter
            // dispatch needed.  `Value::repr_raw()` handles cycle detection
            // internally and produces the same output without a snapshot.
            if !items.iter().any(value_needs_interp_repr) {
                drop(items);
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in =
                id.is_some_and(|id| REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id)));
            if already_in {
                return Ok("[...]".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            // Release the guard before calling user __repr__. CPython walks a
            // list live here: an element repr that appends to this list makes
            // the appended element part of the same representation.
            drop(items);
            let mut parts = Vec::new();
            let mut index = 0;
            loop {
                let item = value.as_list().and_then(|items| items.get(index).cloned());
                let Some(item) = item else {
                    break;
                };
                parts.push(render_value_repr(interp, &item)?);
                index += 1;
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            Ok(format!("[{}]", parts.join(", ")))
        }
        ValueKind::Tuple(items) => {
            // Fast path: all elements are plain scalars — no interpreter
            // dispatch needed.
            if !items.iter().any(value_needs_interp_repr) {
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in =
                id.is_some_and(|id| REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id)));
            if already_in {
                return Ok("(...)".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            // Tuple items are `&[Value]` — no Ref guard to drop.
            let snapshot: Vec<Value> = items.to_vec();
            let mut parts = Vec::with_capacity(snapshot.len());
            for item in &snapshot {
                parts.push(render_value_repr(interp, item)?);
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            let inner = parts.join(", ");
            if snapshot.len() == 1 {
                Ok(format!("({inner},)"))
            } else {
                Ok(format!("({inner})"))
            }
        }
        ValueKind::Dict(items) => {
            // Fast path: all keys and values are plain scalars — no interpreter
            // dispatch needed.
            if !items
                .iter()
                .any(|(k, v)| key_needs_interp_repr(k) || value_needs_interp_repr(v))
            {
                drop(items);
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in =
                id.is_some_and(|id| REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id)));
            if already_in {
                return Ok("{...}".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            // Release the guard before dispatch. Like CPython's dict repr,
            // continue the live insertion-order walk so entries appended by a
            // key/value __repr__ can be rendered by this same call.
            drop(items);
            let mut out = String::new();
            out.push('{');
            let mut index = 0;
            loop {
                let entry = value.dict_with(|items| {
                    items
                        .get_index(index)
                        .map(|(key, item)| (key.clone(), item.clone()))
                });
                let Some((k, v)) = entry.flatten() else {
                    break;
                };
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&render_key_repr(interp, &k)?);
                out.push_str(": ");
                out.push_str(&render_value_repr(interp, &v)?);
                index += 1;
            }
            out.push('}');
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            Ok(out)
        }
        ValueKind::Set(items) => {
            if items.is_empty() {
                return Ok("set()".to_string());
            }
            // Fast path: all elements are plain scalar keys — no interpreter
            // dispatch needed.
            if !items.iter().any(key_needs_interp_repr) {
                drop(items);
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in =
                id.is_some_and(|id| REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id)));
            if already_in {
                return Ok("{...}".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            let snapshot: Vec<PyKey> = items.iter().cloned().collect();
            drop(items);
            let mut parts = Vec::with_capacity(snapshot.len());
            for k in &snapshot {
                parts.push(render_key_repr(interp, k)?);
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            Ok(format!("{{{}}}", parts.join(", ")))
        }
        // Frozenset is stored as a BuiltinObject; its elements are PyKey so
        // they need render_key_repr to dispatch __repr__ on PyKey::Object.
        ValueKind::BuiltinObject { ops, .. }
            if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
        {
            let items = match pyrust_builtins::frozenset::as_items(value) {
                Some(rc) => rc,
                None => return Ok(value.repr_raw()),
            };
            if items.is_empty() {
                return Ok("frozenset()".to_string());
            }
            // Fast path: all elements are plain scalar keys — no interpreter
            // dispatch needed.
            if !items.iter().any(key_needs_interp_repr) {
                drop(items);
                return Ok(value.repr_raw());
            }
            let id = value.value_id();
            let already_in =
                id.is_some_and(|id| REPR_IN_PROGRESS.with(|c| c.borrow().contains(&id)));
            if already_in {
                return Ok("{...}".to_string());
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| c.borrow_mut().push(id));
            }
            let snapshot: Vec<PyKey> = items.iter().cloned().collect();
            drop(items);
            let mut parts = Vec::with_capacity(snapshot.len());
            for k in &snapshot {
                parts.push(render_key_repr(interp, k)?);
            }
            if let Some(id) = id {
                REPR_IN_PROGRESS.with(|c| {
                    let mut v = c.borrow_mut();
                    if let Some(pos) = v.iter().rposition(|&x| x == id) {
                        v.remove(pos);
                    }
                });
            }
            Ok(format!("frozenset({{{}}})", parts.join(", ")))
        }
        // Generators and built-in iterators (#2019): the pure
        // `Value::repr_raw()` cannot tell the concrete iterator kind apart
        // (all are `ValueKind::Generator`), so it returns a fixed
        // `<generator object>`.  Reconstruct CPython's real repr here:
        //   - true generators (def-generator / genexpr):
        //         `<generator object {qualname} at 0x...>`
        //   - everything else (map/filter/zip/enumerate/list_iterator/…):
        //         `<{type_name} object at 0x...>`
        ValueKind::Generator(_) => Ok(generator_repr(value)),
        // Issue #2771: `repr(cls)` dispatches `type(cls).__repr__(cls)` when the
        // class's metaclass defines a user `__repr__`.  `dispatch_metaclass_repr_str`
        // returns `None` for an ordinary class (metatype is the built-in `type`),
        // so the common case still uses the default `<class '...'>` format from
        // `Value::repr_raw()` with no interpreter dispatch.
        ValueKind::PyClass(cls_rc) => {
            let cls_rc = Rc::clone(cls_rc);
            if let Some(res) =
                crate::interpreter::dispatch_metaclass_repr_str(interp, &cls_rc, "__repr__")
            {
                return res;
            }
            Ok(value.repr_raw())
        }
        // For all other value types (int, float, str, bool, None, …), the
        // pure `Value::repr_raw()` is correct and needs no interpreter.
        _ => Ok(value.repr_raw()),
    }
}

/// Render the Python-visible repr/str of a GenericAlias. Argument repr and
/// class-module str dispatch share this path for repr(), str(), and print().
fn render_generic_alias_repr(interp: &mut Interpreter, value: &Value) -> Result<String> {
    pyrust_builtins::generic_alias::render_generic_alias_with(
        value,
        interp,
        render_value_repr,
        Interpreter::render_value_as_str,
    )
}
