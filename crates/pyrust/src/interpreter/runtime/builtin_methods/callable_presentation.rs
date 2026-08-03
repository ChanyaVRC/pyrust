// ─────────────────────────────────────────────────────────────────────────────
// Helper free functions
// ─────────────────────────────────────────────────────────────────────────────

// `unalias_args_for_mutation` was removed in #448.  Its job was to
// satisfy the manual aliasing-safety contract that `as_list_mut` /
// `as_dict_mut` / `as_set_mut` documented.  With those accessors gone
// and the new scoped-borrow API in place (`list_with_mut`,
// `dict_with_mut`, `set_with_mut`, …), the dispatcher no longer holds
// a `&mut <storage>` across calls into the builtin, so no aliasing
// window exists to pre-empt.

/// Issue #2397: classify an unbound builtin-dunder `BuiltinFunction` name
/// (`"list.__len__"`, `"int.__add__"`, …) as a CPython *slot wrapper*
/// (`wrapper_descriptor`) and split it into `(type_name, dunder)`.
///
/// CPython exposes the type-level form of a slot dunder as a
/// `wrapper_descriptor` whose `repr` reads `<slot wrapper '__X__' of 'TYPE'
/// objects>` — but a handful of container slots are `method_descriptor`s
/// instead (`mp_subscript`/`sq_contains`/`__reversed__`), and those keep the
/// generic `builtin_function_or_method` presentation here.  This predicate is
/// the single seam that drives both the `repr` (line ~4348) and the
/// `type(...).__name__` (`builtins.rs::value_class`) for unbound slot dunders,
/// so they stay in lockstep.  The method_descriptor exception set mirrors
/// `runtime/builtin_methods::is_named_protocol_wrapper`. Verified against
/// `python3.12`.
///
/// Returns `None` for non-dunder builtin names (`"list.append"`), bare names
/// (`"len"`), and the method_descriptor container slots.
fn slot_wrapper_parts(name: &str) -> Option<(&str, &str)> {
    let (type_name, dunder) = name.rsplit_once('.')?;
    if !(dunder.starts_with("__") && dunder.ends_with("__") && dunder.len() > 4) {
        return None;
    }
    if matches!(
        type_name,
        "dict_keys" | "dict_items" | "dict_values" | "odict_keys" | "odict_items" | "odict_values"
    ) && matches!(dunder, "__repr__" | "__getattribute__")
    {
        return Some((type_name, dunder));
    }
    // Issue #2433: the object-inherited slot dunders.  CPython exposes these as
    // `wrapper_descriptor`s owned by `object` (`<slot wrapper '__init__' of
    // 'object' objects>`).  `__reduce__`/`__reduce_ex__`/`__sizeof__`/`__dir__`/
    // `__format__` are NOT here — they are `method_descriptor`s, handled by
    // `method_descriptor_parts`. `__new__`/`__init_subclass__`/`__subclasshook__`
    // are static/class builtins (`builtin_function_or_method`) and excluded.
    if type_name == "object" {
        return matches!(
            dunder,
            "__delattr__"
                | "__setattr__"
                | "__getattribute__"
                | "__init__"
                | "__str__"
                | "__hash__"
                | "__repr__"
                | "__eq__"
                | "__ne__"
                | "__lt__"
                | "__le__"
                | "__gt__"
                | "__ge__"
        )
        .then_some((type_name, dunder));
    }
    // Issue #2433: type-owned `__hash__`/`__repr__`/`__str__` (synthesised as
    // `BuiltinFunction("str.__hash__")` etc. by `primitive_owned_object_dunder`)
    // are `wrapper_descriptor`s (`<slot wrapper '__repr__' of 'str' objects>`).
    // `__format__` is a `method_descriptor` (handled by `method_descriptor_parts`).
    if matches!(dunder, "__hash__" | "__repr__" | "__str__")
        && matches!(
            type_name,
            "int"
                | "float"
                | "complex"
                | "str"
                | "bytes"
                | "bytearray"
                | "tuple"
                | "frozenset"
                | "list"
                | "dict"
                | "set"
                | "bool"
        )
    {
        return Some((type_name, dunder));
    }
    // Issue #2433: `int.__bool__`/`__float__`/`__int__` are int-owned slot
    // wrappers in CPython.  (`float.__bool__`/etc. are protocol-only here and
    // not exposed unbound, so only `int` reaches this row.)
    if type_name == "int" && matches!(dunder, "__bool__" | "__float__" | "__int__") {
        return Some((type_name, dunder));
    }
    // Issue #2297: `int.__index__` is an int-owned slot wrapper in CPython
    // (`<slot wrapper '__index__' of 'int' objects>`).  Its siblings
    // `__round__`/`__trunc__`/`__floor__`/`__ceil__` are `method_descriptor`s
    // (handled by `method_descriptor_parts`), so only `__index__` lands here.
    if type_name == "int" && dunder == "__index__" {
        return Some((type_name, dunder));
    }
    // CPython models these container slots as `method_descriptor`, not
    // `wrapper_descriptor` — keep the generic presentation for them.
    // Issue #2399: `range.__reversed__` is a method_descriptor too.
    if matches!(
        (dunder, type_name),
        ("__getitem__", "list" | "dict")
            | ("__contains__", "dict" | "set" | "frozenset")
            | ("__reversed__", "list" | "dict" | "range")
    ) {
        return None;
    }
    // Issue #2399: `range` owns `__hash__`/`__bool__`/`__repr__` as its own slot
    // wrappers (`<slot wrapper '__repr__' of 'range' objects>`), unlike the other
    // primitives where these are inherited from `object` or synthesised via the
    // type-qualified `primitive_owned_object_dunder` path.  range registers them
    // directly as `BuiltinFunction("range.__X__")` (helpers.rs RANGE_CLASS init),
    // so classify them here as wrapper_descriptor.  (`__str__` is NOT owned by
    // range — it inherits `object.__str__` — so it is excluded.)
    if type_name == "range" && matches!(dunder, "__hash__" | "__bool__" | "__repr__") {
        return Some((type_name, dunder));
    }
    // The closed set of dunders pyrust exposes unbound as slot wrappers
    // (`runtime/builtin_methods::slot_dunder_table` SLOT_ATTR rows, minus the
    // exceptions
    // above).  Listed here rather than reaching into the interpreter so the
    // predicate stays in `pyrust-core`; any divergence is caught by the
    // parity fixture's full hasattr/type matrix.
    const SLOT_WRAPPER_DUNDERS: &[&str] = &[
        "__len__",
        "__getitem__",
        "__setitem__",
        "__delitem__",
        "__contains__",
        "__add__",
        "__sub__",
        "__mul__",
        "__truediv__",
        "__floordiv__",
        "__mod__",
        "__rmod__",
        "__pow__",
        "__and__",
        "__rand__",
        "__or__",
        "__ror__",
        "__xor__",
        "__rxor__",
        // Issue #2424: `bool.__invert__` is a bool-owned slot wrapper.  Only
        // ever resolved for `bool` here (`int.__invert__` isn't exposed), so
        // adding it doesn't broaden any other type's surface.
        "__invert__",
        // Issue #2536: the unary number-protocol slots (`nb_negative`/
        // `nb_positive`/`nb_absolute`), `nb_bool`, and the reflected arithmetic
        // slots.  `complex` is the first primitive to expose these unbound
        // (`complex.__neg__` → `<slot wrapper '__neg__' of 'complex' objects>`);
        // `int`/`float` keep them protocol-only (and `int`/`range` `__bool__`
        // resolve via earlier type-specific arms), so this only broadens
        // `complex`'s repr surface.
        "__neg__",
        "__pos__",
        "__abs__",
        "__bool__",
        "__radd__",
        "__rmul__",
        "__rtruediv__",
        "__rpow__",
        "__rsub__",
        "__lshift__",
        "__rshift__",
        "__iadd__",
        "__imul__",
        "__ior__",
        "__iand__",
        "__isub__",
        "__ixor__",
        "__iter__",
        "__eq__",
        "__ne__",
        "__lt__",
        "__le__",
        "__gt__",
        "__ge__",
    ];
    if SLOT_WRAPPER_DUNDERS.contains(&dunder) {
        Some((type_name, dunder))
    } else {
        None
    }
}

