/// Single-class `isinstance` check — `obj` against one concrete class
/// value (i.e. *not* a tuple).  Issue #462: the 11 migrated primitive
/// types (`int`, `str`, `list`, …) are real `PyClass` values now, so
/// their `isinstance` resolves through the standard `class_is_subclass_of`
/// walk — no per-type hard-coded arms.  Only `NoneType` and `BuiltinObject`
/// (frozenset, range, enumerate, …) still take the legacy
/// `BuiltinFunction(name)` path until they're migrated too.
///
/// Note: `isinstance_check` dispatches `__instancecheck__` for ABC classes
/// before reaching this function, so `cls` here is never an ABC class when
/// called from `isinstance_check`.  `isinstance_single` is also called from
/// other internal sites that do not go through `isinstance_check`.
fn isinstance_single(obj: &Value, cls: &Value) -> bool {
    // Migrated primitives: `type(obj)` returns the per-thread PyClass
    // singleton, so a class-vs-class walk handles every primitive check
    // (including `bool` → `int` via base inheritance).
    if let ValueKind::PyClass(expected) = cls.kind() {
        // Deprecated `typing.List`/`typing.Dict`/… aliases (#2601): delegate
        // the check to the underlying builtin (`list`, `dict`, …) so
        // `isinstance([], typing.List)` behaves like `isinstance([], list)`.
        if let Some(delegate) = crate::builtin_modules::typing::legacy_alias_delegate(expected) {
            return isinstance_single(obj, &delegate);
        }
        // Fast path: `object` is the universal base — every Python value
        // is an instance of `object`.  Check before the primitive-class
        // dispatch so that `isinstance(None, object)`,
        // `isinstance(print, object)`, etc. all return `True`.
        if Rc::ptr_eq(expected, &crate::interpreter::object_class_singleton()) {
            return true;
        }
        // Fast path: `type` is the metaclass — every class is an instance of
        // `type` in CPython: `isinstance(int, type)` is True,
        // `isinstance(42, type)` is False (issue #1312).
        if Rc::ptr_eq(expected, &type_class_singleton()) {
            return matches!(obj.kind(), ValueKind::PyClass(_));
        }
        // Fast path: if `expected` is one of the 11 primitive class
        // singletons, do a direct `ValueKind` tag check.  Skips the
        // `primitive_class_for_value` thread_local + Rc::clone + the
        // base-chain walk, recovering most of the master-vs-PR
        // `isinstance` regression (#462).
        if let Some(hit) = crate::interpreter::primitive_class_isinstance_fast(obj, expected) {
            return hit;
        }
        // The variants whose class is a single-expression lookup answer here.
        // Routing them through `value_class` instead measured 1.15–1.18x slower
        // on tight `isinstance(f, C)` / `isinstance(C, D)` loops: that call is
        // opaque to the inliner and round-trips the class through a `Value`.
        let actual_class = match obj.kind() {
            ValueKind::PyInstance(inst) => Some(Rc::clone(&inst.borrow().class)),
            ValueKind::BoundMethod { .. } | ValueKind::ClassBoundMethod { .. } => {
                Some(method_type_singleton())
            }
            ValueKind::UserFunction(f)
                if !matches!(
                    f.kind,
                    UserFunctionKind::StaticMethod | UserFunctionKind::ClassMethod
                ) =>
            {
                Some(function_type_singleton())
            }
            // Issue #1626: a class object is an instance of its metatype.
            // When the class has no custom metatype (None), fall back to the
            // `type` singleton so `isinstance(int, type)` etc. still works.
            ValueKind::PyClass(cls_rc) => {
                let meta = cls_rc.borrow().metatype.clone();
                Some(meta.unwrap_or_else(type_class_singleton))
            }
            // Everything with a non-trivial mapping — built-in iterators
            // (`zip` / `map` / `filter` / `enumerate` / `reversed`), a provider
            // iterator retaining the class generation that created it, `slice`,
            // `types.GenericAlias`, and the primitives — defers to `type()`'s
            // own table rather than keeping a second copy of it here that could
            // drift.  None of these is on a measurable hot path.
            _ => crate::interpreter::value_class_object(obj),
        };
        if let Some(actual) = actual_class {
            return class_is_subclass_of(&actual, expected);
        }
        return false;
    }
    // Non-class `cls` operands are an error at the API boundary
    // (`isinstance_check` rejects them); the only remaining match here is
    // the legacy `BuiltinFunction(name)` path for types that haven't been
    // migrated to PyClass yet.
    match (obj.kind(), cls.kind()) {
        (ValueKind::UserFunction(f), ValueKind::BuiltinFunction("staticmethod")) => {
            f.kind == UserFunctionKind::StaticMethod
        }
        (ValueKind::UserFunction(f), ValueKind::BuiltinFunction("classmethod")) => {
            f.kind == UserFunctionKind::ClassMethod
        }
        (ValueKind::BuiltinObject { ops, .. }, ValueKind::BuiltinFunction(name)) => {
            ops.type_name() == name
        }
        // Generators / coroutines / async generators (and built-in iterators
        // such as `zip`, `enumerate`, …) report their CPython type via the
        // by-name `BuiltinFunction` sentinel that `type()` returns.  Match the
        // same name so `isinstance(g, types.GeneratorType)`,
        // `isinstance(c, types.CoroutineType)`, etc. hold (#2777).
        (ValueKind::Generator(_), ValueKind::BuiltinFunction(name)) => {
            full_type_name_str(obj) == *name
        }
        _ => false,
    }
}

