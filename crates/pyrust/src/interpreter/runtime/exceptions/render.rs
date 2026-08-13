// Exception value string/repr rendering and sequence-iteration termination.
///
/// Does `err` signal end-of-sequence for the legacy `__getitem__`
/// iter protocol?  CPython terminates iteration on `IndexError`,
/// `StopIteration`, **and any subclass of those** raised from
/// `__getitem__`; anything else propagates to the caller.  Issue #394.
///
/// Subclass-aware: compare class identity against the canonical built-in
/// exception singletons. A user-defined class that merely reuses either name
/// must not terminate iteration.
pub(crate) fn is_sequence_iter_terminator(_interp: &Interpreter, err: &PyError) -> bool {
    if is_stop_iteration_error(err) {
        return true;
    }

    match err {
        // PyError::Named is the VM-internal raise shape that pre-dates
        // the PyInstance-backed exception path.  Match the canonical
        // built-in name directly — VM-internal raises never come from a user
        // subclass.
        PyError::Named(cls, _) => cls == "IndexError",
        PyError::Class(cls, _) => class_is_builtin_exception_subclass(cls, "IndexError"),
        PyError::Raised(exc) => match exc.kind() {
            ValueKind::PyInstance(inst) => {
                class_is_builtin_exception_subclass(&inst.borrow().class, "IndexError")
            }
            ValueKind::PyClass(cls) => class_is_builtin_exception_subclass(cls, "IndexError"),
            _ => false,
        },
        _ => false,
    }
}

/// `str()` of an exception instance that has no user-defined `__str__`.
/// Mostly the data-only `Value::to_py_str()` fallback, with one
/// interpreter-needing case split out: `KeyError.__str__` is
/// `repr(args[0])`, and a `PyInstance` key may carry a user `__repr__`
/// override that core cannot dispatch (issue #2389).  Shared by `str(x)` /
/// `print` (`render_instance_str`) and the format `!s` path
/// (`render_value_as_str`).
pub(crate) fn exception_str_with_dispatch(
    interp: &mut Interpreter,
    value: &Value,
    inst_rc: &Rc<RefCell<pyrust_core::PyInstance>>,
    class: &Rc<RefCell<pyrust_core::PyClass>>,
) -> Result<String> {
    let args = exception_instance_args(inst_rc);
    // Only exceptions using the GENERIC BaseException.__str__ take the
    // dispatch path: OSError / SyntaxError chains have dedicated __str__
    // formats built from str/int attrs (no user dunders involved) — core's
    // data-only renderer is exact for them.
    if args
        .iter()
        .any(|a| matches!(a.kind(), ValueKind::PyInstance(_)))
        && !pyrust_core::class_chain_contains_builtin_exception(class, "OSError")
        && !pyrust_core::class_chain_contains_builtin_exception(class, "SyntaxError")
    {
        // KeyError.__str__ = repr(args[0]); generic single-arg = str(args[0]);
        // multi-arg = repr of the args tuple ("(r0, r1, ...)").
        if pyrust_core::class_chain_contains_builtin_exception(class, "KeyError") {
            if let [arg] = args.as_slice() {
                return render_instance_repr(interp, arg);
            }
        } else if let [arg] = args.as_slice() {
            return interp.render_value_as_str(arg);
        }
        if args.len() > 1 {
            let mut parts = Vec::with_capacity(args.len());
            for a in &args {
                parts.push(render_instance_repr(interp, a)?);
            }
            return Ok(format!("({})", parts.join(", ")));
        }
    }
    Ok(value.to_py_str())
}