/// The twelve built-in primitive types whose unbound C-level methods CPython
/// 3.12 exposes as `method_descriptor` / slot-wrapper descriptors.  Used to
/// distinguish a type-method `BuiltinFunction` name (`"list.append"`) from a
/// module function (`"math.sqrt"`, which stays `builtin_function_or_method`).
const PRIMITIVE_TYPE_NAMES: &[&str] = &[
    "list",
    "str",
    "bytes",
    "tuple",
    "bytearray",
    "dict",
    "set",
    "frozenset",
    "int",
    "bool",
    "float",
    "complex",
];

/// Issue #2422: classify an unbound builtin method `BuiltinFunction` name
/// (`"list.append"`, `"dict.__getitem__"`, …) as a CPython *method_descriptor*
/// and split it into `(type_name, method)`.
///
/// CPython exposes the unbound C-level methods of a built-in type as
/// `method_descriptor`s whose `repr` reads `<method '<m>' of '<type>'
/// objects>`.  This is distinct from the slot-wrapper dunders handled by
/// [`slot_wrapper_parts`] (`wrapper_descriptor`, `<slot wrapper …>`), which
/// take precedence here.  The single seam that drives the unbound `repr`
/// (line ~4354) and `type(...).__name__` (`builtins.rs::value_class`).
///
/// Scope (matches issue #2422): plain (non-dunder) methods of the twelve
/// primitive types (`append`, `upper`, `get`, …), plus the method_descriptor
/// container dunders `__getitem__` (list/dict), `__contains__`
/// (dict/set/frozenset), `__reversed__` (list/dict) — empirically the same
/// partition as [`slot_wrapper_parts`]'s exception set, verified against
/// `python3.12`.  Object-inherited dunders (`object.__reduce__`,
/// `str.__format__`, `__init__`/`__new__`/`__repr__`/`__hash__`, …) form a
/// larger separate CPython classification matrix and are deliberately out of
/// scope (still `builtin_function_or_method` here); see the #2422 follow-up.
///
/// Returns `None` for module functions (`"math.sqrt"`), bare names (`"len"`),
/// `object.*`-inherited names, slot-wrapper dunders, and the deferred dunders.
fn method_descriptor_parts(name: &str) -> Option<(&str, &str)> {
    let (type_name, method) = name.rsplit_once('.')?;
    if matches!(
        type_name,
        "dict_keys" | "dict_items" | "dict_values" | "odict_keys" | "odict_items" | "odict_values"
    ) && (method == "__reversed__"
        || method == "isdisjoint" && matches!(type_name, "dict_keys" | "dict_items"))
    {
        return Some((type_name, method));
    }
    // Issue #2399: `range.__reversed__` is a method_descriptor (every other
    // range dunder is a slot wrapper, handled by `slot_wrapper_parts`).
    // `range` is not in `PRIMITIVE_TYPE_NAMES` (it is a VM-native type), so the
    // guard below would otherwise reject it.
    if (type_name, method) == ("range", "__reversed__") {
        return Some((type_name, method));
    }
    // Issue #2433: `object.__reduce__`/`__reduce_ex__`/`__sizeof__`/`__dir__`/
    // `__format__` are the object-inherited *method_descriptor*s (`<method
    // '__reduce__' of 'object' objects>`).  `object` is not in
    // `PRIMITIVE_TYPE_NAMES`, so handle it before that guard.  The remaining
    // `object.*` dunders are slot wrappers (see `slot_wrapper_parts`) or static
    // builtins (`__new__`/`__init_subclass__`/`__subclasshook__`).
    if type_name == "object" {
        return matches!(
            method,
            "__reduce__" | "__reduce_ex__" | "__sizeof__" | "__dir__" | "__format__"
        )
        .then_some((type_name, method));
    }
    if !PRIMITIVE_TYPE_NAMES.contains(&type_name) {
        return None;
    }
    // Slot-wrapper dunders are `wrapper_descriptor`, not method_descriptor.
    if slot_wrapper_parts(name).is_some() {
        return None;
    }
    // Issue #2433: type-owned `__format__` (`str`/`int`/`float`, synthesised by
    // `primitive_owned_object_dunder`) is a `method_descriptor`
    // (`<method '__format__' of 'str' objects>`), unlike the sibling
    // `__hash__`/`__repr__`/`__str__` slot wrappers handled above.
    if method == "__format__" {
        return Some((type_name, method));
    }
    let is_dunder = method.starts_with("__") && method.ends_with("__") && method.len() > 4;
    if is_dunder {
        // The method_descriptor container dunders, plus the int/float numeric
        // method_descriptors (issue #2297/#2481): `int`/`float`.`__round__`/
        // `__trunc__`/`__floor__`/`__ceil__` are `method_descriptor`s in CPython
        // (`<method '__round__' of 'int' objects>`), unlike the sibling
        // `int.__index__` slot wrapper (handled by `slot_wrapper_parts`).
        if matches!(
            (method, type_name),
            ("__getitem__", "list" | "dict")
                | ("__contains__", "dict" | "set" | "frozenset")
                | ("__reversed__", "list" | "dict")
                | (
                    "__round__" | "__trunc__" | "__floor__" | "__ceil__",
                    "int" | "float"
                )
        ) {
            return Some((type_name, method));
        }
        return None;
    }
    // Plain (non-dunder) builtin method: `list.append`, `str.upper`, …
    Some((type_name, method))
}

/// Return the Python-facing callable category and display components for an
/// interpreter-owned builtin dispatch key.
///
/// The core value representation consumes this typed result through an
/// installed callback. Concrete Python type/method tables stay here with the
/// builtin provider; core neither parses dotted keys nor names Python APIs.
pub(crate) fn builtin_callable_presentation(
    name: &str,
) -> pyrust_core::BuiltinCallablePresentation<'_> {
    if let Some((owner, name)) = slot_wrapper_parts(name) {
        return pyrust_core::BuiltinCallablePresentation::WrapperDescriptor { owner, name };
    }
    if let Some((owner, name)) = method_descriptor_parts(name) {
        return pyrust_core::BuiltinCallablePresentation::MethodDescriptor { owner, name };
    }
    let python_name = match crate::builtin_registry::lookup_metadata(name) {
        Some(metadata)
            if metadata.kind == crate::builtin_registry::BuiltinCallableKind::ModuleFunction =>
        {
            metadata.python_name()
        }
        _ => name,
    };
    pyrust_core::BuiltinCallablePresentation::Function { name: python_name }
}