/// True if `inst`'s class is a (proper or improper) subclass of the built-in
/// `dict` type.  Used by `dict()` to drive the `keys()` + `__getitem__`
/// mapping-conversion path for dict subclasses (e.g. `collections.Counter`)
/// that keep their backing map in a custom attr rather than
/// `__builtin_data__` (issue #2010).
fn is_dict_subclass_instance(inst: &Rc<RefCell<crate::value::PyInstance>>) -> bool {
    let class = Rc::clone(&inst.borrow().class);
    match crate::interpreter::primitive_class_by_name("dict") {
        Some(dict_class) => class_is_subclass_of(&class, &dict_class),
        None => false,
    }
}

/// `isinstance(obj, classinfo)` — accept a class *or* an
/// arbitrarily-nested tuple of classes or `UnionType`, matching CPython's
/// recursive contract.  Raises `TypeError` if a leaf is neither a class nor a
/// tuple.  See <https://docs.python.org/3/library/functions.html#isinstance>.
fn isinstance_check(
    fn_name: &str,
    obj: &Value,
    cls: &Value,
    interp: &mut crate::Interpreter,
) -> Result<bool> {
    if let ValueKind::Tuple(items) = cls.kind() {
        let items: Vec<Value> = items.to_vec();
        for item in &items {
            if isinstance_check(fn_name, obj, item, interp)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    // PEP 604: `isinstance(x, int | str)` — unwrap UnionType to its __args__.
    if let Some(args) = pyrust_builtins::union_type::union_type_args(cls) {
        if let ValueKind::Tuple(items) = args.kind() {
            let items: Vec<Value> = items.to_vec();
            for item in &items {
                if isinstance_check(fn_name, obj, item, interp)? {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    // `isinstance(x, typing.Union[int, str])` — CPython 3.12 accepts a
    // `typing.Union[...]` alias as the second arg, treating it like the tuple
    // of its `__args__`.  Detect the alias by its origin being the `Union`
    // special form and recurse over its members.
    if let Some(args) = pyrust_builtins::generic_alias::as_typing_union_args(cls) {
        if let ValueKind::Tuple(items) = args.kind() {
            let items: Vec<Value> = items.to_vec();
            for item in &items {
                if isinstance_check(fn_name, obj, item, interp)? {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    // Issue #2525: when `cls` is a plain instance (not a class) whose *type*
    // defines `__instancecheck__`, CPython invokes
    // `type(cls).__instancecheck__(cls, obj)` rather than rejecting it.  The
    // special method is looked up on the type, so resolve it on the instance's
    // class MRO before applying the `is_class_like` guard.  `get_attr` binds the
    // method to the instance receiver, so calling it with `[obj]` yields the
    // `(cls, obj)` argument pairing CPython uses.
    if let ValueKind::PyInstance(inst) = cls.kind() {
        let inst_class = Rc::clone(&inst.borrow().class);
        if crate::interpreter::lookup_class_attr(&inst_class, "__instancecheck__").is_some() {
            let ic_fn = interp.get_attr(cls, "__instancecheck__")?;
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: obj.clone(),
            }];
            let result = interp.call_function_expanded(ic_fn, &call_args)?;
            return interp.truthy_value(&result);
        }
    }
    if !is_class_like(cls) {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() arg 2 must be a type, a tuple of types, or a union"),
        ));
    }
    // Dispatch through __instancecheck__ when cls is a PyClass that defines it
    // (e.g. all ABC classes). We look up the attr via get_attr so its explicit
    // classmethod descriptor binds the ABC before the hook is called.
    if let ValueKind::PyClass(cls_rc) = cls.kind() {
        // Fast path: when `cls` is one of the 11 primitive class singletons
        // (`int`, `str`, …) a direct `ValueKind` tag check settles the result
        // without the `metaclass_dunder` / `__instancecheck__` / Protocol
        // probing below.  Primitives can never carry those hooks nor be a
        // Protocol subclass, so this both preserves the hot `isinstance(x, int)`
        // path and absorbs the cost of the #2526 Protocol check added later.
        if let Some(hit) = crate::interpreter::primitive_class_isinstance_fast(obj, cls_rc) {
            return Ok(hit);
        }
        // Issue #1955: a metaclass `__instancecheck__` override takes
        // precedence, mirroring CPython's `type(cls).__instancecheck__(cls, x)`
        // dispatch.  `metaclass_dunder` returns `Some` only for a user
        // override, so ordinary classes skip this and keep the fast path.
        //
        // Issue #2939: bind through `invoke_class_method` so a `staticmethod` /
        // `classmethod` hook follows the same descriptor rules as every other
        // implicit dunder instead of unconditionally receiving `cls`.
        if let Some(ic_fn) = crate::interpreter::metaclass_dunder(cls_rc, "__instancecheck__")
            && matches!(ic_fn.kind(), ValueKind::UserFunction(_))
        {
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: obj.clone(),
            }];
            let result = crate::interpreter::invoke_class_method(
                interp,
                ic_fn,
                Value::py_class(Rc::clone(cls_rc)),
                &call_args,
            )?;
            return interp.truthy_value(&result);
        }
        // Legacy ABC path: ABC classes store `__instancecheck__` directly in
        // their own attrs dict (not on a metaclass).
        let has_ic = cls_rc.borrow().attrs.contains_key("__instancecheck__");
        if has_ic {
            let cls_val = Value::py_class(Rc::clone(cls_rc));
            let ic_fn = interp.get_attr(&cls_val, "__instancecheck__")?;
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: obj.clone(),
            }];
            let result = interp.call_function_expanded(ic_fn, &call_args)?;
            return interp.truthy_value(&result);
        }
        // Issue #2526: structural `isinstance` for `typing.Protocol` subclasses.
        // `@runtime_checkable` records the required member names in
        // `__protocol_attrs__`; the subject is an instance iff it has every one
        // of them (`hasattr` semantics).  A Protocol subclass that was NOT
        // decorated raises, matching CPython 3.12's `_ProtocolMeta`.  The bare
        // `Protocol` class itself is skipped so it keeps ordinary behaviour.
        //
        // Primitive classes (`int`, `str`, …) can never be Protocol subclasses,
        // so a single pointer-keyed dispatch-table lookup short-circuits the
        // recursive `is_protocol_subclass` base-chain walk on the hot
        // `isinstance(x, int)` path (keeps the check perf-neutral, #2526).
        if !crate::interpreter::is_primitive_class(cls_rc)
            && crate::builtin_modules::typing::is_protocol_subclass(cls_rc)
            && !crate::builtin_modules::typing::is_protocol_marker_class(cls_rc)
        {
            return protocol_structural_isinstance(obj, cls_rc);
        }
    }
    Ok(isinstance_single(obj, cls))
}

/// Structural `isinstance(obj, P)` for a `typing.Protocol` subclass `cls_rc`
/// (issue #2526).  Requires `@runtime_checkable` (a `__protocol_attrs__` /
/// `__protocol_runtime_checkable__` pair recorded by the decorator); otherwise
/// raises the CPython 3.12 `TypeError`.  Returns `True` iff `obj` statically has
/// every name in `__protocol_attrs__`.  `isinstance` permits data-member
/// protocols (unlike `issubclass`), so no data-member guard here.
fn protocol_structural_isinstance(obj: &Value, cls_rc: &Rc<RefCell<PyClass>>) -> Result<bool> {
    require_runtime_checkable(cls_rc)?;
    // `isinstance` resolves members on the subject's *type* (issue #2551).  When
    // the subject is itself a class, that type is its metaclass — so a member
    // supplied by the metaclass counts, matching `getattr_static`.
    Ok(protocol_members_present(obj, cls_rc, false))
}

/// Structural `issubclass(cls, P)` for a `typing.Protocol` subclass `cls_rc`
/// (issue #2552).  Like `isinstance`, but the subject is the candidate *class*
/// rather than an instance, so member presence is checked across the candidate's
/// own MRO.  CPython 3.12 forbids `issubclass` against a protocol that declares
/// any non-method (data) member, raising `TypeError` even before the structural
/// walk; `isinstance` is still allowed for such protocols.
fn protocol_structural_issubclass(
    candidate: &Value,
    cls_rc: &Rc<RefCell<PyClass>>,
) -> Result<bool> {
    require_runtime_checkable(cls_rc)?;
    let non_callable = protocol_attr_names(
        crate::interpreter::lookup_class_attr(cls_rc, "__non_callable_proto_members__").as_ref(),
    );
    if !non_callable.is_empty() {
        return Err(PyError::named(
            "TypeError",
            "Protocols with non-method members don't support issubclass()".to_string(),
        ));
    }
    // `issubclass` checks the candidate *class*'s own MRO — the class is its own
    // lookup target, NOT its metaclass.  `isinstance(C, P)` and `issubclass(C, P)`
    // therefore differ when a member lives on the metaclass (CPython 3.12).
    Ok(protocol_members_present(candidate, cls_rc, true))
}

/// Shared guard for both Protocol checks: a Protocol subclass must be
/// `@runtime_checkable` (carry `__protocol_runtime_checkable__ == True`) before it
/// can be used with `isinstance`/`issubclass`, matching CPython 3.12's
/// `_ProtocolMeta`.
fn require_runtime_checkable(cls_rc: &Rc<RefCell<PyClass>>) -> Result<()> {
    let runtime_checkable =
        crate::interpreter::lookup_class_attr(cls_rc, "__protocol_runtime_checkable__")
            .is_some_and(|v| matches!(v.kind(), ValueKind::Bool(true)));
    if !runtime_checkable {
        return Err(PyError::named(
            "TypeError",
            "Instance and class checks can only be used with @runtime_checkable protocols"
                .to_string(),
        ));
    }
    Ok(())
}

/// Shared member-presence walk for `isinstance`/`issubclass` against a
/// `@runtime_checkable` Protocol.  `subject` is the instance (isinstance) or the
/// candidate class (issubclass).  Every name in `__protocol_attrs__` must resolve
/// via static attribute lookup (issue #2551 — bypassing `__getattr__` and
/// descriptors).  Missing/empty `__protocol_attrs__` matches everything, mirroring
/// CPython for an attribute-free protocol body.
///
/// `subject_is_class` selects the lookup target: for `issubclass` the subject is
/// the candidate class itself (walk its own MRO), while for `isinstance` the
/// subject is resolved to its type — its metaclass when it happens to be a class.
fn protocol_members_present(
    subject: &Value,
    cls_rc: &Rc<RefCell<PyClass>>,
    subject_is_class: bool,
) -> bool {
    let attrs = crate::interpreter::lookup_class_attr(cls_rc, "__protocol_attrs__");
    let names: Vec<String> = protocol_attr_names(attrs.as_ref());
    // CPython 3.12 treats a member that resolves to `None` as absent unless the
    // member is a declared non-callable (data) member.  `runtime_checkable`
    // records the non-callable subset in `__non_callable_proto_members__`.
    let non_callable = protocol_attr_names(
        crate::interpreter::lookup_class_attr(cls_rc, "__non_callable_proto_members__").as_ref(),
    );
    for name in &names {
        // Issue #2551: CPython's `_ProtocolMeta` resolves each member with
        // `inspect.getattr_static` semantics — it scans the instance `__dict__`
        // and the type's MRO dicts directly, never invoking `__getattr__` or
        // descriptor `__get__`.  A dynamic `get_attr` probe both over-matches
        // (`__getattr__`-supplied attrs count as present) and lets a raising
        // `__getattr__` abort the check.  `has_static_attr` never raises.
        match has_static_attr(subject, name, subject_is_class) {
            None => return false,
            Some(val) => {
                if matches!(val.kind(), ValueKind::None) && !non_callable.iter().any(|n| n == name)
                {
                    // A callable (method) member resolved to `None` → absent.
                    return false;
                }
            }
        }
    }
    true
}

/// Resolve attribute `name` on `value` the way CPython's `inspect.getattr_static`
/// does: consult the instance's own `__dict__` first (for a `PyInstance`), then
/// each class in the MRO's own attribute dict directly, without invoking
/// `__getattr__` or descriptor `__get__`.  Returns the raw stored `Value` if
/// found, else `None`.  Never raises — a missing attribute, or a `__getattr__`
/// that would raise, is simply "absent" (issues #2551 / #2552).
///
/// `value_is_class` selects the lookup target when `value` is a class:
/// `issubclass(C, P)` (`true`) walks `C`'s own MRO, treating `C` as the lookup
/// target; `isinstance(C, P)` (`false`) resolves `C`'s type — its metaclass — so
/// a protocol member supplied by the metaclass counts, matching CPython's
/// `getattr_static(C, name)` which searches the metaclass MRO.
fn has_static_attr(value: &Value, name: &str, value_is_class: bool) -> Option<Value> {
    // Instance `__dict__` shadows the class, matching attribute resolution order.
    if let ValueKind::PyInstance(inst) = value.kind()
        && let Some(v) = inst.borrow().attrs.get(name)
    {
        return Some(v.clone());
    }
    // Class-side static walk via `lookup_class_attr`, which reads each class's own
    // `attrs` dict directly along the C3 MRO — no `__getattr__`, no descriptor
    // binding.
    if let ValueKind::PyClass(cls_rc) = value.kind() {
        // The subject is a class.  `issubclass(C, P)` checks `C`'s own MRO only.
        if value_is_class {
            return crate::interpreter::lookup_class_attr(cls_rc, name);
        }
        // `isinstance(C, P)` mirrors `getattr_static(C, name)`, which searches both
        // `C`'s own MRO and `C`'s metaclass MRO (a classmethod on `C` and a method
        // on the metaclass both satisfy the protocol).
        if let Some(v) = crate::interpreter::lookup_class_attr(cls_rc, name) {
            return Some(v);
        }
        if let ValueKind::PyClass(meta_rc) = value_class(value).kind() {
            return crate::interpreter::lookup_class_attr(meta_rc, name);
        }
        return None;
    }
    // Non-class subject: resolve its type — the instance's class for a
    // `PyInstance`, or the primitive-type singleton for `list`/`int`/… so e.g.
    // `isinstance([], Sized)` still sees `list.__len__`.
    if let ValueKind::PyClass(cls_rc) = value_class(value).kind() {
        return crate::interpreter::lookup_class_attr(cls_rc, name);
    }
    None
}

/// Extract the string names from a Protocol `set`-valued bookkeeping attribute
/// (`__protocol_attrs__` / `__non_callable_proto_members__`).  A missing or
/// non-`set` value yields an empty list.
fn protocol_attr_names(attr: Option<&Value>) -> Vec<String> {
    match attr.map(|v| v.kind()) {
        Some(ValueKind::Set(items)) => items
            .iter()
            .filter_map(|k| match k {
                pyrust_core::PyKey::Str(v) => v.as_str().map(|s| s.to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `issubclass(cls, classinfo)` — same tuple-recursive contract as
/// `isinstance_check`, but compares classes rather than instances.
/// Dispatches through `__subclasscheck__` for PyClass leaves (e.g. ABC
/// classes), mirroring CPython's `type.__subclasscheck__` dispatch.
fn issubclass_check(
    fn_name: &str,
    cls: &Value,
    classinfo: &Value,
    interp: &mut crate::Interpreter,
) -> Result<bool> {
    if let ValueKind::Tuple(items) = classinfo.kind() {
        let items: Vec<Value> = items.to_vec();
        for item in &items {
            if issubclass_check(fn_name, cls, item, interp)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    // Deprecated `typing.List`/`typing.Dict`/… aliases (#2601): delegate the
    // check to the underlying builtin so `issubclass(list, typing.List)`
    // behaves like `issubclass(list, list)`.
    if let ValueKind::PyClass(classinfo_rc) = classinfo.kind()
        && let Some(delegate) = crate::builtin_modules::typing::legacy_alias_delegate(classinfo_rc)
    {
        return issubclass_check(fn_name, cls, &delegate, interp);
    }
    // PEP 604: `issubclass(X, int | str)` — unwrap UnionType to its __args__.
    if let Some(args) = pyrust_builtins::union_type::union_type_args(classinfo) {
        if let ValueKind::Tuple(items) = args.kind() {
            let items: Vec<Value> = items.to_vec();
            for item in &items {
                if issubclass_check(fn_name, cls, item, interp)? {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    // `issubclass(X, typing.Union[int, str])` — accept a `typing.Union[...]`
    // alias as the second arg, treating it like the tuple of its `__args__`
    // (CPython 3.12).
    if let Some(args) = pyrust_builtins::generic_alias::as_typing_union_args(classinfo) {
        if let ValueKind::Tuple(items) = args.kind() {
            let items: Vec<Value> = items.to_vec();
            for item in &items {
                if issubclass_check(fn_name, cls, item, interp)? {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }
    // Dispatch through __subclasscheck__ when classinfo is a PyClass that
    // defines it (e.g. all ABC classes).  This handles structural subtyping
    // for `issubclass(UserClass, Iterable)` and tuple forms like
    // `issubclass(UserClass, (Iterable, Hashable))` (fixes #1799).
    if let ValueKind::PyClass(classinfo_rc) = classinfo.kind() {
        // Issue #1955: a metaclass `__subclasscheck__` override takes
        // precedence, mirroring CPython's
        // `type(classinfo).__subclasscheck__(classinfo, cls)` dispatch.
        // Issue #2939: bind through `invoke_class_method` so a `staticmethod` /
        // `classmethod` hook follows the same descriptor rules as every other
        // implicit dunder instead of unconditionally receiving `classinfo`.
        if let Some(sc_fn) = crate::interpreter::metaclass_dunder(classinfo_rc, "__subclasscheck__")
            && matches!(sc_fn.kind(), ValueKind::UserFunction(_))
        {
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: cls.clone(),
            }];
            let result = crate::interpreter::invoke_class_method(
                interp,
                sc_fn,
                Value::py_class(Rc::clone(classinfo_rc)),
                &call_args,
            )?;
            return interp.truthy_value(&result);
        }
        // Legacy ABC path: ABC classes store `__subclasscheck__` directly in
        // their own attrs dict (not on a metaclass).
        let has_sc = classinfo_rc
            .borrow()
            .attrs
            .contains_key("__subclasscheck__");
        if has_sc {
            let classinfo_val = Value::py_class(Rc::clone(classinfo_rc));
            let sc_fn = interp.get_attr(&classinfo_val, "__subclasscheck__")?;
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: cls.clone(),
            }];
            let result = interp.call_function_expanded(sc_fn, &call_args)?;
            return interp.truthy_value(&result);
        }
    }
    // Issue #2525: when `classinfo` is a plain instance (not a class) whose
    // *type* defines `__subclasscheck__`, CPython invokes
    // `type(classinfo).__subclasscheck__(classinfo, cls)` rather than raising
    // `TypeError`.  Resolve the hook on the instance's class MRO before the
    // match's `arg 2 must be a class` fallback.  `get_attr` binds the method to
    // the instance receiver, so calling it with `[cls]` yields the
    // `(classinfo, cls)` pairing CPython uses.
    if let ValueKind::PyInstance(inst) = classinfo.kind() {
        let inst_class = Rc::clone(&inst.borrow().class);
        if crate::interpreter::lookup_class_attr(&inst_class, "__subclasscheck__").is_some() {
            let sc_fn = interp.get_attr(classinfo, "__subclasscheck__")?;
            let call_args = [crate::interpreter::ExpandedCallArg {
                name: None,
                value: cls.clone(),
            }];
            let result = interp.call_function_expanded(sc_fn, &call_args)?;
            return interp.truthy_value(&result);
        }
    }
    // `cls` may be either a user-defined class (`PyClass`) or a built-in type
    // token (`BuiltinFunction("int")` etc.); anything else is a `TypeError`,
    // matching CPython.  This runs *after* the `__subclasscheck__` dispatch
    // above so a custom hook on `type(classinfo)` can accept a non-class
    // `cls` (issue #2525); it is reached per tuple/union leaf, matching
    // CPython's lazy per-leaf validation.
    if !is_class_like(cls) {
        return Err(PyError::named(
            "TypeError",
            format!("{fn_name}() arg 1 must be a class"),
        ));
    }
    // Issue #2552: structural `issubclass` for `typing.Protocol` subclasses.
    // Mirrors the `isinstance` short-circuit but checks the candidate *class*'s
    // MRO rather than an instance.  Reached only after the `arg 1 must be a
    // class` guard above, matching CPython's error precedence (a non-class
    // `cls` raises before the protocol's data-member `TypeError`).  Primitive
    // classes can never be Protocol subclasses, so the dispatch-table guard
    // keeps the hot `issubclass(x, int)` path off the base-chain walk.
    if let ValueKind::PyClass(classinfo_rc) = classinfo.kind()
        && !crate::interpreter::is_primitive_class(classinfo_rc)
        && crate::builtin_modules::typing::is_protocol_subclass(classinfo_rc)
        && !crate::builtin_modules::typing::is_protocol_marker_class(classinfo_rc)
    {
        return protocol_structural_issubclass(cls, classinfo_rc);
    }
    match (cls.kind(), classinfo.kind()) {
        // User-defined → user-defined: walk the `base` chain.
        (ValueKind::PyClass(c), ValueKind::PyClass(expected)) => {
            Ok(class_is_subclass_of(c, expected))
        }
        // User-defined → builtin type token: never a match in PyRust
        // (user classes don't inherit from built-in types here).
        (ValueKind::PyClass(_), ValueKind::BuiltinFunction(_)) => Ok(false),
        // Builtin type token → builtin type token: handle the small
        // hard-coded relations (`bool` ⊂ `int`, anything ⊂ itself,
        // anything ⊂ `object`).
        (ValueKind::BuiltinFunction(a), ValueKind::BuiltinFunction(b)) => {
            Ok(builtin_is_subclass_of(a, b))
        }
        // Builtin → user-defined: never matches.
        (ValueKind::BuiltinFunction(_), ValueKind::PyClass(_)) => Ok(false),
        (_, ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_)) => Ok(false),
        _ => Err(PyError::named(
            "TypeError",
            format!("{fn_name}() arg 2 must be a class, a tuple of classes, or a union"),
        )),
    }
}

/// True if built-in type token `a` is a subclass of token `b`.  Only
/// the CPython-documented built-in relations matter here: every type
/// is a subclass of itself and of `object`; `bool` is a subclass of
/// `int`.
fn builtin_is_subclass_of(a: &str, b: &str) -> bool {
    if a == b || b == "object" {
        return true;
    }
    matches!((a, b), ("bool", "int"))
}

/// True if `v` looks like a class-info leaf accepted by
/// `isinstance`/`issubclass` — either a user-defined `PyClass` or a
/// built-in type token (`BuiltinFunction("int")` etc.).
fn is_class_like(v: &Value) -> bool {
    matches!(
        v.kind(),
        ValueKind::PyClass(_) | ValueKind::BuiltinFunction(_),
    )
}