/// `repr()` of an exception instance whose args contain `PyInstance` values:
/// CPython's `BaseException.__repr__` is `Name(repr(a0), repr(a1), …)` with
/// each arg's `__repr__` dispatched — core's data-only `exception_repr` can
/// only handle the inherited-backing case (issue #2389/#2390 review).
/// Returns `None` when the data-only renderer is exact (no instance args).
pub(crate) fn exception_repr_with_dispatch(
    interp: &mut Interpreter,
    inst_rc: &Rc<RefCell<pyrust_core::PyInstance>>,
) -> Result<Option<String>> {
    let args = exception_instance_args(inst_rc);
    if !args
        .iter()
        .any(|a| matches!(a.kind(), ValueKind::PyInstance(_)))
    {
        return Ok(None);
    }
    let class_name = inst_rc.borrow().class.borrow().name.clone();
    let mut parts = Vec::with_capacity(args.len());
    for a in &args {
        parts.push(render_instance_repr(interp, a)?);
    }
    Ok(Some(format!("{class_name}({})", parts.join(", "))))
}

/// The exception instance's `args` tuple (empty when unset / not a tuple).
fn exception_instance_args(inst_rc: &Rc<RefCell<pyrust_core::PyInstance>>) -> Vec<Value> {
    let borrowed = inst_rc.borrow();
    match borrowed.attrs.get_slot("args").map(|v| v.kind()) {
        Some(ValueKind::Tuple(items)) => items.to_vec(),
        _ => Vec::new(),
    }
}

