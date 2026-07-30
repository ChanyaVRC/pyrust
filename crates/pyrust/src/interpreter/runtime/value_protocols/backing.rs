pub(super) fn is_not_implemented(v: &Value) -> bool {
    matches!(v.kind(), ValueKind::NotImplemented)
}

/// Does a class-attribute value look like a callable method?  Accepts
/// both pure-Python user functions and the `BuiltinFunction` entries
/// that `pyrust_module!`'s `class { … }` block produces — anything
/// else (descriptor, raw int set via `Foo.x = 1`, …) should fall
/// through dunder dispatch without being invoked.  Issue #331 added
/// `BuiltinFunction` to the accepted set so Counter's `__add__`
/// participates in the binary-op path.
pub(super) fn is_callable_method(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::UserFunction(_) | ValueKind::BuiltinFunction(_)
    )
}

/// Whether a resolved class-attribute slot value is callable, in the same
/// sense as the `callable()` builtin.  A plain function / builtin function is
/// callable (the common case); issue #2054 additionally accepts a callable
/// *instance* (an object whose class defines `__call__`), a bound method, or a
/// class object as a dunder slot, matching CPython's "invoke whatever the slot
/// resolves to" behaviour.  `invoke_class_method` knows how to dispatch each of
/// these.  A slot that is *not* callable (`__add__ = 5`) is rejected by the
/// callers with `TypeError: 'int' object is not callable` (issue #2055).
pub(crate) fn slot_is_callable(v: &Value) -> bool {
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

/// Whether a resolved class-attribute slot value carries *descriptor*
/// semantics — i.e. CPython's special-method lookup binds it through
/// `__get__(instance, type(instance))` and then calls the **result** with the
/// slot's own arguments, instead of calling the slot value directly.
///
/// `_PyObject_LookupSpecial` / `lookup_maybe_method` are uniform about this:
/// after `_PyType_Lookup` finds the slot they consult `Py_TYPE(slot)->
/// tp_descr_get`, so a `property`, a `functools.cached_property`, or any user
/// object whose class defines `__get__` is bound exactly like a plain function
/// is.  `__len__ = property(lambda self: lambda: 4)` therefore runs the getter
/// with the instance and calls its `lambda` result — while
/// `__len__ = property(lambda self: 4)` runs the getter and then raises
/// `TypeError: 'int' object is not callable`, naming the *getter's result*
/// (issue #2944).
///
/// Deliberately **not** folded into [`slot_is_callable`]: the two answer
/// different questions, and the "not callable" error text is keyed on whichever
/// value is ultimately called.  Callability is also irrelevant to this test —
/// CPython binds a descriptor slot whether or not it is itself callable — so
/// callers must consult this *before* the callable check.
///
/// `UserFunction` / `BoundMethod` are excluded even though a Python function is
/// a descriptor: their binding is exactly "prepend the receiver", which
/// `invoke_class_method`'s own arms already do, and routing them here would
/// wrap every ordinary dunder call in a needless `__get__`.
///
/// Only a `BuiltinObject` (`property` / `cached_property`) or a `PyInstance`
/// can carry `__get__`, so the discriminant test is kept here, `#[inline]`, and
/// the actual probing lives out of line.  Every hot caller — the binary/unary
/// operator slot gates and the per-instantiation metaclass `__call__` gate —
/// sees a `UserFunction` or `BuiltinFunction` and folds to `false` with no call
/// at all; inlining the probes themselves instead cost ~6% on a user-`__dunder__`
/// loop, which constructs an instance per iteration.
#[inline]
pub(crate) fn slot_is_descriptor(v: &Value) -> bool {
    match v.kind() {
        ValueKind::BuiltinObject { .. } | ValueKind::PyInstance(_) => slot_is_descriptor_probe(v),
        _ => false,
    }
}

#[inline(never)]
fn slot_is_descriptor_probe(v: &Value) -> bool {
    match v.kind() {
        // A `property` in a partial slot (`.getter` / `.setter` builder state)
        // is not a live descriptor; mirror `call_descriptor_get`'s gate.
        ValueKind::BuiltinObject { .. } => {
            pyrust_builtins::property::property_partial_slot(v) == Some(None)
                || pyrust_builtins::cached_property::with_cached_property(v, |_| ()).is_some()
        }
        ValueKind::PyInstance(inst) => {
            let class = Rc::clone(&inst.borrow().class);
            lookup_class_attr(&class, "__get__").is_some()
        }
        _ => false,
    }
}

/// Whether `invoke_class_method` can dispatch this slot value at all — it is
/// either directly callable or a descriptor that binds to something callable.
///
/// Slot pre-checks that reject a non-callable slot up front (`__add__ = 5`,
/// `__neg__ = 5`, `__hash__ = 5`) must use this rather than
/// [`slot_is_callable`], or a descriptor slot is rejected before it is ever
/// bound and its getter never runs (issue #2944).  The error itself stays in
/// `invoke_class_method`, which alone knows the post-binding value to name.
#[inline]
pub(crate) fn slot_is_dispatchable(v: &Value) -> bool {
    slot_is_callable(v) || slot_is_descriptor(v)
}

/// Look up a user-defined special method on a Python-level value.
///
/// Instance special methods live on the instance's class, while special
/// methods for a class object live on its metaclass. Keeping that distinction
/// at the value-protocol boundary prevents individual numeric consumers from
/// accidentally ignoring metaclass slots.
pub(crate) fn lookup_value_special_method(value: &Value, name: &str) -> Option<Value> {
    match value.kind() {
        ValueKind::PyInstance(instance) => {
            let class = Rc::clone(&instance.borrow().class);
            lookup_class_attr(&class, name)
        }
        ValueKind::PyClass(class) => metaclass_dunder(class, name),
        _ => None,
    }
}

/// Normalize an `__int__` / `__trunc__` result to a plain integer value.
///
/// CPython temporarily accepts builtin subclasses returned by these slots
/// (while warning about the deprecated return shape). PyRust does not model
/// that warning yet, but must still unwrap the accepted value. `None` means
/// the original result was not an integer family member; callers retain the
/// original value for their slot-specific diagnostic.
pub(crate) fn normalize_int_slot_result(result: &Value) -> Option<Value> {
    let candidate = match result.kind() {
        ValueKind::Int(_) | ValueKind::Bool(_) | ValueKind::BigInt(_) => result.clone(),
        ValueKind::PyInstance(_) => coerce_subclass_backing(result, &[])?,
        _ => return None,
    };
    match candidate.kind() {
        ValueKind::Int(value) => Some(Value::int(value)),
        ValueKind::Bool(value) => Some(Value::int(value as i64)),
        ValueKind::BigInt(value) => Some(Value::bigint(value.clone())),
        _ => None,
    }
}

/// Normalize an `__float__` result to a plain float, accepting a float
/// subclass through its builtin backing while leaving wrong subclasses
/// distinguishable to the caller.
pub(crate) fn normalize_float_slot_result(result: &Value) -> Option<Value> {
    let candidate = match result.kind() {
        ValueKind::Float(_) => result.clone(),
        ValueKind::PyInstance(_) => coerce_subclass_backing(result, &[])?,
        _ => return None,
    };
    match candidate.kind() {
        ValueKind::Float(value) => Some(Value::float(value)),
        _ => None,
    }
}

/// Normalize an `__complex__` result to a plain complex, accepting a complex
/// subclass through its builtin backing.
pub(crate) fn normalize_complex_slot_result(result: &Value) -> Option<Value> {
    let candidate = match result.kind() {
        ValueKind::Complex(_, _) => result.clone(),
        ValueKind::PyInstance(_) => coerce_subclass_backing(result, &[])?,
        _ => return None,
    };
    match candidate.kind() {
        ValueKind::Complex(real, imag) => Some(Value::complex(real, imag)),
        _ => None,
    }
}

pub(crate) fn coerce_numeric(v: &Value) -> Value {
    // Extract via kind() in a scope so the borrow is dropped before we
    // clone `v` in the fallthrough — #450 made `kind()`'s borrow
    // explicit, so we can't hold a borrow while returning an owned Value.
    if let ValueKind::Bool(b) = v.kind() {
        return Value::int(b as i64);
    }
    // Issue #1204: PyInstance subclasses of int/float/str/bytes carry their
    // underlying primitive value as `__builtin_data__`.  Extract it here so
    // that arithmetic and concatenation operations on bare subclass instances
    // (e.g. `MyInt(42) + 1`) fall through to the primitive fast paths below.
    // This mirrors CPython's slot delegation for `tp_as_number` / `tp_as_sequence`.
    if let Some(backing) = builtin_data_backing(v) {
        let is_scalar = matches!(
            backing.kind(),
            ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Float(_)
                | ValueKind::Complex(_, _)
                | ValueKind::Str(_)
                | ValueKind::Bytes(_)
        );
        if is_scalar {
            return backing;
        }
    }
    v.clone()
}

/// Like [`coerce_numeric`] but also unwraps *container* subclass backings
/// (list/tuple/dict/set/frozenset).  Used by the `+`/`*`/`<` operator paths
/// where a user dunder override was already dispatched upstream (so no
/// override check is needed here) and the result type should follow the base
/// type (`L([1]) + [2]` → plain `list`).
///
/// Hot-path: a single `as_py_instance_rc()` tag check.  Concrete operands
/// (the common `[1,2] + [3,4]` / `5 + 6` case) take the `Bool`-then-clone
/// fall-through with no extra instance probe — identical cost to the bare
/// `coerce_numeric` it replaced.
#[inline]
pub(crate) fn coerce_operand_backing(v: &Value) -> Value {
    if let ValueKind::Bool(b) = v.kind() {
        return Value::int(b as i64);
    }
    if let Some(backing) = builtin_data_backing(v) {
        let is_primitive = matches!(
            backing.kind(),
            ValueKind::Int(_)
                | ValueKind::BigInt(_)
                | ValueKind::Float(_)
                | ValueKind::Str(_)
                | ValueKind::Bytes(_)
                | ValueKind::List(_)
                | ValueKind::Tuple(_)
                | ValueKind::Dict(_)
                | ValueKind::Set(_)
        ) || pyrust_builtins::frozenset::as_items(&backing).is_some()
            || pyrust_builtins::bytearray::as_bytearray_snapshot(&backing).is_some();
        if is_primitive {
            return backing;
        }
    }
    v.clone()
}

/// Extract the primitive backing of a builtin-subclass `PyInstance` —
/// scalars (int/float/str/bytes) AND containers (list/tuple/dict/set/
/// frozenset) — but only when the subclass does NOT override the relevant
/// dunder(s) with a user method.  Returns `Some(backing)` when the value is
/// such a subclass using inherited builtin behaviour; `None` otherwise.
///
/// This is the container-aware analogue of [`coerce_numeric`] used by the
/// `+`/`*`/`==`/ordering/hashing paths so that, e.g., `L([1]) + [2]` (where
/// `L` subclasses `list`) operates on the backing list and yields a plain
/// `list`, matching CPython's inherited-slot semantics (#1929/#1934/#1936/
/// #1939).
///
/// `override_dunders` lists the user-method names that, if present on the
/// subclass MRO, mean the subclass customises this operation and the backing
/// must NOT be used (the override wins).  `lookup_class_attr` only finds
/// *user*-defined dunders (builtin dunders aren't exposed as class attrs —
/// #1909), so a `Some` result there reliably indicates an override.
///
/// Hot-path note: the only work for a non-`PyInstance` operand is the
/// `as_py_instance_rc()` tag check, which returns `None` immediately — the
/// dunder lookups run only for actual subclass instances.
pub(crate) fn coerce_subclass_backing(v: &Value, override_dunders: &[&str]) -> Option<Value> {
    // Thin alias for the unified representation-substitutability boundary
    // (issue #2386).  `effective_builtin_receiver` covers every builtin backing
    // — scalars, containers, AND `BuiltinObject`-backed `bytearray`/`frozenset`
    // — with the same inherited-vs-overridden dunder gate this op-path needs.
    effective_builtin_receiver(v, override_dunders)
}