/// Renders a value using its `__repr__` dunder for the `!r` conversion flag in
/// `str.format`.  Mirrors the `repr()` builtin's dispatch: for `PyInstance`
/// values, looks up `__repr__` via MRO, calls it, and validates the return is a
/// `str`.  Generator-backed values use the interpreter renderer because their
/// public repr carries state that `value.repr_raw()` cannot see; other
/// non-instances fall back to `value.repr_raw()` unchanged.
///
/// Note: exception instances do not bypass `__repr__` here — CPython dispatches
/// `__repr__` on exceptions normally (only `__str__` has the special-case).
pub(super) fn render_instance_repr(interp: &mut Interpreter, value: &Value) -> Result<String> {
    // gh-95778: enforce int_max_str_digits for base-10 int->str (no-op unless
    // `value` is, or transitively contains, an over-limit BigInt).
    pyrust_core::check_int_str_conversion(value)?;
    // Issue #2771: `repr(cls)` dispatches `type(cls).__repr__(cls)` when the
    // class's metaclass defines a user `__repr__`.  Returns `None` for an
    // ordinary class, so plain classes fall through to `repr_raw()`'s default
    // `<class '...'>` format.
    if let ValueKind::PyClass(cls_rc) = value.kind() {
        let cls_rc = Rc::clone(cls_rc);
        if let Some(res) =
            crate::interpreter::dispatch_metaclass_repr_str(interp, &cls_rc, "__repr__")
        {
            return res;
        }
    }
    if matches!(value.kind(), ValueKind::Generator(_)) {
        return render_value_repr(interp, value);
    }
    let ValueKind::PyInstance(inst) = value.kind() else {
        return Ok(value.repr_raw());
    };
    let inst_rc = Rc::clone(inst);
    let class = Rc::clone(&inst_rc.borrow().class);
    if let Some(method_val) = lookup_class_attr(&class, "__repr__") {
        // Issue #1537: primitive types now expose `object` as an explicit
        // MRO base, so `object.__repr__` is reachable for user subclasses
        // (e.g. `class MyList(list): pass`).  Skip the `object.__repr__`
        // sentinel when the instance has a primitive backing store — the
        // backing-data path below renders the contents correctly, matching
        // CPython's `list.__repr__`, `dict.__repr__`, etc. behaviour.
        let is_object_repr = crate::interpreter::value_is_canonical_slot(
            &method_val,
            crate::interpreter::CanonicalSlot::ObjectRepr,
        );
        // Builtin BaseException.__repr__ sentinel: render arg reprs with
        // interpreter dispatch when any arg is a PyInstance — core's
        // data-only exception_repr cannot honour a user __repr__ override
        // on an arg (issue #2389/#2390 review).  A user-defined __repr__
        // on the exception class itself is not a BuiltinFunction and takes
        // the normal invoke below.
        if matches!(method_val.kind(), ValueKind::BuiltinFunction(_))
            && pyrust_core::is_exception_instance(&inst_rc)
            && let Some(rendered) = exception_repr_with_dispatch(interp, &inst_rc)?
        {
            return Ok(rendered);
        }
        if !is_object_repr || builtin_data_backing(value).is_none() {
            let result = invoke_class_method(
                interp,
                method_val,
                Value::py_instance(Rc::clone(&inst_rc)),
                &[],
            )?;
            return match result.kind() {
                ValueKind::Str(s) => Ok(s.to_string()),
                _ => Err(pyrust_core::type_err!(
                    "__repr__ returned non-string (type {})",
                    pyrust_core::builtin_type_name(&result)
                )),
            };
        }
    }
    // Issue #1205: no __repr__ in MRO (or object.__repr__ skipped above) —
    // delegate to backing container so that list/dict/tuple/set subclasses
    // render their contents rather than the generic `<ClassName object at
    // 0x...>` object repr.
    // Use render_value_repr (interp-aware) so that PyInstance elements
    // inside the backing container have their __repr__ called correctly.
    // Issue #1542: scalar backings (int/float/str/bytes subclasses) also
    // need to delegate to the backing value's repr() so that
    // `"%r" % MyInt(42)` returns "42" rather than the address repr.
    if let Some(backing) = builtin_data_backing(value) {
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
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Frozenset) =>
            {
                let class_name = class.borrow().name.clone();
                let items = pyrust_builtins::frozenset::as_items(&backing);
                let is_empty = items.as_ref().is_none_or(|rc| rc.is_empty());
                if is_empty {
                    return Ok(format!("{class_name}()"));
                }
                // Render elements as `{e1, e2}` without the outer `frozenset(...)`
                // Use render_key_repr (interp-aware) so PyKey::Object elements
                // have their user __repr__ called.
                let snapshot: Vec<_> = items.unwrap().iter().cloned().collect();
                let mut inner_elems = Vec::with_capacity(snapshot.len());
                for k in &snapshot {
                    inner_elems.push(render_key_repr(interp, k)?);
                }
                return Ok(format!("{class_name}({{{}}})", inner_elems.join(", ")));
            }
            // bytearray subclass (#2386): CPython renders `ClassName(b'...')`
            // — the subclass name wrapping the bytes-content repr — unlike a
            // bytes subclass, which renders the bare base `b'...'` form.
            ValueKind::BuiltinObject { ops, .. }
                if ops.canonical_class_tag() == Some(pyrust_core::CanonicalClassTag::Bytearray) =>
            {
                if let Some(data) = pyrust_builtins::bytearray::as_bytearray_snapshot(&backing) {
                    let class_name = class.borrow().name.clone();
                    // `Value::bytes(...).repr_raw()` renders the `b'...'` content
                    // form; wrap it in the subclass name.
                    let inner = Value::bytes(data).repr_raw();
                    return Ok(format!("{class_name}({inner})"));
                }
            }
            _ => {}
        }
    }
    Ok(value.repr_raw())
}

/// Escapes all non-ASCII characters in `s` using Python's `\xNN`, `\uNNNN`,
/// or `\UNNNNNNNN` notation.  This is the pure string-transformation step
/// used by `ascii_repr_interp`.
fn ascii_escape_str(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii() {
                vec![c]
            } else {
                let cp = c as u32;
                if cp <= 0xFF {
                    format!("\\x{cp:02x}").chars().collect()
                } else if cp <= 0xFFFF {
                    format!("\\u{cp:04x}").chars().collect()
                } else {
                    format!("\\U{cp:08x}").chars().collect()
                }
            }
        })
        .collect()
}

/// Interpreter-aware `ascii()` implementation.  Dispatches user `__repr__` for
/// `PyInstance` values (matching the behaviour of the `repr()` builtin), then
/// applies ASCII escaping to the resulting string.  Raises `TypeError` if
/// `__repr__` returns a non-string.
pub(crate) fn ascii_repr_interp(interp: &mut Interpreter, value: &Value) -> Result<String> {
    let repr_str = render_instance_repr(interp, value)?;
    Ok(ascii_escape_str(&repr_str))
}
